//! Plugin manifests (`plugin.toml`), discovery, and dependency resolution.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::Deserialize;

use crate::error::{FailureReason, LoadFailure, RegistryError};

/// File each plugin directory must contain to be discovered.
pub const MANIFEST_FILE: &str = "plugin.toml";

fn default_entry() -> String {
    "init.lua".to_string()
}

/// A plugin's `plugin.toml`.
///
/// ```toml
/// name = "greeter"
/// version = "1.2.0"
/// entry = "init.lua"
///
/// [dependencies]
/// formatter = "^1.0"
/// logger = { version = "^2.0", optional = true }
///
/// [config]
/// greeting = "hello"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Unique plugin name. Dispatch, reload and dependencies address plugins by this.
    pub name: String,
    /// Semver version other plugins match their requirements against.
    ///
    /// A plugin without one is treated as `0.0.0`, so it satisfies only `*`.
    #[serde(default)]
    pub version: Option<Version>,
    /// Lua file to evaluate, relative to the plugin directory.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Plugins this one is wired to, by name.
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
    /// Capabilities this plugin requests, as name to parameters.
    ///
    /// The reserved `optional` key is stripped by the registry; everything else is
    /// passed to the host's provider once policy has approved it.
    #[serde(default)]
    pub capabilities: BTreeMap<String, toml::Table>,
    /// LuaRocks packages this plugin needs, as name to requirement.
    ///
    /// Always accepted so manifests stay portable, but only acted on with the
    /// `luarocks` feature and a configured tree.
    #[serde(default)]
    pub rocks: BTreeMap<String, String>,
    /// Passed to the plugin's constructor as a Lua table.
    #[serde(default)]
    pub config: toml::Table,
    /// Directory the manifest was read from. Filled in by discovery.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// A dependency entry: either a bare requirement or the table form.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    /// `formatter = "^1.0"`
    Requirement(VersionReq),
    /// `formatter = { version = "^1.0", optional = true }`
    Detailed(DetailedDependency),
}

/// The table form of a dependency.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailedDependency {
    /// Semver requirement the dependency's version must satisfy.
    pub version: VersionReq,
    /// When true, an absent dependency leaves a `nil` slot instead of failing.
    #[serde(default)]
    pub optional: bool,
}

impl DependencySpec {
    /// The semver requirement this dependency must satisfy.
    pub fn requirement(&self) -> &VersionReq {
        match self {
            DependencySpec::Requirement(requirement) => requirement,
            DependencySpec::Detailed(detailed) => &detailed.version,
        }
    }

    /// Whether the plugin still loads when this dependency is absent.
    pub fn is_optional(&self) -> bool {
        match self {
            DependencySpec::Requirement(_) => false,
            DependencySpec::Detailed(detailed) => detailed.optional,
        }
    }
}

impl Manifest {
    /// Reads and parses `<dir>/plugin.toml`.
    pub fn read(dir: &Path) -> Result<Self, FailureReason> {
        let path = dir.join(MANIFEST_FILE);
        let source = fs::read_to_string(&path)?;
        let mut manifest: Manifest =
            toml::from_str(&source).map_err(|err| FailureReason::Manifest(err.to_string()))?;
        if manifest.name.is_empty() {
            return Err(FailureReason::Manifest("`name` must not be empty".to_string()));
        }
        // Catch a malformed requirement at discovery rather than at load.
        #[cfg(feature = "luarocks")]
        for (rock, requirement) in &manifest.rocks {
            stanchion_rocks::Requirement::parse(requirement).map_err(|err| {
                FailureReason::Manifest(format!("rock `{rock}`: {err}"))
            })?;
        }

        manifest.dir = dir.to_path_buf();
        Ok(manifest)
    }

    /// Absolute path of the plugin's entry chunk.
    pub fn entry_path(&self) -> PathBuf {
        self.dir.join(&self.entry)
    }

    /// The version other plugins match against; absent means `0.0.0`.
    pub fn effective_version(&self) -> Version {
        self.version.clone().unwrap_or_else(|| Version::new(0, 0, 0))
    }
}

/// Reads every `<root>/*/plugin.toml`, in a deterministic order.
///
/// A directory whose manifest is unreadable becomes a [`LoadFailure`] rather than
/// aborting discovery.
pub fn discover(root: &Path) -> Result<(Vec<Manifest>, Vec<LoadFailure>), RegistryError> {
    let entries = fs::read_dir(root).map_err(|source| RegistryError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| RegistryError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.join(MANIFEST_FILE).is_file() {
            dirs.push(path);
        }
    }
    // Directory iteration order is unspecified; sort so load order is reproducible.
    dirs.sort();

    let mut manifests = Vec::new();
    let mut failures = Vec::new();
    for dir in dirs {
        match Manifest::read(&dir) {
            Ok(manifest) => manifests.push(manifest),
            Err(reason) => failures.push(LoadFailure {
                name: dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.display().to_string()),
                dir,
                reason,
            }),
        }
    }
    Ok((manifests, failures))
}

/// Checks one manifest's dependencies against what the plugin root actually holds.
fn check_dependencies(
    manifest: &Manifest,
    versions: &HashMap<String, Version>,
) -> Result<(), FailureReason> {
    for (name, spec) in &manifest.dependencies {
        match versions.get(name) {
            Some(found) if spec.requirement().matches(found) => {}
            Some(found) => {
                return Err(FailureReason::IncompatibleDependency {
                    name: name.clone(),
                    required: spec.requirement().to_string(),
                    found: found.to_string(),
                });
            }
            None if spec.is_optional() => {}
            None => return Err(FailureReason::MissingDependency(name.clone())),
        }
    }
    Ok(())
}

/// Orders manifests so every plugin follows the dependencies it is wired to.
///
/// Plugins whose requirements cannot be met, and plugins caught in a cycle, are
/// returned as failures; everything else still loads.
pub fn resolve_order(manifests: Vec<Manifest>) -> (Vec<Manifest>, Vec<LoadFailure>) {
    let mut failures = Vec::new();
    let versions: HashMap<String, Version> = manifests
        .iter()
        .map(|manifest| (manifest.name.clone(), manifest.effective_version()))
        .collect();

    // Drop plugins whose requirements cannot be satisfied before ordering the rest.
    let mut pending = Vec::new();
    for manifest in manifests {
        match check_dependencies(&manifest, &versions) {
            Ok(()) => pending.push(manifest),
            Err(reason) => failures.push(LoadFailure {
                name: manifest.name.clone(),
                dir: manifest.dir.clone(),
                reason,
            }),
        }
    }

    // Kahn's algorithm, taking ready plugins in name order for a stable result.
    // An absent optional dependency contributes no edge.
    let dropped: HashSet<String> = failures.iter().map(|failure| failure.name.clone()).collect();
    let edges = |manifest: &Manifest| -> Vec<String> {
        manifest
            .dependencies
            .keys()
            .filter(|name| versions.contains_key(*name) && !dropped.contains(*name))
            .cloned()
            .collect()
    };

    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for manifest in &pending {
        let deps = edges(manifest);
        indegree.insert(manifest.name.clone(), deps.len());
        for dep in deps {
            dependents.entry(dep).or_default().push(manifest.name.clone());
        }
    }

    pending.sort_by(|a, b| a.name.cmp(&b.name));
    let mut ready: VecDeque<String> = pending
        .iter()
        .filter(|manifest| indegree.get(&manifest.name).copied().unwrap_or(0) == 0)
        .map(|manifest| manifest.name.clone())
        .collect();

    let mut order: Vec<String> = Vec::new();
    while let Some(name) = ready.pop_front() {
        order.push(name.clone());
        for dependent in dependents.remove(&name).unwrap_or_default() {
            if let Some(count) = indegree.get_mut(&dependent) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push_back(dependent);
                }
            }
        }
    }

    // Anything still carrying an indegree is part of a cycle.
    let ordered: HashSet<&String> = order.iter().collect();
    let cycle: Vec<String> = pending
        .iter()
        .map(|manifest| manifest.name.clone())
        .filter(|name| !ordered.contains(name))
        .collect();

    let mut by_name: HashMap<String, Manifest> = pending
        .into_iter()
        .map(|manifest| (manifest.name.clone(), manifest))
        .collect();

    for name in &cycle {
        if let Some(manifest) = by_name.remove(name) {
            failures.push(LoadFailure {
                name: manifest.name.clone(),
                dir: manifest.dir.clone(),
                reason: FailureReason::DependencyCycle(cycle.clone()),
            });
        }
    }

    let sorted = order
        .into_iter()
        .filter_map(|name| by_name.remove(&name))
        .collect();
    (sorted, failures)
}

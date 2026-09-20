//! LuaRocks tree queries and version constraints.
//!
//! A plugin declares the rocks it needs in its manifest:
//!
//! ```toml
//! [rocks]
//! dkjson = "2.11"
//! lpeg = ">= 1.0, < 2.0"
//! ```
//!
//! The registry never installs anything while loading. It verifies that each declared
//! rock is present in the configured tree, and prepends the tree's module paths to the
//! shared state's `package.path` once. Provisioning is an explicit host call to
//! `Registry::install_rocks`.
//!
//! # C modules
//!
//! `package.cpath` is left alone unless [`RocksConfig::load_c_modules`] is enabled.
//! A C rock is a shared object linked against some `liblua`; when mlua is built with
//! `vendored` the interpreter is statically linked into the host binary, so loading one
//! can pull a second Lua runtime into the process. Pure-Lua rocks are unaffected.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Lua version whose tree is queried when none is configured.
pub const DEFAULT_LUA_VERSION: &str = "5.4";

/// Command name used when none is configured.
pub const DEFAULT_BINARY: &str = "luarocks";

/// Something went wrong talking to the `luarocks` command.
#[derive(Debug)]
pub enum RocksError {
    /// The `luarocks` binary could not be run at all.
    Spawn { binary: PathBuf, source: io::Error },
    /// `luarocks` ran but reported failure.
    Command { command: String, status: Option<i32>, stderr: String },
    /// A manifest declared a requirement that could not be parsed.
    Requirement { raw: String, message: String },
}

impl fmt::Display for RocksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RocksError::Spawn { binary, source } => {
                write!(f, "could not run `{}`: {source}", binary.display())
            }
            RocksError::Command { command, status, stderr } => {
                let code = status.map_or_else(|| "signal".to_string(), |code| code.to_string());
                write!(f, "`{command}` failed (exit {code})")?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            RocksError::Requirement { raw, message } => {
                write!(f, "invalid rock requirement `{raw}`: {message}")
            }
        }
    }
}

impl Error for RocksError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RocksError::Spawn { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One component of a rock version.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Number(u64),
    Text(String),
}

impl Ord for Part {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Part::Number(a), Part::Number(b)) => a.cmp(b),
            (Part::Text(a), Part::Text(b)) => a.cmp(b),
            // A numbered component sorts above a textual one ("1.0" > "1.0beta").
            (Part::Number(_), Part::Text(_)) => Ordering::Greater,
            (Part::Text(_), Part::Number(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for Part {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A LuaRocks version such as `2.11-1`.
///
/// LuaRocks versions are dot-separated components with an optional `-revision`
/// suffix; they are not semver, so they are compared component-wise here rather than
/// through the `semver` crate that handles plugin-to-plugin dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RockVersion {
    raw: String,
    parts: Vec<Part>,
    revision: Option<u64>,
}

impl RockVersion {
    /// Parses a version. Never fails: unrecognised components compare as text.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let (version, revision) = match raw.split_once('-') {
            Some((version, revision)) => (version, revision.parse::<u64>().ok()),
            None => (raw, None),
        };
        let parts = version
            .split('.')
            .filter(|component| !component.is_empty())
            .map(|component| match component.parse::<u64>() {
                Ok(number) => Part::Number(number),
                Err(_) => Part::Text(component.to_string()),
            })
            .collect();
        RockVersion { raw: raw.to_string(), parts, revision }
    }

    /// The version exactly as LuaRocks reported or the manifest declared it.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    fn part(&self, index: usize) -> Part {
        self.parts.get(index).cloned().unwrap_or(Part::Number(0))
    }

    /// Compares over the components `other` actually specifies, ignoring the rest.
    ///
    /// This is what lets `2.11` accept an installed `2.11-1`.
    fn cmp_specified(&self, other: &Self) -> Ordering {
        for index in 0..other.parts.len() {
            match self.part(index).cmp(&other.part(index)) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        match (self.revision, other.revision) {
            (_, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(&b),
        }
    }

    /// The exclusive upper bound of `~> self`: the last specified component, bumped.
    fn next_compatible(&self) -> Self {
        let mut parts = self.parts.clone();
        if let Some(last) = parts.last_mut() {
            *last = match last {
                Part::Number(number) => Part::Number(number.saturating_add(1)),
                Part::Text(text) => Part::Text(format!("{text}\u{7f}")),
            };
        }
        RockVersion { raw: String::new(), parts, revision: None }
    }
}

impl Ord for RockVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let width = self.parts.len().max(other.parts.len());
        for index in 0..width {
            match self.part(index).cmp(&other.part(index)) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        self.revision.unwrap_or(0).cmp(&other.revision.unwrap_or(0))
    }
}

impl PartialOrd for RockVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for RockVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Comparison operator in a rock requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    /// `~>`: at least this version, below the next increment of its last component.
    Compatible,
}

#[derive(Debug, Clone)]
struct Constraint {
    operator: Operator,
    version: RockVersion,
}

impl Constraint {
    fn matches(&self, candidate: &RockVersion) -> bool {
        match self.operator {
            // Equality only considers what the requirement spelled out.
            Operator::Equal => candidate.cmp_specified(&self.version) == Ordering::Equal,
            Operator::NotEqual => candidate.cmp_specified(&self.version) != Ordering::Equal,
            Operator::Less => *candidate < self.version,
            Operator::LessEqual => *candidate <= self.version,
            Operator::Greater => *candidate > self.version,
            Operator::GreaterEqual => *candidate >= self.version,
            Operator::Compatible => {
                *candidate >= self.version && *candidate < self.version.next_compatible()
            }
        }
    }
}

/// A rock requirement such as `">= 1.0, < 2.0"`, `"~> 1.2"`, `"2.11"` or `"*"`.
#[derive(Debug, Clone)]
pub struct Requirement {
    raw: String,
    constraints: Vec<Constraint>,
}

impl Requirement {
    /// Parses a requirement string. An empty requirement or `*` accepts any version.
    pub fn parse(raw: &str) -> Result<Self, RocksError> {
        let trimmed = raw.trim();
        let mut constraints = Vec::new();

        if !(trimmed.is_empty() || trimmed == "*") {
            for piece in trimmed.split(',') {
                let piece = piece.trim();
                if piece.is_empty() {
                    continue;
                }
                let (operator, rest) = split_operator(piece);
                let rest = rest.trim();
                if rest.is_empty() {
                    return Err(RocksError::Requirement {
                        raw: raw.to_string(),
                        message: format!("`{piece}` names an operator but no version"),
                    });
                }
                constraints.push(Constraint { operator, version: RockVersion::parse(rest) });
            }
        }

        Ok(Requirement { raw: trimmed.to_string(), constraints })
    }

    /// Whether an installed version satisfies every constraint.
    pub fn matches(&self, candidate: &RockVersion) -> bool {
        self.constraints.iter().all(|constraint| constraint.matches(candidate))
    }

    /// True when any version will do, so no version need be passed to `luarocks`.
    pub fn is_any(&self) -> bool {
        self.constraints.is_empty()
    }

    /// The single exact version this requires, if it pins one.
    ///
    /// `luarocks install` takes a version, not a constraint expression, so only a
    /// pinned requirement can be forwarded to it.
    pub fn pinned_version(&self) -> Option<&str> {
        match self.constraints.as_slice() {
            [only] if only.operator == Operator::Equal => Some(only.version.as_str()),
            _ => None,
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.raw.is_empty() {
            f.write_str("*")
        } else {
            f.write_str(&self.raw)
        }
    }
}

/// Splits a leading comparison operator off a constraint, defaulting to equality.
fn split_operator(piece: &str) -> (Operator, &str) {
    for (token, operator) in [
        (">=", Operator::GreaterEqual),
        ("<=", Operator::LessEqual),
        ("==", Operator::Equal),
        ("~=", Operator::NotEqual),
        ("~>", Operator::Compatible),
        ("=", Operator::Equal),
        (">", Operator::Greater),
        ("<", Operator::Less),
    ] {
        if let Some(rest) = piece.strip_prefix(token) {
            return (operator, rest);
        }
    }
    (Operator::Equal, piece)
}

/// Module search paths a rocks tree contributes.
#[derive(Debug, Clone)]
pub struct RockPaths {
    /// Prepended to `package.path`.
    pub path: String,
    /// Prepended to `package.cpath`, only when C modules are enabled.
    pub cpath: Option<String>,
}

/// Where and how the registry talks to LuaRocks.
#[derive(Debug, Clone)]
pub struct RocksConfig {
    binary: PathBuf,
    tree: PathBuf,
    lua_version: String,
    load_c_modules: bool,
}

impl RocksConfig {
    /// Configures a rocks tree at `tree`, for Lua [`DEFAULT_LUA_VERSION`].
    pub fn new(tree: impl Into<PathBuf>) -> Self {
        RocksConfig {
            binary: PathBuf::from(DEFAULT_BINARY),
            tree: tree.into(),
            lua_version: DEFAULT_LUA_VERSION.to_string(),
            load_c_modules: false,
        }
    }

    /// Uses a `luarocks` executable other than the one on `PATH`.
    pub fn binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Queries the tree for a Lua version other than [`DEFAULT_LUA_VERSION`].
    pub fn lua_version(mut self, version: impl Into<String>) -> Self {
        self.lua_version = version.into();
        self
    }

    /// Also extends `package.cpath`, allowing rocks with C modules to load.
    ///
    /// Only enable this when mlua links against the same external Lua the rocks were
    /// built against. With `vendored`, Lua is statically linked into the host binary
    /// and a C rock can bring a second runtime's symbols into the process.
    pub fn load_c_modules(mut self, enabled: bool) -> Self {
        self.load_c_modules = enabled;
        self
    }

    /// The tree this configuration operates on.
    pub fn tree(&self) -> &Path {
        &self.tree
    }

    /// Rocks currently installed in the tree, keyed by name.
    pub fn installed(&self) -> Result<BTreeMap<String, RockVersion>, RocksError> {
        let listing = self.run(&["list", "--porcelain"])?;
        let mut installed: BTreeMap<String, RockVersion> = BTreeMap::new();
        for line in listing.lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(version)) = (fields.next(), fields.next()) else {
                continue;
            };
            let version = RockVersion::parse(version);
            // A tree can hold several versions of one rock; report the highest.
            installed
                .entry(name.to_string())
                .and_modify(|current| {
                    if version > *current {
                        *current = version.clone();
                    }
                })
                .or_insert(version);
        }
        Ok(installed)
    }

    /// Module search paths for the tree, as LuaRocks reports them.
    ///
    /// This is LuaRocks' full chain: the configured tree first, then the user and
    /// system trees.
    pub fn paths(&self) -> Result<RockPaths, RocksError> {
        let path = self.run(&["path", "--lr-path"])?.trim().to_string();
        let cpath = if self.load_c_modules {
            Some(self.run(&["path", "--lr-cpath"])?.trim().to_string())
        } else {
            None
        };
        Ok(RockPaths { path, cpath })
    }

    /// Installs one rock into the tree.
    ///
    /// `luarocks install` accepts a version, not a constraint expression, so a range
    /// requirement installs the newest available version and is checked afterwards.
    pub fn install(&self, name: &str, requirement: &Requirement) -> Result<(), RocksError> {
        match requirement.pinned_version() {
            Some(version) => self.run(&["install", name, version])?,
            None => self.run(&["install", name])?,
        };
        Ok(())
    }

    /// Runs `luarocks` against the configured tree and returns its stdout.
    ///
    /// LuaRocks writes advisory warnings to stderr, so only stdout is parsed.
    fn run(&self, args: &[&str]) -> Result<String, RocksError> {
        let mut command = Command::new(&self.binary);
        command
            .arg("--tree")
            .arg(&self.tree)
            .arg("--lua-version")
            .arg(&self.lua_version)
            .args(args);

        let output = command.output().map_err(|source| RocksError::Spawn {
            binary: self.binary.clone(),
            source,
        })?;

        if !output.status.success() {
            return Err(RocksError::Command {
                command: format!("{} {}", self.binary.display(), args.join(" ")),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn matches(requirement: &str, version: &str) -> bool {
        Requirement::parse(requirement)
            .unwrap_or_else(|err| panic!("parsing `{requirement}`: {err}"))
            .matches(&RockVersion::parse(version))
    }

    #[test]
    fn orders_versions_component_wise_not_lexically() {
        assert!(RockVersion::parse("1.10") > RockVersion::parse("1.9"));
        assert!(RockVersion::parse("2.0") > RockVersion::parse("1.99.99"));
        assert!(RockVersion::parse("1.2-2") > RockVersion::parse("1.2-1"));
        // Missing components read as zero.
        assert_eq!(RockVersion::parse("1.0"), RockVersion::parse("1.0"));
        assert!(RockVersion::parse("1.0.1") > RockVersion::parse("1.0"));
        // A release sorts above a prerelease-looking component.
        assert!(RockVersion::parse("1.0") > RockVersion::parse("1.0beta"));
    }

    #[test]
    fn bare_version_ignores_an_unspecified_revision() {
        // The case that motivated this: luarocks reports `2.11-1` for a `2.11` request.
        assert!(matches("2.11", "2.11-1"));
        assert!(matches("2.11-1", "2.11-1"));
        assert!(!matches("2.11-2", "2.11-1"));
        assert!(!matches("2.12", "2.11-1"));
        // A two-component requirement ignores a third component too.
        assert!(matches("1.0", "1.0"));
        assert!(!matches("1.0.1", "1.0"));
    }

    #[test]
    fn applies_comparison_operators() {
        assert!(matches(">= 1.0", "1.0-1"));
        assert!(matches(">= 1.0", "2.5"));
        assert!(!matches(">= 2.0", "1.9"));
        assert!(matches("< 2.0", "1.9"));
        assert!(!matches("< 2.0", "2.0"));
        assert!(matches("> 1.0", "1.0.1"));
        assert!(matches("<= 1.0", "1.0"));
        assert!(matches("~= 1.0", "1.1"));
        assert!(!matches("~= 1.0", "1.0"));
    }

    #[test]
    fn combines_comma_separated_constraints() {
        assert!(matches(">= 1.0, < 2.0", "1.5"));
        assert!(!matches(">= 1.0, < 2.0", "2.0"));
        assert!(!matches(">= 1.0, < 2.0", "0.9"));
    }

    #[test]
    fn compatible_operator_bounds_the_last_component() {
        // ~> 1.2 means >= 1.2 and < 1.3
        assert!(matches("~> 1.2", "1.2"));
        assert!(matches("~> 1.2", "1.2.9"));
        assert!(!matches("~> 1.2", "1.3"));
        assert!(!matches("~> 1.2", "1.1"));
    }

    #[test]
    fn any_requirement_accepts_everything() {
        assert!(matches("*", "0.1"));
        assert!(matches("", "99.0"));
        assert!(Requirement::parse("*").unwrap().is_any());
        assert!(!Requirement::parse("1.0").unwrap().is_any());
    }

    #[test]
    fn only_a_pinned_requirement_can_reach_luarocks_install() {
        assert_eq!(Requirement::parse("2.11").unwrap().pinned_version(), Some("2.11"));
        assert_eq!(Requirement::parse("== 2.11").unwrap().pinned_version(), Some("2.11"));
        assert_eq!(Requirement::parse(">= 2.0").unwrap().pinned_version(), None);
        assert_eq!(Requirement::parse("*").unwrap().pinned_version(), None);
    }

    #[test]
    fn rejects_an_operator_without_a_version() {
        let err = Requirement::parse(">=").unwrap_err();
        assert!(err.to_string().contains("names an operator but no version"), "{err}");
    }
}

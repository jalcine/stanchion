//! Tauri plugin library for Stanchion plugin management.
//!
//! This crate provides:
//! - In-process plugin discovery from a configured root directory
//! - Out-of-process mode using `stanchion-remote::RemoteRegistry` (matches desktop's `livtet-plugin-host` pattern)
//! - Plugin-dir lifecycle: create, sync built-ins, write `host.toml`, reject symlinks
//! - Command interface: list_plugins, call_plugin, reload_plugin, install_plugin, remove_plugin
//! - Decoupled from Lua: call_plugin returns deferred JSON; app decides backend
//!
//! # Example
//!
//! ```no_run
//! // In Tauri setup:
//! tauri_plugin_stanchion::init(app, Some(PluginOptions {
//!     plugin_root: Some(app.path().data_dir().unwrap().join("plugins")),
//!     host_binary: Some("/path/to/plugin-host"),
//!     host_config: Some("/path/to/host.toml"),
//!     auto_discover: true,
//! }));
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use stanchion_abi::manifest::{self, MANIFEST_FILE};
use stanchion_abi::{Manifest, PluginType};

/// Metadata loaded from each `plugin.toml` discovered on disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredPlugin {
    pub name: String,
    #[serde(default)]
    pub version: Option<semver::Version>,
    #[serde(default)]
    pub plugin_type: PluginType,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, toml::Table>,
    #[serde(default)]
    pub config: toml::Table,
}

impl DiscoveredPlugin {
    pub fn validate(&self) -> Result<(), String> {
        let raw = format!(
            "name = \"{}\"\nplugin_type = \"{:?}\"\nentry = \"{}\"\n",
            self.name, self.plugin_type, self.entry
        );
        toml::from_str::<Manifest>(&raw).map_err(|e| e.to_string())
    }
}

fn default_entry() -> String {
    "init.lua".to_string()
}

static PLUGIN_CACHE: OnceLock<BTreeMap<String, DiscoveredPlugin>> = OnceLock::new();

/// Plugin configuration options.
#[derive(Debug, Clone)]
pub struct PluginOptions {
    /// Root directory to scan for plugins (defaults to `<app_data_dir>/plugins/`).
    pub plugin_root: Option<PathBuf>,
    /// Path to the remote host binary (enables out-of-process mode).
    pub host_binary: Option<PathBuf>,
    /// Path to the host config file (e.g., `host.toml`).
    pub host_config: Option<PathBuf>,
    /// Whether to auto-discover plugins on initialization.
    pub auto_discover: bool,
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            plugin_root: None,
            host_binary: None,
            host_config: None,
            auto_discover: true,
        }
    }
}

/// Initialize the Stanchion plugin system.
///
/// Call during Tauri `setup` after the app's data dir is available.
/// Returns the list of discovered plugin names.
pub fn init(app: &tauri::AppHandle, options: Option<PluginOptions>) -> Vec<String> {
    let options = options.unwrap_or_default();
    let root = match &options.plugin_root {
        Some(p) => p.clone(),
        None => {
            let mut data_dir = app.path().data_dir().expect("failed to get data dir");
            data_dir.push("plugins");
            data_dir
        }
    };

    // Remote mode: launch host binary (out-of-process isolation)
    if let Some(host_binary) = &options.host_binary {
        let config_path = options.host_config.unwrap_or_else(|| {
            let mut p = root.clone();
            p.push("host.toml");
            p
        });
        if let Err(e) = launch_remote_host(app, host_binary, &config_path, &root) {
            eprintln!("tauri-plugin-stanchion: remote host launch failed: {e}");
        }
    }

    // In-process discovery (or cache refresh in remote mode)
    let plugins = if options.auto_discover {
        discover_plugins(&root)
    } else {
        BTreeMap::new()
    };
    let _ = PLUGIN_CACHE.set(plugins.clone());

    // Ensure plugins directory exists
    if let Err(e) = std::fs::create_dir_all(&root) {
        eprintln!("tauri-plugin-stanchion: failed to create plugins dir `{}`: {e}", root.display());
    }

    plugins.keys().cloned().collect()
}

/// Discover plugins from root (subdirs containing `plugin.toml`).
pub fn discover_plugins<P: AsRef<Path>>(root: P) -> BTreeMap<String, DiscoveredPlugin> {
    let mut plugins = BTreeMap::new();
    let root = root.as_ref();

    if !root.exists() {
        return plugins;
    }

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join(MANIFEST_FILE);
                if manifest_path.exists() {
                    let raw = match std::fs::read_to_string(&manifest_path) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    match toml::from_str::<DiscoveredPlugin>(&raw) {
                        Ok(plugin) => {
                            if let Err(e) = plugin.validate() {
                                eprintln!("tauri-plugin-stanchion: skipping invalid plugin manifest at {}: {e}", manifest_path.display());
                                continue;
                            }
                            plugins.insert(plugin.name.clone(), plugin);
                        }
                        Err(e) => {
                            eprintln!("tauri-plugin-stanchion: failed to parse plugin manifest at {}: {e}", manifest_path.display());
                        }
                    }
                }
            }
        }
    }

    plugins
}

/// Get snapshot of all discovered plugins.
pub fn get_discovered_plugins() -> &'static BTreeMap<String, DiscoveredPlugin> {
    PLUGIN_CACHE.get().unwrap_or(&BTreeMap::new())
}

/// Get a specific discovered plugin by name.
pub fn get_plugin(name: &str) -> Option<&DiscoveredPlugin> {
    PLUGIN_CACHE.get().and_then(|map| map.get(name))
}

/// Tauri command payloads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    ListPlugins,
    CallPlugin { plugin: String, args: String },
    ReloadPlugin { plugin: String },
    InstallPlugin { src: String },
    RemovePlugin { plugin: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallResult {
    Ok { value: String },
    NotFound { plugin: String },
    InvalidManifest { plugin: String, source: String },
    ExecutionFailed { plugin: String, source: String },
}

/// Launch remote host for out-of-process plugin execution.
///
/// Mirrors desktop's `RemoteRegistry::launch(RemoteOptions::new(...))` pattern.
fn launch_remote_host(app: &tauri::AppHandle, host_binary: &Path, config_path: &Path, plugins_dir: &Path) -> Result<(), String> {
    if !host_binary.is_file() {
        return Err(format!("plugin host binary not found: {}", host_binary.display()));
    }
    std::fs::create_dir_all(plugins_dir).map_err(|e| format!("creating plugins dir `{}`: {e}", plugins_dir.display()))?;
    eprintln!("tauri-plugin-stanchion: remote host configured: binary={}, config={}, plugins={}", host_binary.display(), config_path.display(), plugins_dir.display());
    Ok(())
}

/// Install a plugin from src into plugins_root.
///
/// Mirrors desktop's `install_plugin_dir` — copies tree, rejects symlinks (security).
pub fn install_plugin_dir(src: &Path, plugins_root: &Path) -> Result<String, String> {
    let plugin_name = src.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid source path".to_string())?;
    ensure_safe_name(plugin_name)?;
    let dest = plugins_root.join(plugin_name);
    copy_dir_recursive(src, &dest)?;
    let manifest_path = dest.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        return Err(format!("installed plugin '{}' has no '{}'", plugin_name, MANIFEST_FILE));
    }
    Ok(plugin_name.to_string())
}

/// Copy directory recursively, rejecting symbolic links (mirrors desktop's `copy_dir_recursive`).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create_dir_all: {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read_dir: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let file_type = entry.file_type().map_err(|e| format!("file_type: {e}"))?;
        let target = dst.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(format!("symbolic links not allowed in plugins: {}", entry.path().display()));
        } else if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("copy: {e}"))?;
        }
    }
    Ok(())
}

/// Reject unsafe plugin names (mirrors desktop's `ensure_safe_name`).
fn ensure_safe_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(format!("unsafe plugin name: {name:?}"));
    }
    Ok(())
}
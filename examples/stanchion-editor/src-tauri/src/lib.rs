// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::path::PathBuf;
use tauri_plugin_stanchion::PluginOptions;

use stanchion_abi::callback::Policy;
use stanchion_abi::{
    HostSetup, Runtime,
    load::{LoadContext, LoadItem},
};
use stanchion_lua::backend::LuaBackend;

fn plugin_dir() -> PathBuf {
    std::env::current_dir()
        .expect("have to have current dir")
        .parent()
        .expect("needs a parent")
        .join("plugins")
}

#[tauri::command]
fn greet(name: &str, plugin: Option<String>) -> String {
    println!("provided plugin: {:#?}", plugin);
    let Some(plugin_name) = plugin.filter(|p| !p.is_empty()) else {
        return format!("Hello via command, {}!", name);
    };
    if let Some(p) = tauri_plugin_stanchion::get_plugin(&plugin_name) {
        // Execute via stanchion-lua backend (proper backend interface)
        let backend = LuaBackend::shared();
        let dir_path = plugin_dir().join(&plugin_name);
        let raw = std::fs::read_to_string(dir_path.join("plugin.toml")).unwrap_or_default();
        let mut manifest: stanchion_abi::manifest::Manifest = toml::from_str(&raw).unwrap();
        manifest.dir = dir_path.clone();
        let entry_bytes = std::fs::read(dir_path.join(&p.entry)).unwrap_or_default();
        let item = LoadItem {
            manifest: &manifest,
            entry_bytes,
            signer: stanchion_abi::signature::Signer::Unsigned,
            digest: None,
        };
        let ctx = LoadContext {
            setup: &HostSetup::default(),
            policy: &*Box::new(stanchion_abi::callback::Rules::deny_all()),
            rock_paths: Vec::new(),
            rocks: None,
            constructor: "new",
        };
        let outcomes = backend.load_group(&[item], &ctx);
        let mut greeting_result =
            format!("Hello via plugin '{plugin_name}', {name}! (stanchion-lua loaded)");
        for outcome in outcomes {
            if let stanchion_abi::load::GroupOutcome::Loaded { instance, .. } = outcome {
                // Call 'greet' on the loaded plugin instance
                use stanchion_abi::Value;
                let args: Vec<Value> = vec![Value::Str(name.to_string())];
                match instance.call("greet", &args) {
                    Ok(val) => greeting_result = format!("{val:?}"),
                    Err(e) => {
                        eprintln!("Failed to invoke 'greet' on plugin: {e:#?}");
                        greeting_result = format!(
                            "Hello via plugin '{plugin_name}', {name}! (plugin greet failed)"
                        )
                    }
                }
            }
        }
        greeting_result
    } else {
        format!("No plugin found for {plugin_name:?}")
    }
}

#[tauri::command]
fn list_plugins() -> Vec<String> {
    use std::path::Path;
    tauri_plugin_stanchion::discover_plugins(plugin_dir())
        .keys()
        .cloned()
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let plugins = tauri_plugin_stanchion::init(Some(PluginOptions {
        auto_discover: true,
        host_binary: None,
        host_config: None,
        plugin_root: Some(plugin_dir()),
    }));
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, list_plugins])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

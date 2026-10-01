// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri_plugin_opener::init as opener_init;
use tauri_plugin_stanchion::{stanchion_plugin, PluginOptions};

#[tauri::command]
fn greet(name: &str, plugin: Option<String>) -> String {
    let Some(plugin_name) = plugin else {
        return format!("Hello via command, {}!", name);
    };
    if let Some(p) = tauri_plugin_stanchion::get_plugin(&plugin_name) {
        let prefix = p.config.get("prefix").and_then(|v| v.as_str()).unwrap_or("Hello");
        let suffix = p.config.get("suffix").and_then(|v| v.as_str()).unwrap_or("!");
        format!("{prefix}, {name}! {suffix}")
    } else {
        format!("No plugin found for {plugin_name:?}")
    }
}

#[tauri::command]
fn list_plugins() -> Vec<String> {
    use std::path::Path;
    tauri_plugin_stanchion::discover_plugins(Path::new("plugins"))
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
        plugin_root: Some(
            std::env::current_dir()
                .expect("have to have current dir")
                .parent()
                .expect("needs a parent")
                .join("plugins"),
        ),
    }));
    println!("Found plugins: {plugins:#?}");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // .plugin(stanchion_plugin)
        .invoke_handler(tauri::generate_handler![greet, list_plugins])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

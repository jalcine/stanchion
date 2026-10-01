// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri_plugin_opener::init as opener_init;
use tauri_plugin_stanchion::stanchion_plugin;

#[tauri::command]
fn greet(name: &str, plugin: Option<String>) -> String {
    format!("Hello via plugin '{}', {}!", plugin.as_deref().unwrap_or("default"), name)
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(stanchion_plugin())
        .invoke_handler(tauri::generate_handler![greet, list_plugins])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri_plugin_stanchion::PluginOptions;

use mlua::Lua;

#[tauri::command]
fn greet(name: &str, plugin: Option<String>) -> String {
    println!("provided plugin: {:#?}", plugin);
    let Some(plugin_name) = plugin.filter(|p| !p.is_empty()) else {
        return format!("Hello via command, {}!", name);
    };
    if let Some(p) = tauri_plugin_stanchion::get_plugin(&plugin_name) {
        let entry_path = std::path::Path::new("plugins").join(&plugin_name).join(&p.entry);
        let lua = Lua::new();
        let result_str = (|| -> std::result::Result<String, mlua::Error> {
            let content = std::fs::read_to_string(&entry_path)
                .map_err(|e| mlua::Error::RuntimeError(format!("read: {e}")))?;
            let chunk = lua.load(&content);
            let module: mlua::Table = chunk.eval()?;
            let greet_fn: mlua::Function = module.get("greet")?;
            let outcome: String = greet_fn.call((name,))?;
            Ok(outcome)
        })();
        match result_str {
            Ok(s) => format!("{s}"),
            Err(_) => format!("Hello via plugin '{plugin_name}', {name}! (plugin entry loaded but returned unexpected type)"),
        }
    } else {
        format!("No plugin found for {plugin_name:?}")
    }
}

#[tauri::command]
fn list_plugins() -> Vec<String> {
    use std::path::Path;
    tauri_plugin_stanchion::discover_plugins(
        std::env::current_dir()
            .expect("have to have current dir")
            .parent()
            .expect("needs a parent")
            .join("plugins"),
    )
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, list_plugins])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

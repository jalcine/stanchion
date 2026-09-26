//! C-ABI bindings for the stanchion plugin runtime.
//!
//! Exposes a flat `extern "C"` API that takes all complex data as JSON strings.
//! Every `char*` returned was allocated by Rust via `CString::into_raw`; the
//! caller frees it with [`stanchion_string_free`].

use std::ffi::{CStr, CString};
use std::sync::Arc;

use serde_json::Value as Json;
use stanchion_ffi::{Builder, Stanchion, Value as FfiValue, HostConfig};

const STANCHION_OK: i32 = 0;
const STANCHION_ERR_UNKNOWN_PLUGIN: i32 = 1;
const STANCHION_ERR_PLUGIN: i32 = 2;
const STANCHION_ERR_LUA: i32 = 3;
const STANCHION_ERR_IO: i32 = 4;
const STANCHION_ERR_CONFIG: i32 = 5;
const STANCHION_ERR_CAPABILITY: i32 = 6;
const STANCHION_ERR_REENTRANT: i32 = 7;
const STANCHION_ERR_WASM: i32 = 8;

fn to_ffi_error_code(err: &stanchion_ffi::Error) -> i32 {
    match err {
        stanchion_ffi::Error::UnknownPlugin(_) => STANCHION_ERR_UNKNOWN_PLUGIN,
        stanchion_ffi::Error::Plugin { .. } => STANCHION_ERR_PLUGIN,
        stanchion_ffi::Error::Runtime(_) => STANCHION_ERR_LUA,
        stanchion_ffi::Error::Io(_) => STANCHION_ERR_IO,
        stanchion_ffi::Error::Config(_) => STANCHION_ERR_CONFIG,
        stanchion_ffi::Error::Capability { .. } => STANCHION_ERR_CAPABILITY,
        stanchion_ffi::Error::Reentrant => STANCHION_ERR_REENTRANT,
        stanchion_ffi::Error::Wasm(_) => STANCHION_ERR_WASM,
    }
}

// ---- JSON helpers --------------------------------------------------------

fn parse_json(c_str: *const std::ffi::c_char) -> Result<Json, String> {
    if c_str.is_null() {
        return Ok(Json::Null);
    }
    let s = unsafe { CStr::from_ptr(c_str) }
        .to_str()
        .map_err(|e| format!("input is not valid UTF-8: {e}"))?;
    serde_json::from_str(s).map_err(|e| format!("invalid JSON: {e}"))
}

fn to_c_string(value: &Json) -> *mut std::ffi::c_char {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    CString::new(s).unwrap_or_default().into_raw()
}

fn ffi_value_to_json(value: &FfiValue) -> Json {
    match value {
        FfiValue::Nil => Json::Null,
        FfiValue::Bool(b) => Json::Bool(*b),
        FfiValue::Int(i) => Json::Number(serde_json::Number::from(*i)),
        FfiValue::Float(f) => {
            Json::Number(serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)))
        }
        FfiValue::Str(s) => Json::String(s.clone()),
        FfiValue::List(items) => Json::Array(items.iter().map(ffi_value_to_json).collect()),
        FfiValue::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, v) in entries {
                map.insert(k.clone(), ffi_value_to_json(v));
            }
            Json::Object(map)
        }
        FfiValue::Function => Json::Null,
    }
}

fn json_to_ffi_value(value: &Json) -> FfiValue {
    match value {
        Json::Null => FfiValue::Nil,
        Json::Bool(b) => FfiValue::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                FfiValue::Int(i)
            } else {
                FfiValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Json::String(s) => FfiValue::Str(s.clone()),
        Json::Array(items) => FfiValue::List(items.iter().map(json_to_ffi_value).collect()),
        Json::Object(map) => {
            let mut entries = std::collections::BTreeMap::new();
            for (k, v) in map {
                entries.insert(k.clone(), json_to_ffi_value(v));
            }
            FfiValue::Map(entries)
        }
    }
}

fn parse_args_json(c_str: *const std::ffi::c_char) -> Result<Vec<FfiValue>, String> {
    let json = parse_json(c_str)?;
    match json {
        Json::Array(items) => Ok(items.iter().map(json_to_ffi_value).collect()),
        Json::Null => Ok(Vec::new()),
        _ => Err("args_json must be a JSON array or null".to_string()),
    }
}

fn set_error(out_error: *mut *mut std::ffi::c_char, msg: &str) {
    if out_error.is_null() {
        return;
    }
    let c_msg = CString::new(msg).unwrap_or_default();
    unsafe { *out_error = c_msg.into_raw() };
}

/// Reads an optional root path from a C string.
///
/// A NULL pointer, or an empty string, means "use the configured default root";
/// anything else must be valid UTF-8 or it is reported as an error rather than
/// silently discarded.
fn root_arg(root: *const std::ffi::c_char) -> Result<Option<std::path::PathBuf>, String> {
    if root.is_null() {
        return Ok(None);
    }
    let s = unsafe { CStr::from_ptr(root) }
        .to_str()
        .map_err(|e| format!("plugin root is not valid UTF-8: {e}"))?;
    if s.is_empty() {
        return Ok(None);
    }
    Ok(Some(std::path::PathBuf::from(s)))
}

// ---- built-in capability provider for "log" ----------------------------

struct LogProvider;
impl stanchion_ffi::CapabilityProvider for LogProvider {
    fn invoke(&self, call: &stanchion_ffi::CapabilityCall) -> std::result::Result<FfiValue, String> {
        let message = call
            .args
            .first()
            .and_then(|v| {
                if let FfiValue::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("(no message)");
        eprintln!("[{}] {message}", call.plugin);
        Ok(FfiValue::Nil)
    }
}
// ---- exported C functions ------------------------------------------------

/// Initializes a stanchion registry from a JSON config string.
///
/// # Safety
///
/// `config_json` must be null or point to a NUL-terminated UTF-8 string, and
/// `out_error` must be null or a valid pointer to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_init(
    config_json: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut Stanchion {
    let json = match parse_json(config_json) {
        Ok(v) => v,
        Err(msg) => {
            set_error(out_error, &msg);
            return std::ptr::null_mut();
        }
    };

    let obj = match json.as_object() {
        Some(o) => o,
        None => {
            set_error(out_error, "config must be a JSON object");
            return std::ptr::null_mut();
        }
    };

    let mut host_config = HostConfig::default();

    if let Some(plugins) = obj.get("plugins").and_then(Json::as_str) {
        host_config.plugins = Some(std::path::PathBuf::from(plugins));
    }
    if let Some(shared) = obj.get("shared").and_then(Json::as_bool) {
        host_config.sandbox.shared = shared;
    }
    if let Some(libs) = obj.get("libs").and_then(Json::as_array) {
        host_config.sandbox.libs = Some(
            libs.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        );
    }
    if let Some(deny) = obj.get("deny").and_then(Json::as_array) {
        host_config.sandbox.deny = Some(
            deny.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        );
    }
    if let Some(bytes) = obj.get("memory_limit").and_then(Json::as_u64) {
        host_config.sandbox.memory_limit = (bytes > 0).then_some(bytes as usize);
    }
    if let Some(instructions) = obj.get("instruction_limit").and_then(Json::as_u64) {
        host_config.sandbox.instruction_limit = (instructions > 0).then_some(instructions);
    }
    if let Some(allow) = obj.get("allow").and_then(Json::as_array) {
        host_config.capabilities.allow = allow
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    if let Some(required) = obj.get("require_signatures").and_then(Json::as_bool) {
        host_config.signatures.required = required;
    }

    if let Some(caps) = obj.get("capabilities").and_then(Json::as_array) {
        for cap in caps {
            if let Some(name) = cap.get("name").and_then(Json::as_str)
                && !host_config.capabilities.callbacks.contains(&name.to_string())
            {
                host_config.capabilities.callbacks.push(name.to_string());
            }
        }
    }

    let mut builder = Builder::new().config(host_config);
    builder = builder.capability("log", Arc::new(LogProvider));

    match builder.build() {
        Ok(stanchion) => Box::into_raw(Box::new(stanchion)),
        Err(err) => {
            set_error(out_error, &err.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Destroys a stanchion instance.
///
/// # Safety
///
/// `s` must be a handle returned by `stanchion_init` that has not already
/// been destroyed, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_destroy(s: *mut Stanchion) {
    if !s.is_null() {
        unsafe { drop(Box::from_raw(s)) };
    }
}

/// Loads plugins from a directory. Returns JSON `{"loaded":[],"failures":[]}`.
///
/// # Safety
///
/// `s` must be a valid handle; `root` must be null or a NUL-terminated UTF-8
/// string; `out_error` must be null or a valid pointer to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_load(
    s: *mut Stanchion,
    root: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut std::ffi::c_char {
    let stanchion = unsafe { &*s };
    let root_path = match root_arg(root) {
        Ok(path) => path,
        Err(msg) => {
            set_error(out_error, &msg);
            return std::ptr::null_mut();
        }
    };

    match stanchion.load(root_path.as_deref()) {
        Ok(report) => {
            let mut map = serde_json::Map::new();
            map.insert(
                "loaded".to_string(),
                Json::Array(report.loaded.iter().map(|n| Json::String(n.clone())).collect()),
            );
            map.insert(
                "failures".to_string(),
                Json::Array(
                    report
                        .failures
                        .iter()
                        .map(|f| {
                            let mut fm = serde_json::Map::new();
                            fm.insert("plugin".to_string(), Json::String(f.plugin.clone()));
                            fm.insert("reason".to_string(), Json::String(f.reason.clone()));
                            Json::Object(fm)
                        })
                        .collect(),
                ),
            );
            to_c_string(&Json::Object(map))
        }
        Err(err) => {
            set_error(out_error, &err.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Returns a JSON array of loaded plugins.
///
/// # Safety
///
/// `s` must be a valid handle; `out_error` must be null or a valid pointer
/// to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_list(
    s: *mut Stanchion,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut std::ffi::c_char {
    let stanchion = unsafe { &*s };
    match stanchion.list() {
        Ok(plugins) => {
            let list: Vec<Json> = plugins
                .iter()
                .map(|p| {
                    let mut map = serde_json::Map::new();
                    map.insert("name".to_string(), Json::String(p.name.clone()));
                    map.insert(
                        "version".to_string(),
                        match &p.version {
                            Some(v) => Json::String(v.clone()),
                            None => Json::Null,
                        },
                    );
                    map.insert(
                        "granted".to_string(),
                        Json::Array(p.granted.iter().map(|g| Json::String(g.clone())).collect()),
                    );
                    map.insert("signer".to_string(), Json::String(p.signer.clone()));
                    map.insert("runtime".to_string(), Json::String(p.runtime.clone()));
                    Json::Object(map)
                })
                .collect();
            to_c_string(&Json::Array(list))
        }
        Err(err) => {
            set_error(out_error, &err.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Reports what plugins under `root` would request, without running them.
///
/// # Safety
///
/// `s` must be a valid handle; `root` must be null or a NUL-terminated UTF-8
/// string; `out_error` must be null or a valid pointer to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_audit(
    s: *mut Stanchion,
    root: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut std::ffi::c_char {
    let stanchion = unsafe { &*s };
    let root_path = match root_arg(root) {
        Ok(path) => path,
        Err(msg) => {
            set_error(out_error, &msg);
            return std::ptr::null_mut();
        }
    };

    match stanchion.audit(root_path.as_deref()) {
        Ok(entries) => {
            let list: Vec<Json> = entries
                .iter()
                .map(|entry| {
                    let mut map = serde_json::Map::new();
                    map.insert("plugin".to_string(), Json::String(entry.plugin.clone()));
                    map.insert(
                        "capabilities".to_string(),
                        Json::Array(
                            entry
                                .capabilities
                                .iter()
                                .map(|c| Json::String(c.clone()))
                                .collect(),
                        ),
                    );
                    map.insert("signer".to_string(), Json::String(entry.signer.clone()));
                    Json::Object(map)
                })
                .collect();
            to_c_string(&Json::Array(list))
        }
        Err(err) => {
            set_error(out_error, &err.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Calls one method on one plugin. Returns a JSON value, or NULL on error.
///
/// # Safety
///
/// `s` must be a valid handle; `plugin`, `method` and `args_json` must be null
/// or NUL-terminated UTF-8 strings; `out_error` must be null or a valid pointer
/// to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_call(
    s: *mut Stanchion,
    plugin: *const std::ffi::c_char,
    method: *const std::ffi::c_char,
    args_json: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut std::ffi::c_char {
    let stanchion = unsafe { &*s };
    let plugin = match unsafe { CStr::from_ptr(plugin) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("plugin name not UTF-8: {e}")); return std::ptr::null_mut(); }
    };
    let method = match unsafe { CStr::from_ptr(method) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("method name not UTF-8: {e}")); return std::ptr::null_mut(); }
    };
    let args = match parse_args_json(args_json) {
        Ok(a) => a,
        Err(msg) => { set_error(out_error, &msg); return std::ptr::null_mut(); }
    };

    match stanchion.call(plugin, method, &args) {
        Ok(value) => to_c_string(&ffi_value_to_json(&value)),
        Err(err) => { set_error(out_error, &err.to_string()); std::ptr::null_mut() }
    }
}

/// Calls a method on every loaded plugin. Returns JSON array of outcomes.
///
/// # Safety
///
/// `s` must be a valid handle; `method` and `args_json` must be null or
/// NUL-terminated UTF-8 strings; `out_error` must be null or a valid pointer
/// to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_dispatch(
    s: *mut Stanchion,
    method: *const std::ffi::c_char,
    args_json: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> *mut std::ffi::c_char {
    let stanchion = unsafe { &*s };
    let method = match unsafe { CStr::from_ptr(method) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("method name not UTF-8: {e}")); return std::ptr::null_mut(); }
    };
    let args = match parse_args_json(args_json) {
        Ok(a) => a,
        Err(msg) => { set_error(out_error, &msg); return std::ptr::null_mut(); }
    };

    match stanchion.dispatch(method, &args) {
        Ok(outcomes) => {
            let list: Vec<Json> = outcomes.into_iter().map(|o| {
                let mut map = serde_json::Map::new();
                map.insert("plugin".to_string(), Json::String(o.plugin));
                if let Some(value) = o.value {
                    map.insert("value".to_string(), ffi_value_to_json(&value));
                }
                if let Some(error) = o.error {
                    map.insert("error".to_string(), Json::String(error));
                }
                Json::Object(map)
            }).collect();
            to_c_string(&Json::Array(list))
        }
        Err(err) => { set_error(out_error, &err.to_string()); std::ptr::null_mut() }
    }
}

/// Reloads one plugin from disk.
///
/// # Safety
///
/// `s` must be a valid handle; `plugin` must be a NUL-terminated UTF-8 string;
/// `out_error` must be null or a valid pointer to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_reload(
    s: *mut Stanchion,
    plugin: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> i32 {
    let stanchion = unsafe { &*s };
    let plugin = match unsafe { CStr::from_ptr(plugin) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("plugin name not UTF-8: {e}")); return STANCHION_ERR_CONFIG; }
    };
    match stanchion.reload(plugin) {
        Ok(()) => STANCHION_OK,
        Err(err) => { set_error(out_error, &err.to_string()); to_ffi_error_code(&err) }
    }
}

/// Revokes a granted capability from a loaded plugin.
///
/// # Safety
///
/// `s` must be a valid handle; `plugin` and `capability` must be NUL-terminated
/// UTF-8 strings; `out_error` must be null or a valid pointer to a `char*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_revoke(
    s: *mut Stanchion,
    plugin: *const std::ffi::c_char,
    capability: *const std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> i32 {
    let stanchion = unsafe { &*s };
    let plugin = match unsafe { CStr::from_ptr(plugin) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("plugin name not UTF-8: {e}")); return STANCHION_ERR_CONFIG; }
    };
    let cap = match unsafe { CStr::from_ptr(capability) }.to_str() {
        Ok(s) => s,
        Err(e) => { set_error(out_error, &format!("capability name not UTF-8: {e}")); return STANCHION_ERR_CONFIG; }
    };
    match stanchion.revoke(plugin, cap) {
        Ok(true) => STANCHION_OK,
        Ok(false) => { set_error(out_error, &format!("`{plugin}` does not hold `{cap}`")); STANCHION_ERR_UNKNOWN_PLUGIN }
        Err(err) => { set_error(out_error, &err.to_string()); to_ffi_error_code(&err) }
    }
}

/// Frees a string allocated by any stanchion function.
///
/// # Safety
///
/// `s` must be null, or a pointer returned by a stanchion function that has
/// not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stanchion_string_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)); }
    }
}

/// Returns a human-readable description of an error code.
///
/// The returned pointer is a static string literal; it must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn stanchion_error_string(code: i32) -> *const std::ffi::c_char {
    let message: &'static std::ffi::CStr = match code {
        STANCHION_OK => c"ok",
        STANCHION_ERR_UNKNOWN_PLUGIN => c"unknown plugin",
        STANCHION_ERR_PLUGIN => c"plugin error",
        STANCHION_ERR_LUA => c"Lua error",
        STANCHION_ERR_IO => c"I/O error",
        STANCHION_ERR_CONFIG => c"configuration error",
        STANCHION_ERR_CAPABILITY => c"capability error",
        STANCHION_ERR_REENTRANT => c"reentrant call detected (would deadlock)",
        STANCHION_ERR_WASM => c"WASM error",
        _ => c"unknown error code",
    };
    message.as_ptr()
}

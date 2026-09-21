use std::collections::HashMap;
use wasmtime::{Engine, Module, Store, Instance, Val};
use stanchion_ffi::Value;

/// A loaded WASM runtime for executing WASM plugin exports.
pub struct WasmRuntime {
    engine: Engine,
    store: Store<()>,
    instance: Instance,
    exports: HashMap<String, wasmtime::Func>,
}

impl WasmRuntime {
    /// Creates a new WASM runtime from a compiled binary.
    pub fn new(wasm_binary: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::from_binary(&engine, wasm_binary)
            .map_err(|e| format!("Failed to load WASM module: {}", e))?;
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| format!("Failed to instantiate WASM module: {}", e))?;

        let mut exports = HashMap::new();
        for export in instance.exports(&mut store) {
            if let Some(func) = export.into_func() {
                exports.insert(export.name().to_string(), func);
            }
        }

        Ok(Self { engine, store, instance, exports })
    }

    /// Calls an exported WASM function with the given arguments.
    pub fn call(&mut self, func: &str, args: &[Value]) -> Result<Value, String> {
        let func = self.exports.get(func)
            .ok_or_else(|| format!("Export '{}' not found", func))?;

        let wasm_args = values_to_wasm(func, args)?;

        let mut results = vec![Val::I32(0); func.result_types(&self.store).len()];
        func.call(&mut self.store, &wasm_args, &mut results)
            .map_err(|e| format!("WASM call failed: {}", e))?;

        wasm_to_value(&results)
    }

    /// Returns the names of all exported functions.
    pub fn exports(&self) -> impl Iterator<Item = &str> {
        self.exports.keys().map(String::as_str)
    }

    /// Checks whether a named export exists.
    pub fn has_export(&self, name: &str) -> bool {
        self.exports.contains_key(name)
    }
}

/// Converts stanchion values to WASM values.
fn values_to_wasm(func: &wasmtime::Func, args: &[Value]) -> Result<Vec<Val>, String> {
    let param_types = func.param_types(&mut std::cell::RefCell::new(()));
    if args.len() != param_types.len() {
        return Err(format!(
            "Expected {} arguments, got {}",
            param_types.len(),
            args.len()
        ));
    }

    args.iter()
        .zip(param_types)
        .map(|(arg, ty)| match (arg, ty) {
            (Value::Integer(n), ValType::I32) => Ok(Val::I32(*n as i32)),
            (Value::Integer(n), ValType::I64) => Ok(Val::I64(*n as i64)),
            (Value::Number(f), ValType::F32) => Ok(Val::F32(*f as f32)),
            (Value::Number(f), ValType::F64) => Ok(Val::F64(*f as f64)),
            (Value::Str(s), ValType::I32) | (Value::Str(s), ValType::I64) => {
                // A string pointer passed as an i32/i64 offset into linear memory.
                // Real implementations would need to write into WASM memory.
                Err(format!(
                    "String arguments require memory access, not raw i32/i64: '{}'",
                    s
                ))
            }
            (Value::Bool(b), _) => Err(format!(
                "Boolean argument passed to WASM export expecting {:?}",
                ty
            )),
            (Value::Nil, _) => Err("Nil argument passed to WASM export".to_string()),
            (Value::Bytes(_), _) => Err("Bytes argument passed to WASM export".to_string()),
            (Value::Table(_), _) => Err("Table argument passed to WASM export".to_string()),
            (arg, ty) => Err(format!(
                "Unsupported argument type {:?} for WASM type {:?}",
                arg, ty
            )),
        })
        .collect()
}

/// Converts WASM return values to stanchion values.
fn wasm_to_value(results: &[Val]) -> Result<Value, String> {
    match results {
        [] => Ok(Value::Nil),
        [Val::I32(n)] => Ok(Value::Integer(*n as i64)),
        [Val::I64(n)] => Ok(Value::Integer(*n)),
        [Val::F32(n)] => Ok(Value::Number(*n as f64)),
        [Val::F64(n)] => Ok(Value::Number(*n)),
        other => Err(format!(
            "Unsupported WASM return types: {:?}",
            other.iter().map(|v| format!("{:?}", v)).collect::<Vec<_>>()
        )),
    }
}

/// A handle to a loaded WASM plugin instance, analogous to DynInstance.
pub struct WasmInstance {
    runtime: WasmRuntime,
    entry_point: String,
}

impl WasmInstance {
    pub fn new(runtime: WasmRuntime, entry_point: String) -> Self {
        Self { runtime, entry_point }
    }

    /// Calls the plugin's entry-point export with the given arguments.
    pub fn call(&mut self, args: &[Value]) -> Result<Value, String> {
        self.runtime.call(&self.entry_point, args)
    }
}
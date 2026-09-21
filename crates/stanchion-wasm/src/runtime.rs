use std::collections::HashMap;
use wasmtime::{Engine, Module, Store, Instance, Func, Val, ValType};
use stanchion_ffi::Value;

/// A loaded WASM runtime for executing WASM plugin exports.
pub struct WasmRuntime {
    engine: Engine,
    store: Store<()>,
    instance: Instance,
    exports: HashMap<String, Func>,
}

impl WasmRuntime {
    /// Creates a new WASM runtime from a compiled binary.
    pub fn new(wasm_binary: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_binary)
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

        // Determine expected param and result types from the function's signature.
        let param_types: Vec<ValType> = func.params().collect();
        let result_types: Vec<ValType> = func.results().collect();

        if args.len() != param_types.len() {
            return Err(format!(
                "Expected {} arguments, got {}",
                param_types.len(),
                args.len()
            ));
        }

        let wasm_args = values_to_wasm(args, &param_types)?;

        let mut results = vec![Val::I32(0); result_types.len()];
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

/// Converts stanchion values to WASM values using the function's parameter types.
fn values_to_wasm(args: &[Value], param_types: &[ValType]) -> Result<Vec<Val>, String> {
    args.iter()
        .zip(param_types)
        .map(|(arg, ty)| match (arg, ty) {
            (Value::Int(n), ValType::I32) => Ok(Val::I32(*n as i32)),
            (Value::Int(n), ValType::I64) => Ok(Val::I64(*n)),
            (Value::Float(f), ValType::F32) => Ok(Val::F32(*f as f32)),
            (Value::Float(f), ValType::F64) => Ok(Val::F64(*f)),
            (Value::Str(s), ValType::I32) | (Value::Str(s), ValType::I64) => {
                // String pointers passed as integers require writing into WASM memory
                // and are not yet supported in this prototype.
                Err(format!(
                    "String arguments require linear memory access, not raw i32/i64: '{}'",
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
        [Val::I32(n)] => Ok(Value::Int(*n as i64)),
        [Val::I64(n)] => Ok(Value::Int(*n)),
        [Val::F32(n)] => Ok(Value::Float(*n as f64)),
        [Val::F64(n)] => Ok(Value::Float(*n)),
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
use stanchion_abi::Value;
use std::collections::HashMap;
use wasmtime::{
    Config, Engine, Func, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, Val, ValType,
};

/// Resource ceilings a WASM plugin runs under.
///
/// Without these a plugin can loop forever (hanging the calling thread) or
/// `memory.grow` up to 4 GiB. They are set by the host, never by the plugin — a plugin
/// manifest may only *lower* the fuel ceiling (see `WasmBackend`), never raise it.
#[derive(Debug, Clone, Copy)]
pub struct WasmLimits {
    /// Fuel (roughly, executed WASM instructions) allowed per call and for the
    /// module's `start`. `None` disables the fuel meter entirely.
    pub max_fuel: Option<u64>,
    /// Ceiling on the plugin's linear memory, in bytes. `None` leaves wasmtime's
    /// default (up to 4 GiB per memory).
    pub memory_limit: Option<usize>,
}

impl Default for WasmLimits {
    fn default() -> Self {
        // Secure-by-default: a bounded amount of work and 64 MiB of memory, matching
        // the Lua sandbox defaults, so a backend registered with `WasmBackend::new()`
        // is not a DoS hole.
        WasmLimits {
            max_fuel: Some(1_000_000_000),
            memory_limit: Some(64 * 1024 * 1024),
        }
    }
}

/// Per-store data holding the resource limiter wasmtime enforces.
struct StoreData {
    limits: StoreLimits,
}

/// A loaded WASM runtime for executing WASM plugin exports.
#[allow(dead_code)]
pub struct WasmRuntime {
    engine: Engine,
    store: Store<StoreData>,
    instance: Instance,
    exports: HashMap<String, Func>,
    /// Refuelled before each call so the ceiling is per call, not for the lifetime.
    max_fuel: Option<u64>,
}

impl WasmRuntime {
    /// Creates a new WASM runtime from a compiled binary, under `limits`.
    pub fn new(wasm_binary: &[u8], limits: WasmLimits) -> Result<Self, String> {
        let mut config = Config::new();
        if limits.max_fuel.is_some() {
            config.consume_fuel(true);
        }
        let engine =
            Engine::new(&config).map_err(|e| format!("Failed to configure WASM engine: {e}"))?;
        let module = Module::new(&engine, wasm_binary)
            .map_err(|e| format!("Failed to load WASM module: {}", e))?;

        let mut store_limits = StoreLimitsBuilder::new();
        if let Some(bytes) = limits.memory_limit {
            store_limits = store_limits.memory_size(bytes);
        }
        let mut store = Store::new(
            &engine,
            StoreData {
                limits: store_limits.build(),
            },
        );
        store.limiter(|data| &mut data.limits);
        // Bound the module's `start` too: it runs during instantiation and could
        // otherwise loop forever.
        if let Some(fuel) = limits.max_fuel {
            store
                .set_fuel(fuel)
                .map_err(|e| format!("Failed to set WASM fuel: {e}"))?;
        }
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| format!("Failed to instantiate WASM module: {:#}", e))?;

        let mut exports = HashMap::new();
        for export in instance.exports(&mut store) {
            let name = export.name().to_string();
            if let Some(func) = export.into_func() {
                exports.insert(name, func);
            }
        }

        Ok(Self {
            engine,
            store,
            instance,
            exports,
            max_fuel: limits.max_fuel,
        })
    }

    /// Calls an exported WASM function with the given arguments.
    pub fn call(&mut self, func: &str, args: &[Value]) -> Result<Value, String> {
        let wasm_func = *self
            .exports
            .get(func)
            .ok_or_else(|| format!("Export '{}' not found", func))?;

        // Refuel so each call gets the full instruction ceiling; an infinite loop then
        // traps ("all fuel consumed") instead of hanging the calling thread.
        if let Some(fuel) = self.max_fuel {
            self.store
                .set_fuel(fuel)
                .map_err(|e| format!("Failed to reset WASM fuel: {e}"))?;
        }

        // Determine expected param and result types from the function's signature.
        let ty = wasm_func.ty(&self.store);
        let param_types: Vec<ValType> = ty.params().collect();
        let result_types: Vec<ValType> = ty.results().collect();

        if args.len() != param_types.len() {
            return Err(format!(
                "Expected {} arguments, got {}",
                param_types.len(),
                args.len()
            ));
        }

        let wasm_args = values_to_wasm(args, &param_types)?;

        let mut results = vec![Val::I32(0); result_types.len()];
        wasm_func
            .call(&mut self.store, &wasm_args, &mut results)
            .map_err(|e| format!("WASM call failed: {:#}", e))?;

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
///
/// Narrowing conversions are checked and fail closed: an `Int` outside `i32`
/// range or a finite `Float` outside `f32` range is rejected rather than
/// silently truncated or saturated to infinity.
fn values_to_wasm(args: &[Value], param_types: &[ValType]) -> Result<Vec<Val>, String> {
    args.iter()
        .zip(param_types)
        .map(|(arg, ty)| match (arg, ty) {
            (Value::Int(n), ValType::I32) => i32::try_from(*n)
                .map(Val::I32)
                .map_err(|_| format!("value {n} does not fit in an i32")),
            (Value::Int(n), ValType::I64) => Ok(Val::I64(*n)),
            (Value::Float(f), ValType::F32) => {
                if f.is_finite() && (*f > f32::MAX as f64 || *f < f32::MIN as f64) {
                    return Err(format!("value {f} does not fit in an f32"));
                }
                Ok(Val::F32((*f as f32).to_bits()))
            }
            (Value::Float(f), ValType::F64) => Ok(Val::F64((*f).to_bits())),
            (Value::Str(s), ValType::I32) | (Value::Str(s), ValType::I64) => {
                // String pointers passed as integers require writing into WASM memory
                // and are not yet supported in this prototype.
                Err(format!(
                    "String arguments require linear memory access, not raw i32/i64: '{}'",
                    s
                ))
            }
            (Value::Bool(_), _) => Err(format!(
                "Boolean argument passed to WASM export expecting {:?}",
                ty
            )),
            (Value::Nil, _) => Err("Nil argument passed to WASM export".to_string()),
            (Value::List(_), _) => Err("List argument passed to WASM export".to_string()),
            (Value::Map(_), _) => Err("Map argument passed to WASM export".to_string()),
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
        [Val::F32(n)] => Ok(Value::Float(f32::from_bits(*n) as f64)),
        [Val::F64(n)] => Ok(Value::Float(f64::from_bits(*n))),
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
        Self {
            runtime,
            entry_point,
        }
    }

    /// Calls the plugin's entry-point export with the given arguments.
    pub fn call(&mut self, args: &[Value]) -> Result<Value, String> {
        self.runtime.call(&self.entry_point, args)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use stanchion_abi::Value;

    fn i32_ok(n: i64) {
        let val = Value::Int(n);
        let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::I32]);
        assert!(res.is_ok(), "expected {n} to fit in i32, got {:?}", res.err());
        let vals = res.unwrap();
        assert_eq!(vals.len(), 1, "expected one val");
        match vals.first().unwrap() {
            Val::I32(v) => assert_eq!(*v, n as i32),
            other => panic!("expected I32, got {other:?}"),
        }
        // round-trip via wasm_to_value
        let back = wasm_to_value(&vals).unwrap();
        assert_eq!(back, Value::Int(n));
    }

    fn i32_err(n: i64) {
        let val = Value::Int(n);
        let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::I32]);
        assert!(res.is_err(), "expected {n} to be rejected for i32");
        let msg = res.unwrap_err();
        assert!(msg.contains("does not fit in an i32"), "msg: {msg}");
    }

    fn f32_ok(f: f64) {
        let val = Value::Float(f);
        let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::F32]);
        assert!(res.is_ok(), "expected {f} to fit in f32, got {:?}", res.err());
    }

    fn f32_err(f: f64) {
        let val = Value::Float(f);
        let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::F32]);
        assert!(res.is_err(), "expected {f} to be rejected for f32");
        let msg = res.unwrap_err();
        assert!(msg.contains("does not fit in an f32"), "msg: {msg}");
    }

    #[test]
    fn int_to_i32_boundaries() {
        i32_ok(0);
        i32_ok(1);
        i32_ok(-1);
        i32_ok(i32::MAX as i64);
        i32_ok(i32::MIN as i64);
        // issue table
        i32_err(0x1_0000_0000); // 4294967296 -> 0 if truncated
        i32_err(0x1_0000_0001);
        i32_err(0xFFFF_FFFF); // 4294967295 -> -1 if truncated
        i32_err(0x1_FFFF_FFFF);
        i32_err(i32::MAX as i64 + 1);
        i32_err(i32::MIN as i64 - 1);
        i32_err(u32::MAX as i64);
    }

    #[test]
    fn int_to_i64_always_ok() {
        for n in [i64::MAX, i64::MIN, 0x1_0000_0000, 0xFFFF_FFFF] {
            let val = Value::Int(n);
            let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::I64]);
            assert!(res.is_ok(), "i64 should accept {n}");
        }
    }

    #[test]
    fn float_to_f32_boundaries() {
        f32_ok(0.0);
        f32_ok(1.0);
        f32_ok(-1.0);
        f32_ok(f32::MAX as f64);
        f32_ok(f32::MIN as f64);
        f32_ok(0.1); // precision loss but in range -> allowed
        f32_ok(f64::INFINITY);
        f32_ok(f64::NEG_INFINITY);
        f32_ok(f64::NAN);
        // finite out of range -> rejected
        f32_err(f32::MAX as f64 * 2.0);
        f32_err(f32::MIN as f64 * 2.0);
        f32_err(1e40);
        f32_err(-1e40);
    }

    #[test]
    fn float_to_f64_always_ok() {
        for f in [0.0, 1e40, f64::MAX, f64::MIN, f64::INFINITY, f64::NAN] {
            let val = Value::Float(f);
            let res = values_to_wasm(std::slice::from_ref(&val), &[ValType::F64]);
            assert!(res.is_ok(), "f64 should accept {f}");
        }
    }

    #[test]
    fn wasm_runtime_rejects_truncated_int() {
        // Minimal WAT: (module (func (export "id") (param i32) (result i32) local.get 0))
        let wat = br#"(module (func (export "id") (param i32) (result i32) local.get 0))"#;
        let mut rt = WasmRuntime::new(wat, WasmLimits::default()).expect("wat");
        // in-range succeeds and round-trips
        let ok = rt.call("id", &[Value::Int(42)]).expect("call ok");
        assert_eq!(ok, Value::Int(42));
        // out-of-range is rejected before the call
        let err = rt.call("id", &[Value::Int(0x1_0000_0000)]).expect_err("should reject");
        assert!(err.contains("does not fit in an i32"), "err: {err}");
    }

    #[test]
    fn an_infinite_loop_traps_on_fuel_instead_of_hanging() {
        // Without a fuel meter this call never returns; with one it must trap.
        let wat = br#"(module (func (export "spin") (loop br 0)))"#;
        let limits = WasmLimits {
            max_fuel: Some(1_000_000),
            memory_limit: Some(1024 * 1024),
        };
        let mut rt = WasmRuntime::new(wat, limits).expect("wat");
        let err = rt.call("spin", &[]).expect_err("infinite loop must trap");
        assert!(
            err.to_lowercase().contains("fuel"),
            "expected a fuel-exhaustion trap, got: {err}"
        );
    }

    #[test]
    fn a_module_start_that_loops_is_bounded() {
        // A `start` function that loops forever must not hang instantiation.
        let wat = br#"(module (func $s (loop br 0)) (start $s))"#;
        let limits = WasmLimits {
            max_fuel: Some(500_000),
            memory_limit: Some(1024 * 1024),
        };
        let err = match WasmRuntime::new(wat, limits) {
            Ok(_) => panic!("a looping start function must trap"),
            Err(err) => err,
        };
        assert!(
            err.to_lowercase().contains("fuel"),
            "expected fuel exhaustion during start, got: {err}"
        );
    }

    #[test]
    fn memory_growth_is_capped() {
        // memory.grow past the store limit fails (returns -1) rather than reaching 4 GiB.
        let wat = br#"(module
            (memory 1)
            (func (export "grow") (result i32) (memory.grow (i32.const 1000))))"#;
        let limits = WasmLimits {
            max_fuel: Some(1_000_000),
            memory_limit: Some(2 * 64 * 1024), // 2 pages
        };
        let mut rt = WasmRuntime::new(wat, limits).expect("wat");
        let result = rt.call("grow", &[]).expect("grow call returns");
        assert_eq!(
            result,
            Value::Int(-1),
            "memory.grow past the limit must fail rather than succeed"
        );
    }
}


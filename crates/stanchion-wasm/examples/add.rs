//! The WASM boundary is numeric and bounded: scalars cross, loops trap.
//!
//! Only `Int`/`Float` map onto WASM params and results today — strings, bools,
//! lists and maps are refused rather than silently flattened. Every call runs
//! under the host's fuel ceiling, so an infinite loop traps instead of hanging
//! the thread.
//!
//! Modules are written inline as WAT; wasmtime accepts the text form directly.
//!
//! ```sh
//! cargo run -p stanchion-wasm --example add
//! ```

use std::io;

use stanchion_abi::Value;
use stanchion_wasm::{WasmLimits, WasmRuntime};

const ADD: &[u8] =
    br#"(module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))"#;

const SPIN: &[u8] = br#"(module (func (export "spin") (loop br 0)))"#;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut add = WasmRuntime::new(ADD, WasmLimits::default()).map_err(io::Error::other)?;
    match add
        .call("add", &[Value::Int(2), Value::Int(3)])
        .map_err(io::Error::other)?
    {
        Value::Int(sum) if sum == 5 => println!("add(2, 3) = {sum}"),
        other => {
            return Err(io::Error::other(format!("unexpected result: {other:?}")).into());
        }
    }

    // A string is not a number: rejected before the function runs.
    match add.call("add", &[Value::Str("2".to_string()), Value::Int(3)]) {
        Ok(value) => {
            return Err(io::Error::other(format!("unexpectedly accepted: {value:?}")).into());
        }
        Err(err) => println!("refused Str: {}", first_line(&err)),
    }

    // An integer that does not fit the target is rejected, not truncated.
    match add.call("add", &[Value::Int(0x1_0000_0000), Value::Int(0)]) {
        Ok(value) => {
            return Err(io::Error::other(format!("unexpectedly accepted: {value:?}")).into());
        }
        Err(err) => println!("refused out-of-range Int: {}", first_line(&err)),
    }

    // Fuel bounds the call: the loop traps, and this process keeps running.
    let mut spinning = WasmRuntime::new(
        SPIN,
        WasmLimits {
            max_fuel: Some(1_000),
            memory_limit: Some(64 * 1024 * 1024),
        },
    )
    .map_err(io::Error::other)?;
    match spinning.call("spin", &[]) {
        Ok(_) => println!("unexpected: the loop was supposed to trap"),
        Err(err) => println!("trapped: {}", first_line(&err)),
    }
    println!("host alive: still running");

    Ok(())
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

//! Per-plugin Lua state policy: standard libraries, memory and instruction limits.
//!
//! Note that mlua's `StdLib::ALL_SAFE` means *memory*-safe, not sandboxed — it still
//! includes `io` and `os`, so a plugin could read files or spawn processes. The
//! defaults here are deliberately tighter; see [`Sandbox::restricted`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{Lua, LuaOptions, StdLib, Table, Value};

/// Globals removed by [`Sandbox::restricted`].
///
/// `package` is loaded so `require` works for rocks and plugin-local modules, but
/// `package.loadlib` would let a plugin load an arbitrary shared object, and
/// `dofile`/`loadfile` reach the filesystem regardless of which libraries are loaded.
///
/// `string.dump` serialises a function to Lua bytecode. On its own that is harmless,
/// but paired with a `load` that accepts bytecode it is half of a VM-corruption
/// primitive, so it is removed here and [`Sandbox::restricted`] also forces `load` to
/// text-only mode (see [`harden_load`]).
pub const RESTRICTED_DENY_LIST: &[&str] =
    &["dofile", "loadfile", "package.loadlib", "string.dump"];

/// Libraries present in every supported Lua version, minus `io`, `os` and `debug`.
fn core_libs() -> StdLib {
    StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::PACKAGE | StdLib::COROUTINE
}

/// Remaining instruction allowance for one plugin state.
///
/// Shared with the VM hook, and reset by the registry at each call boundary so the
/// limit applies per call rather than for the lifetime of the plugin.
#[derive(Debug, Clone)]
pub struct Budget {
    used: Arc<AtomicU64>,
    limit: u64,
}

impl Budget {
    fn new(limit: u64) -> Self {
        Budget {
            used: Arc::new(AtomicU64::new(0)),
            limit,
        }
    }

    /// Clears the allowance. Called before each plugin call.
    pub fn reset(&self) {
        self.used.store(0, Ordering::Relaxed);
    }

    /// The configured allowance.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Instructions charged against the allowance since the last [`Budget::reset`].
    ///
    /// Counted per *state*, so under [`crate::Isolation::PerGroup`] every member of a
    /// dependency group reads the same number: the group is the accounting unit.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    fn consume(&self, amount: u64) -> mlua::Result<mlua::VmState> {
        let used = self
            .used
            .fetch_add(amount, Ordering::Relaxed)
            .saturating_add(amount);
        if used > self.limit {
            return Err(mlua::Error::RuntimeError(format!(
                "plugin exceeded its instruction limit of {}",
                self.limit
            )));
        }
        Ok(mlua::VmState::Continue)
    }
}

/// Policy for Lua states the registry creates for plugins.
#[derive(Debug, Clone)]
pub struct Sandbox {
    libs: StdLib,
    options: LuaOptions,
    memory_limit: Option<usize>,
    instruction_limit: Option<u64>,
    denied: Vec<String>,
    text_only_load: bool,
}

impl Default for Sandbox {
    fn default() -> Self {
        Sandbox::restricted()
    }
}

impl Sandbox {
    /// String, table, math, coroutine and package — no `io`, `os` or `debug` — with
    /// [`RESTRICTED_DENY_LIST`] removed on top.
    ///
    /// Leaves `pcall` and `xpcall` as Lua defines them, which means a plugin can catch
    /// a panic raised in one of your callbacks. See
    /// [`Sandbox::catch_rust_panics`] for why that is a choice rather than a detail.
    pub fn restricted() -> Self {
        Sandbox {
            libs: core_libs(),
            options: LuaOptions::new().catch_rust_panics(true),
            memory_limit: None,
            instruction_limit: None,
            denied: RESTRICTED_DENY_LIST
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            text_only_load: true,
        }
    }

    /// Adds `io` and `os`, and removes the deny list.
    ///
    /// Appropriate for first-party plugins that legitimately touch the filesystem;
    /// not for code you did not write. Note that `os.exit` is reachable here, and it
    /// ends the process without unwinding — nothing in-process can intercept it.
    pub fn permissive() -> Self {
        Sandbox {
            libs: core_libs() | StdLib::IO | StdLib::OS,
            options: LuaOptions::new().catch_rust_panics(true),
            memory_limit: None,
            instruction_limit: None,
            denied: Vec::new(),
            text_only_load: false,
        }
    }

    /// Whether `load` is confined to text chunks, rejecting Lua bytecode.
    ///
    /// Lua does not verify bytecode, so crafted bytecode passed to `load` is a route to
    /// VM-memory corruption and native code execution — an escape from every sandbox
    /// mode. [`Sandbox::restricted`] enables this (and denies `string.dump`);
    /// [`Sandbox::permissive`] does not, since it already grants `io`/`os`.
    pub fn text_only_load(mut self, enabled: bool) -> Self {
        self.text_only_load = enabled;
        self
    }

    /// Sets exactly which standard libraries plugin states load.
    pub fn libs(mut self, libs: StdLib) -> Self {
        self.libs = libs;
        self
    }

    /// Which standard libraries this sandbox loads.
    pub fn loaded_libs(&self) -> StdLib {
        self.libs
    }

    /// Removes globals by dotted path, e.g. `"os.execute"`, after the libraries load.
    pub fn deny(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.denied = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Caps the memory one plugin's state may allocate, enforced by the VM allocator.
    pub fn memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = Some(bytes);
        self
    }

    /// Caps how long one plugin call may run, in VM instructions.
    ///
    /// Exceeding it raises a Lua error, so a runaway loop surfaces as a failed call
    /// rather than a hung process. Under `luau` the limit counts interrupt callbacks
    /// rather than instructions, since Luau has no debug-hook instruction counter.
    pub fn instruction_limit(mut self, instructions: u64) -> Self {
        self.instruction_limit = Some(instructions);
        self
    }

    /// Whether *Lua* may catch a Rust panic raised inside one of your callbacks.
    ///
    /// The name reads backwards. A panic in a callback is never able to unwind through
    /// the VM: mlua wraps every callback in `catch_unwind` and carries the payload out
    /// as a Lua error object, resuming the panic once it reaches Rust again. What this
    /// option decides is whether Lua's own `pcall`/`xpcall` get to intercept that
    /// object on the way.
    ///
    /// - `true` (the default, and what both presets use): stock `pcall`/`xpcall`, so a
    ///   plugin wrapping a host call in `pcall` **swallows the panic** and carries on.
    /// - `false`: mlua substitutes handlers that rethrow a panic past the plugin's
    ///   handler, so it always reaches the host.
    ///
    /// For plugins you did not write, `false` is the defensible setting: a panic in
    /// your code is not a plugin's to discard. It is not the default here because
    /// changing it changes what already-working plugins observe.
    pub fn catch_rust_panics(mut self, enabled: bool) -> Self {
        self.options = self.options.catch_rust_panics(enabled);
        self
    }

    /// Creates a state under this policy, plus its instruction budget if configured.
    pub fn build(&self) -> mlua::Result<(Lua, Option<Budget>)> {
        let lua = Lua::new_with(self.libs, self.options.clone())?;
        if let Some(limit) = self.memory_limit {
            lua.set_memory_limit(limit)?;
        }
        for path in &self.denied {
            remove_global(&lua, path)?;
        }
        if self.text_only_load {
            harden_load(&lua)?;
        }
        let budget = self.install_limit(&lua)?;
        Ok((lua, budget))
    }

    #[cfg(not(feature = "luau"))]
    fn install_limit(&self, lua: &Lua) -> mlua::Result<Option<Budget>> {
        use mlua::debug::HookTriggers;

        let Some(limit) = self.instruction_limit else {
            return Ok(None);
        };
        // Check often enough to stop a tight loop promptly, without hooking every
        // single instruction.
        let step = limit.clamp(1, 1_000);
        let budget = Budget::new(limit);
        let tracked = budget.clone();

        let triggers = HookTriggers::new().every_nth_instruction(step.try_into().unwrap_or(1_000));
        lua.set_hook(triggers, move |_, _| tracked.consume(step))?;
        Ok(Some(budget))
    }

    #[cfg(feature = "luau")]
    fn install_limit(&self, lua: &Lua) -> mlua::Result<Option<Budget>> {
        let Some(limit) = self.instruction_limit else {
            return Ok(None);
        };
        let budget = Budget::new(limit);
        let tracked = budget.clone();
        // Luau has no instruction hook; its interrupt fires periodically instead, so
        // the budget counts ticks.
        lua.set_interrupt(move |_| tracked.consume(1));
        Ok(Some(budget))
    }
}

/// Replaces the global `load` with a wrapper that forces text-only mode.
///
/// mlua's safe mode still lets `load` accept a bytecode chunk. Since Lua does not
/// verify bytecode, that is a memory-safety escape (see [`RESTRICTED_DENY_LIST`]). The
/// wrapper preserves `load`'s `(chunk, chunkname, mode, env)` signature but ignores the
/// caller's `mode`, always passing `"t"`, so `load` returns `nil, err` on a binary
/// chunk exactly as a text-only `load` does. A missing `load` (library not loaded) is
/// left alone.
fn harden_load(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let Some(original) = globals.get::<Option<mlua::Function>>("load")? else {
        return Ok(());
    };
    let safe = lua.create_function(
        move |_, (chunk, chunkname, _mode, env): (Value, Value, Value, Value)| {
            let _ = _mode;
            original.call::<mlua::MultiValue>((chunk, chunkname, "t", env))
        },
    )?;
    globals.set("load", safe)?;
    Ok(())
}

/// Sets a dotted global path to nil, e.g. `package.loadlib`.
fn remove_global(lua: &Lua, path: &str) -> mlua::Result<()> {
    let mut segments = path.split('.').peekable();
    let mut table: Table = lua.globals();

    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            table.set(segment, Value::Nil)?;
            return Ok(());
        }
        // A library that was never loaded has nothing to remove.
        match table.get::<Value>(segment)? {
            Value::Table(next) => table = next,
            _ => return Ok(()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_load_rejects_bytecode() {
        let (lua, _) = Sandbox::restricted().build().expect("build restricted state");
        // Under a permissive-style state string.dump is available to produce bytecode;
        // build it there so the test does not depend on string.dump surviving.
        let (donor, _) = Sandbox::permissive().build().expect("build donor state");
        let bytecode: mlua::LuaString = donor
            .load("return string.dump(function() return 42 end)")
            .eval()
            .expect("dump a function to bytecode");
        let bytes = bytecode.as_bytes().to_vec();

        // load(<bytecode>) must fail (return nil, err) rather than accept the chunk.
        let loaded: Value = lua
            .load("return function(b) return (load(b)) end")
            .eval::<mlua::Function>()
            .expect("build a load probe")
            .call(lua.create_string(&bytes).expect("bytes into lua string"))
            .expect("calling load must not raise");
        assert!(
            matches!(loaded, Value::Nil),
            "load must reject bytecode under restricted(), got {loaded:?}"
        );
    }

    #[test]
    fn restricted_load_still_accepts_text() {
        let (lua, _) = Sandbox::restricted().build().expect("build restricted state");
        let result: i64 = lua
            .load(r#"return load("return 7")()"#)
            .eval()
            .expect("text chunks must still load");
        assert_eq!(result, 7);
    }

    #[test]
    fn restricted_denies_string_dump() {
        let (lua, _) = Sandbox::restricted().build().expect("build restricted state");
        let dump: Value = lua.globals().get::<Table>("string").unwrap().get("dump").unwrap();
        assert!(
            matches!(dump, Value::Nil),
            "string.dump must be removed under restricted()"
        );
    }
}

//! Ruby bindings for stanchion.
//!
//! A thin translation layer over `stanchion-ffi`: Ruby objects in, `Value`s out,
//! Ruby callables wrapped as capability providers and policies. Everything that
//! decides *behaviour* lives in `stanchion-ffi`, so this binding and the other four
//! cannot drift apart.
//!
//! # The GVL stays held
//!
//! A capability provider is a Ruby callable, and Ruby code may only run on a thread
//! holding the GVL. Releasing it around a plugin call would let other Ruby threads
//! run, but the callback would then arrive on a thread that must not touch the VM —
//! so the GVL is held for the whole call and **a plugin call blocks the Ruby VM**.
//!
//! That is the same trade the JavaScript binding makes, for the same reason, and the
//! instruction and memory limits in `docs/isolation.md` are what bound it. They are
//! on by default.

use std::sync::Arc;

use magnus::value::{Opaque, ReprValue};
use magnus::{
    Error, ExceptionClass, Module, Object, RArray, RHash, Ruby, Symbol, TryConvert,
    Value as Rb, function, method,
};

use stanchion_ffi::{
    CapabilityCall as FfiCall, CapabilityProvider, CapabilityRequest as FfiRequest,
    Decision as FfiDecision, Error as FfiError, HostConfig, Policy as FfiPolicy,
    Stanchion as Host, Value as FfiValue,
};

mod value;

use value::{from_ruby, to_ruby};

/// Maps a failure onto the exception class that names it.
fn raise(ruby: &Ruby, err: FfiError) -> Error {
    let name = match err.kind() {
        "unknown-plugin" => "Stanchion::UnknownPluginError",
        "plugin" => "Stanchion::PluginError",
        "lua" => "Stanchion::LuaError",
        "io" => "Stanchion::IOError",
        "config" => "Stanchion::ConfigError",
        "capability" => "Stanchion::CapabilityError",
        "reentrant" => "Stanchion::ReentrantError",
        _ => "Stanchion::Error",
    };
    let class = ruby
        .eval::<ExceptionClass>(name)
        .unwrap_or_else(|_| ruby.exception_runtime_error());
    Error::new(class, err.to_string())
}

// ---- what a policy answers --------------------------------------------------

/// A policy's answer to one request.
///
/// Built through the three constructors rather than by returning bare values, so
/// that "grant nothing" and "grant with no parameters" cannot be confused.
#[magnus::wrap(class = "Stanchion::Decision", free_immediately, size)]
struct Decision {
    inner: FfiDecision,
}

impl Decision {
    fn grant() -> Decision {
        Decision {
            inner: FfiDecision::Grant,
        }
    }

    fn grant_with(ruby: &Ruby, params: Rb) -> Result<Decision, Error> {
        Ok(Decision {
            inner: FfiDecision::GrantWith(from_ruby(ruby, params)?),
        })
    }

    fn deny(reason: String) -> Decision {
        Decision {
            inner: FfiDecision::Deny(reason),
        }
    }

    fn inspect(&self) -> String {
        match &self.inner {
            FfiDecision::Grant => "#<Stanchion::Decision grant>".to_string(),
            FfiDecision::GrantWith(_) => "#<Stanchion::Decision grant_with>".to_string(),
            FfiDecision::Deny(reason) => format!("#<Stanchion::Decision deny {reason:?}>"),
        }
    }
}

// ---- bridging Ruby callables ------------------------------------------------

/// A Ruby callable answering a capability.
///
/// The callable is held as an [`Opaque`] so this is `Send + Sync` as the seam
/// requires; it is only ever unwrapped on a thread that holds the GVL, which is
/// every thread that can reach [`CapabilityProvider::invoke`] here.
struct RubyProvider {
    callable: Opaque<Rb>,
}

impl CapabilityProvider for RubyProvider {
    fn invoke(&self, call: &FfiCall) -> Result<FfiValue, String> {
        let ruby = Ruby::get().map_err(|_| "a capability was called off the Ruby thread")?;
        let described = describe_call(&ruby, call).map_err(render)?;
        let callable = ruby.get_inner(self.callable);
        let answer: Rb = callable.funcall("call", (described,)).map_err(render)?;
        from_ruby(&ruby, answer).map_err(render)
    }
}

/// A Ruby callable deciding policy.
struct RubyPolicy {
    callable: Opaque<Rb>,
}

impl FfiPolicy for RubyPolicy {
    fn decide(&self, request: &FfiRequest) -> FfiDecision {
        let Ok(ruby) = Ruby::get() else {
            return FfiDecision::Deny("policy was consulted off the Ruby thread".to_string());
        };
        let described = match describe_request(&ruby, request) {
            Ok(described) => described,
            Err(err) => return FfiDecision::Deny(render(err)),
        };
        let callable = ruby.get_inner(self.callable);

        // A policy that raises denies. Letting the exception escape would unwind
        // through Lua and the registry, and a host whose policy is broken should
        // refuse rather than crash.
        let answer: Rb = match callable.funcall("call", (described,)) {
            Ok(answer) => answer,
            Err(err) => return FfiDecision::Deny(format!("the policy raised: {}", render(err))),
        };
        match <&Decision>::try_convert(answer) {
            Ok(decision) => decision.inner.clone(),
            Err(_) => FfiDecision::Deny(
                "a policy must return a Stanchion::Decision: grant, grant_with or deny"
                    .to_string(),
            ),
        }
    }
}

/// Renders a Ruby exception as the string the seam carries.
fn render(err: Error) -> String {
    err.to_string()
}

/// The hash a provider is handed: `plugin:`, `capability:`, `grant:`, `args:`.
fn describe_call(ruby: &Ruby, call: &FfiCall) -> Result<RHash, Error> {
    let hash = ruby.hash_new();
    hash.aset(ruby.to_symbol("plugin"), ruby.str_new(&call.plugin))?;
    hash.aset(ruby.to_symbol("capability"), ruby.str_new(&call.capability))?;
    // The parameters policy approved, which may be narrower than the manifest asked
    // for. A provider re-checks against these rather than trusting the narrowing.
    hash.aset(ruby.to_symbol("grant"), to_ruby(ruby, &call.grant)?)?;
    let args = ruby.ary_new_capa(call.args.len());
    for arg in &call.args {
        args.push(to_ruby(ruby, arg)?)?;
    }
    hash.aset(ruby.to_symbol("args"), args)?;
    Ok(hash)
}

/// The hash a policy is handed.
fn describe_request(ruby: &Ruby, request: &FfiRequest) -> Result<RHash, Error> {
    let hash = ruby.hash_new();
    hash.aset(ruby.to_symbol("plugin"), ruby.str_new(&request.plugin))?;
    hash.aset(ruby.to_symbol("capability"), ruby.str_new(&request.capability))?;
    hash.aset(ruby.to_symbol("params"), to_ruby(ruby, &request.params)?)?;
    hash.aset(ruby.to_symbol("optional"), request.optional)?;
    hash.aset(ruby.to_symbol("signer"), ruby.str_new(&request.signer))?;
    Ok(hash)
}

// ---- the registry -----------------------------------------------------------

/// A registry of Lua plugins.
#[derive(magnus::TypedData)]
#[magnus(class = "Stanchion::Registry", free_immediately, mark)]
struct Registry {
    inner: Arc<Host>,
    /// The callables the providers and policy hold.
    ///
    /// They live inside `Arc<dyn …>` on the Rust side, where Ruby's collector cannot
    /// see them. Marking them from here is what stops a provider being swept while
    /// the registry that calls it is still alive.
    roots: Vec<Opaque<Rb>>,
}

impl magnus::DataTypeFunctions for Registry {
    fn mark(&self, marker: &magnus::gc::Marker) {
        for root in &self.roots {
            marker.mark(*root);
        }
    }
}

/// Reads one optional key out of the options hash.
fn option<T: TryConvert>(
    ruby: &Ruby,
    options: Option<RHash>,
    key: &str,
) -> Result<Option<T>, Error> {
    match options {
        None => Ok(None),
        Some(options) => match options.get(ruby.to_symbol(key)) {
            None => Ok(None),
            Some(value) if value.is_nil() => Ok(None),
            Some(value) => Ok(Some(T::try_convert(value)?)),
        },
    }
}

impl Registry {
    /// Builds a registry.
    ///
    /// Capabilities and policy are fixed here and cannot change afterwards: a host
    /// that could widen a running plugin's reach would have given up the guarantee
    /// the capability system exists to make.
    fn new(ruby: &Ruby, args: &[Rb]) -> Result<Registry, Error> {
        let parsed = magnus::scan_args::scan_args::<(), (Option<RHash>,), (), (), (), ()>(args)?;
        let options = parsed.optional.0;

        let mut config = HostConfig {
            plugins: option::<String>(ruby, options, "plugins")?.map(Into::into),
            ..HostConfig::default()
        };
        config.sandbox.shared = option::<bool>(ruby, options, "shared")?.unwrap_or(false);
        config.signatures.required =
            option::<bool>(ruby, options, "require_signatures")?.unwrap_or(false);
        config.sandbox.libs = option::<Vec<String>>(ruby, options, "libs")?;
        config.sandbox.deny = option::<Vec<String>>(ruby, options, "deny")?;
        if let Some(allow) = option::<Vec<String>>(ruby, options, "allow")? {
            config.capabilities.allow = allow;
        }
        // `nil` keeps the default ceiling; an explicit `0` means "no limit", since a
        // zero-byte allowance would refuse every plugin.
        if let Some(bytes) = option::<usize>(ruby, options, "memory_limit")? {
            config.sandbox.memory_limit = (bytes > 0).then_some(bytes);
        }
        if let Some(instructions) = option::<u64>(ruby, options, "instruction_limit")? {
            config.sandbox.instruction_limit = (instructions > 0).then_some(instructions);
        }

        let mut roots = Vec::new();
        let mut builder = Host::builder().config(config);

        if let Some(capabilities) = option::<RHash>(ruby, options, "capabilities")? {
            let mut declared: Vec<(String, Rb)> = Vec::new();
            capabilities.foreach(|name: Rb, provider: Rb| {
                let name = if let Some(symbol) = Symbol::from_value(name) {
                    symbol.name()?.to_string()
                } else {
                    String::try_convert(name)?
                };
                declared.push((name, provider));
                Ok(magnus::r_hash::ForEach::Continue)
            })?;
            for (name, provider) in declared {
                let callable = Opaque::from(provider);
                roots.push(callable);
                builder = builder.capability(
                    name,
                    Arc::new(RubyProvider { callable }) as Arc<dyn CapabilityProvider>,
                );
            }
        }

        if let Some(policy) = option::<Rb>(ruby, options, "policy")? {
            let callable = Opaque::from(policy);
            roots.push(callable);
            builder = builder.policy(Arc::new(RubyPolicy { callable }) as Arc<dyn FfiPolicy>);
        }

        Ok(Registry {
            inner: Arc::new(builder.build().map_err(|err| raise(ruby, err))?),
            roots,
        })
    }

    /// Discovers and loads every plugin under a root.
    fn load(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<RHash, Error> {
        let parsed = magnus::scan_args::scan_args::<(), (Option<String>,), (), (), (), ()>(args)?;
        let report = rb_self
            .inner
            .load(parsed.optional.0.as_deref().map(std::path::Path::new))
            .map_err(|err| raise(ruby, err))?;

        let failures = ruby.ary_new_capa(report.failures.len());
        for failure in &report.failures {
            let entry = ruby.hash_new();
            entry.aset(ruby.to_symbol("plugin"), ruby.str_new(&failure.plugin))?;
            entry.aset(ruby.to_symbol("reason"), ruby.str_new(&failure.reason))?;
            failures.push(entry)?;
        }

        let hash = ruby.hash_new();
        hash.aset(ruby.to_symbol("loaded"), strings(ruby, &report.loaded)?)?;
        hash.aset(ruby.to_symbol("failures"), failures)?;
        hash.aset(ruby.to_symbol("clean"), report.is_clean())?;
        Ok(hash)
    }

    /// Reports what plugins request, without running any of their code.
    fn audit(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<RArray, Error> {
        let parsed = magnus::scan_args::scan_args::<(), (Option<String>,), (), (), (), ()>(args)?;
        let entries = rb_self
            .inner
            .audit(parsed.optional.0.as_deref().map(std::path::Path::new))
            .map_err(|err| raise(ruby, err))?;

        let array = ruby.ary_new_capa(entries.len());
        for entry in &entries {
            let hash = ruby.hash_new();
            hash.aset(ruby.to_symbol("plugin"), ruby.str_new(&entry.plugin))?;
            hash.aset(
                ruby.to_symbol("capabilities"),
                strings(ruby, &entry.capabilities)?,
            )?;
            hash.aset(ruby.to_symbol("signer"), ruby.str_new(&entry.signer))?;
            array.push(hash)?;
        }
        Ok(array)
    }

    /// The loaded plugins.
    fn list(ruby: &Ruby, rb_self: &Registry) -> Result<RArray, Error> {
        let plugins = rb_self.inner.list().map_err(|err| raise(ruby, err))?;
        let array = ruby.ary_new_capa(plugins.len());
        for plugin in &plugins {
            let hash = ruby.hash_new();
            hash.aset(ruby.to_symbol("name"), ruby.str_new(&plugin.name))?;
            match &plugin.version {
                Some(version) => hash.aset(ruby.to_symbol("version"), ruby.str_new(version))?,
                None => hash.aset(ruby.to_symbol("version"), ruby.qnil())?,
            }
            hash.aset(ruby.to_symbol("granted"), strings(ruby, &plugin.granted)?)?;
            hash.aset(ruby.to_symbol("signer"), ruby.str_new(&plugin.signer))?;
            array.push(hash)?;
        }
        Ok(array)
    }

    /// The loaded plugins' names.
    fn names(ruby: &Ruby, rb_self: &Registry) -> Result<RArray, Error> {
        let names = rb_self.inner.names().map_err(|err| raise(ruby, err))?;
        strings(ruby, &names)
    }

    /// Calls one method on one plugin.
    fn call(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<Rb, Error> {
        let (plugin, method, arguments) = split_call(ruby, args)?;
        let value = rb_self
            .inner
            .call(&plugin, &method, &arguments)
            .map_err(|err| raise(ruby, err))?;
        to_ruby(ruby, &value)
    }

    /// Calls the same method on every plugin, collecting one result each.
    fn dispatch(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<RArray, Error> {
        let (method, arguments) = split_dispatch(ruby, args)?;
        let outcomes = rb_self
            .inner
            .dispatch(&method, &arguments)
            .map_err(|err| raise(ruby, err))?;
        outcomes_to_ruby(ruby, outcomes)
    }

    /// Calls a method that yields, driving it as a coroutine.
    ///
    /// This does **not** free the Ruby VM — the GVL is held throughout, as it is for
    /// every other method here. What it buys is that the plugin may `coroutine.yield`,
    /// which the plain call cannot drive.
    fn call_async(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<Rb, Error> {
        let (plugin, method, arguments) = split_call(ruby, args)?;
        let value = futures_executor::block_on(rb_self.inner.call_async(&plugin, &method, &arguments))
            .map_err(|err| raise(ruby, err))?;
        to_ruby(ruby, &value)
    }

    /// Dispatches a yielding method to every plugin, in turn.
    fn dispatch_async(ruby: &Ruby, rb_self: &Registry, args: &[Rb]) -> Result<RArray, Error> {
        let (method, arguments) = split_dispatch(ruby, args)?;
        let outcomes = futures_executor::block_on(rb_self.inner.dispatch_async(&method, &arguments))
            .map_err(|err| raise(ruby, err))?;
        outcomes_to_ruby(ruby, outcomes)
    }

    /// Re-reads one plugin from disk.
    fn reload(ruby: &Ruby, rb_self: &Registry, plugin: String) -> Result<(), Error> {
        rb_self.inner.reload(&plugin).map_err(|err| raise(ruby, err))
    }

    /// Unbinds a granted capability from a live plugin.
    fn revoke(ruby: &Ruby, rb_self: &Registry, plugin: String, capability: String) -> Result<bool, Error> {
        rb_self.inner
            .revoke(&plugin, &capability)
            .map_err(|err| raise(ruby, err))
    }

    /// Whether plugins share one Lua state: `"shared"` or `"per-plugin"`.
    fn isolation(ruby: &Ruby, rb_self: &Registry) -> Result<String, Error> {
        rb_self.inner
            .isolation()
            .map(str::to_string)
            .map_err(|err| raise(ruby, err))
    }

    fn length(ruby: &Ruby, rb_self: &Registry) -> Result<usize, Error> {
        rb_self.inner.len().map_err(|err| raise(ruby, err))
    }

    fn inspect(ruby: &Ruby, rb_self: &Registry) -> Result<String, Error> {
        Ok(format!("#<Stanchion::Registry {} plugins>", Registry::length(ruby, rb_self)?))
    }
}

fn strings(ruby: &Ruby, values: &[String]) -> Result<RArray, Error> {
    let array = ruby.ary_new_capa(values.len());
    for value in values {
        array.push(ruby.str_new(value))?;
    }
    Ok(array)
}

fn outcomes_to_ruby(ruby: &Ruby, outcomes: Vec<stanchion_ffi::Outcome>) -> Result<RArray, Error> {
    let array = ruby.ary_new_capa(outcomes.len());
    for outcome in &outcomes {
        let hash = ruby.hash_new();
        hash.aset(ruby.to_symbol("plugin"), ruby.str_new(&outcome.plugin))?;
        match &outcome.value {
            Some(value) => hash.aset(ruby.to_symbol("value"), to_ruby(ruby, value)?)?,
            None => hash.aset(ruby.to_symbol("value"), ruby.qnil())?,
        }
        match &outcome.error {
            Some(error) => hash.aset(ruby.to_symbol("error"), ruby.str_new(error))?,
            None => hash.aset(ruby.to_symbol("error"), ruby.qnil())?,
        }
        array.push(hash)?;
    }
    Ok(array)
}

/// `call(plugin, method, *args)`.
fn split_call(ruby: &Ruby, args: &[Rb]) -> Result<(String, String, Vec<FfiValue>), Error> {
    let parsed =
        magnus::scan_args::scan_args::<(String, String), (), RArray, (), (), ()>(args)?;
    let (plugin, method) = parsed.required;
    Ok((plugin, method, convert(ruby, parsed.splat)?))
}

/// `dispatch(method, *args)`.
fn split_dispatch(ruby: &Ruby, args: &[Rb]) -> Result<(String, Vec<FfiValue>), Error> {
    let parsed = magnus::scan_args::scan_args::<(String,), (), RArray, (), (), ()>(args)?;
    Ok((parsed.required.0, convert(ruby, parsed.splat)?))
}

fn convert(ruby: &Ruby, args: RArray) -> Result<Vec<FfiValue>, Error> {
    let mut converted = Vec::with_capacity(args.len());
    for arg in args.into_iter() {
        converted.push(from_ruby(ruby, arg)?);
    }
    Ok(converted)
}

#[magnus::init(name = "stanchion")]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Stanchion")?;

    // Every failure is a `Stanchion::Error`, so one rescue catches the lot, with a
    // subclass per distinction a caller actually acts on.
    let base = module.define_error("Error", ruby.exception_standard_error())?;
    module.define_error("UnknownPluginError", base)?;
    module.define_error("PluginError", base)?;
    module.define_error("LuaError", base)?;
    module.define_error("IOError", base)?;
    module.define_error("ConfigError", base)?;
    module.define_error("CapabilityError", base)?;
    module.define_error("ReentrantError", base)?;

    let decision = module.define_class("Decision", ruby.class_object())?;
    decision.define_singleton_method("grant", function!(Decision::grant, 0))?;
    decision.define_singleton_method("grant_with", function!(Decision::grant_with, 1))?;
    decision.define_singleton_method("deny", function!(Decision::deny, 1))?;
    decision.define_method("inspect", method!(Decision::inspect, 0))?;

    let registry = module.define_class("Registry", ruby.class_object())?;
    registry.define_singleton_method("new", function!(Registry::new, -1))?;
    registry.define_method("load", method!(Registry::load, -1))?;
    registry.define_method("audit", method!(Registry::audit, -1))?;
    registry.define_method("list", method!(Registry::list, 0))?;
    registry.define_method("names", method!(Registry::names, 0))?;
    registry.define_method("call", method!(Registry::call, -1))?;
    registry.define_method("dispatch", method!(Registry::dispatch, -1))?;
    registry.define_method("call_async", method!(Registry::call_async, -1))?;
    registry.define_method("dispatch_async", method!(Registry::dispatch_async, -1))?;
    registry.define_method("reload", method!(Registry::reload, 1))?;
    registry.define_method("revoke", method!(Registry::revoke, 2))?;
    registry.define_method("isolation", method!(Registry::isolation, 0))?;
    registry.define_method("length", method!(Registry::length, 0))?;
    registry.define_method("size", method!(Registry::length, 0))?;
    registry.define_method("inspect", method!(Registry::inspect, 0))?;
    Ok(())
}

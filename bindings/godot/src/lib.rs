//! Godot 4 bindings for stanchion.
//!
//! A thin translation layer over `stanchion-ffi`, exposed to GDScript as a single
//! `Stanchion` class: Godot `Variant`s in, [`Value`]s out, GDScript `Callable`s
//! wrapped as capability providers and policies. Everything that decides *behaviour*
//! lives in `stanchion-ffi`, so this binding and the other four cannot drift apart.
//!
//! # Threading
//!
//! Godot drives game logic on one thread, and this binding is meant to be used from
//! it. A `Callable` is bound to the engine and is not `Send`; the wrappers below
//! assert `Send + Sync` on the strength of that single-threaded contract. Do not
//! share a `Stanchion` across Godot's `WorkerThreadPool` or a `Thread`.

use std::sync::Arc;

use godot::builtin::{VarArray, VarDictionary};
use godot::classes::RefCounted;
use godot::prelude::*;

use stanchion_ffi::{
    CapabilityCall, CapabilityProvider, CapabilityRequest, Decision, Error as FfiError, HostConfig,
    Policy, Stanchion as Host, Value,
};

mod value;

use value::{from_variant, to_variant};

struct StanchionExtension;

#[gdextension]
unsafe impl ExtensionLibrary for StanchionExtension {}

/// Flattens a stanchion error into the `kind: message` string GDScript sees, both in
/// the editor log and through [`Stanchion::get_last_error`].
fn describe(error: &FfiError) -> String {
    format!("{}: {}", error.kind(), error)
}

/// Sets a string-keyed entry on an untyped dictionary.
fn dset(dictionary: &mut VarDictionary, key: &str, value: Variant) {
    let key = key.to_variant();
    dictionary.set(&key, &value);
}

/// Reads a string-keyed entry from an untyped dictionary.
fn dget(dictionary: &VarDictionary, key: &str) -> Option<Variant> {
    let key = key.to_variant();
    dictionary.get(&key)
}

// ---- bridging GDScript callables -------------------------------------------

/// A GDScript `Callable` answering a capability or deciding policy.
///
/// `Callable` is not `Send`, but a stanchion host requires its providers to be. The
/// binding only ever invokes these on the thread that called into Lua, which — per
/// the module contract — is Godot's main thread, so the assertion holds.
struct MainThreadCallable(Callable);

// SAFETY: see the type and module docs — only ever touched on Godot's main thread.
unsafe impl Send for MainThreadCallable {}
// SAFETY: as above.
unsafe impl Sync for MainThreadCallable {}

impl MainThreadCallable {
    /// Calls the wrapped callable with one argument.
    fn call_one(&self, argument: Variant) -> Result<Variant, String> {
        if !self.0.is_valid() {
            return Err("the callable is no longer valid".to_string());
        }
        let mut arguments = VarArray::new();
        arguments.push(&argument);
        Ok(self.0.callv(&arguments))
    }
}

impl CapabilityProvider for MainThreadCallable {
    fn invoke(&self, call: &CapabilityCall) -> Result<Value, String> {
        let mut request = VarDictionary::new();
        dset(&mut request, "plugin", call.plugin.to_variant());
        dset(&mut request, "capability", call.capability.to_variant());
        dset(&mut request, "grant", to_variant(&call.grant));
        let mut args = VarArray::new();
        for arg in &call.args {
            args.push(&to_variant(arg));
        }
        dset(&mut request, "args", args.to_variant());

        let answer = self.call_one(request.to_variant())?;
        from_variant(&answer)
    }
}

impl Policy for MainThreadCallable {
    fn decide(&self, request: &CapabilityRequest) -> Decision {
        let mut payload = VarDictionary::new();
        dset(&mut payload, "plugin", request.plugin.to_variant());
        dset(&mut payload, "capability", request.capability.to_variant());
        dset(&mut payload, "params", to_variant(&request.params));
        dset(&mut payload, "optional", request.optional.to_variant());
        dset(&mut payload, "signer", request.signer.to_variant());

        // A policy that errors denies. Letting the failure escape would unwind through
        // Lua and the registry, and a host whose policy is broken should refuse rather
        // than crash.
        match self.call_one(payload.to_variant()) {
            Err(reason) => Decision::Deny(format!("the policy failed: {reason}")),
            Ok(answer) => interpret_decision(&answer),
        }
    }
}

/// Reads a policy callable's return value as a [`Decision`].
///
/// GDScript has no way to name a Rust enum, so the shape is kept ergonomic:
/// * `true` grants exactly what was asked for, `false` denies;
/// * `{ "grant_with": params }` grants with substituted parameters;
/// * `{ "deny": "reason" }` denies with a message;
/// * a bare `String` denies with that message.
fn interpret_decision(answer: &Variant) -> Decision {
    match answer.get_type() {
        VariantType::BOOL => match answer.try_to::<bool>() {
            Ok(true) => Decision::Grant,
            _ => Decision::Deny("the policy denied the request".to_string()),
        },
        VariantType::STRING | VariantType::STRING_NAME => {
            Decision::Deny(answer.stringify().to_string())
        }
        VariantType::DICTIONARY => match answer.try_to::<VarDictionary>() {
            Err(_) => Decision::Deny("the policy returned an unreadable value".to_string()),
            Ok(dictionary) => {
                if let Some(params) = dget(&dictionary, "grant_with") {
                    match from_variant(&params) {
                        Ok(params) => Decision::GrantWith(params),
                        Err(reason) => {
                            Decision::Deny(format!("grant_with was unreadable: {reason}"))
                        }
                    }
                } else if let Some(reason) = dget(&dictionary, "deny") {
                    Decision::Deny(reason.stringify().to_string())
                } else {
                    Decision::Deny(
                        "a policy dictionary needs a \"grant_with\" or \"deny\" key".to_string(),
                    )
                }
            }
        },
        _ => Decision::Deny(
            "a policy must return true, false, a String, or a { grant_with | deny } Dictionary"
                .to_string(),
        ),
    }
}

// ---- the registry -----------------------------------------------------------

/// A registry of sandboxed Lua plugins, driven from GDScript.
///
/// Register capabilities and a policy first, then [`open`](Stanchion::open) to build
/// the sandbox; afterwards, load and call plugins. Capabilities and policy are fixed
/// at `open` and cannot change on a live registry — a host that could widen a running
/// plugin's reach would have given up the guarantee the capability system exists to
/// make.
#[derive(GodotClass)]
#[class(base = RefCounted)]
pub struct Stanchion {
    providers: Vec<(String, Callable)>,
    policy: Option<Callable>,
    host: Option<Arc<Host>>,
    last_error: String,
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for Stanchion {
    fn init(base: Base<RefCounted>) -> Self {
        Self {
            providers: Vec::new(),
            policy: None,
            host: None,
            last_error: String::new(),
            base,
        }
    }
}

#[godot_api]
impl Stanchion {
    /// Registers a GDScript `Callable` to answer a capability. Call before `open`.
    ///
    /// The callable is handed one Dictionary — `{ plugin, capability, grant, args }` —
    /// and returns the value the plugin receives back.
    #[func]
    fn register_capability(&mut self, name: GString, callable: Callable) {
        if self.host.is_some() {
            self.record("register_capability after open has no effect".to_string());
            return;
        }
        self.providers.push((name.to_string(), callable));
    }

    /// Sets the GDScript `Callable` that decides every capability request. Call before
    /// `open`. See the class docs for the shape of the answer it must return.
    #[func]
    fn set_policy(&mut self, callable: Callable) {
        if self.host.is_some() {
            self.record("set_policy after open has no effect".to_string());
            return;
        }
        self.policy = Some(callable);
    }

    /// Builds the sandbox from a configuration Dictionary. Returns whether it built.
    ///
    /// Recognised keys, all optional: `plugins` (String path), `shared` (bool),
    /// `require_signatures` (bool), `libs`/`deny`/`allow` (Array of String),
    /// `memory_limit` and `instruction_limit` (int; 0 means "no limit").
    #[func]
    fn open(&mut self, config: VarDictionary) -> bool {
        if self.host.is_some() {
            self.record("this Stanchion is already open".to_string());
            return false;
        }

        let host_config = build_config(&config);
        let mut builder = Host::builder().config(host_config);
        for (name, callable) in &self.providers {
            builder = builder.capability(
                name.clone(),
                Arc::new(MainThreadCallable(callable.clone())) as Arc<dyn CapabilityProvider>,
            );
        }
        if let Some(policy) = &self.policy {
            builder =
                builder.policy(Arc::new(MainThreadCallable(policy.clone())) as Arc<dyn Policy>);
        }

        match builder.build() {
            Ok(host) => {
                self.host = Some(Arc::new(host));
                true
            }
            Err(error) => {
                self.record(describe(&error));
                false
            }
        }
    }

    /// Discovers and loads every plugin under a root (empty string means the root the
    /// configuration named). Returns `{ loaded, failures, clean }`.
    #[func]
    fn load(&mut self, root: GString) -> VarDictionary {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return VarDictionary::new();
            }
        };
        match host.load(optional_path(&root).as_deref()) {
            Ok(report) => {
                let mut loaded = VarArray::new();
                for name in &report.loaded {
                    loaded.push(&name.to_variant());
                }
                let mut failures = VarArray::new();
                for failure in &report.failures {
                    let mut entry = VarDictionary::new();
                    dset(&mut entry, "plugin", failure.plugin.to_variant());
                    dset(&mut entry, "reason", failure.reason.to_variant());
                    failures.push(&entry.to_variant());
                }
                let clean = report.is_clean();
                let mut result = VarDictionary::new();
                dset(&mut result, "loaded", loaded.to_variant());
                dset(&mut result, "failures", failures.to_variant());
                dset(&mut result, "clean", clean.to_variant());
                result
            }
            Err(error) => {
                self.record(describe(&error));
                VarDictionary::new()
            }
        }
    }

    /// Reports what each plugin requests, without running any of its code. Returns an
    /// Array of `{ plugin, capabilities, signer }`.
    #[func]
    fn audit(&mut self, root: GString) -> VarArray {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return VarArray::new();
            }
        };
        match host.audit(optional_path(&root).as_deref()) {
            Ok(entries) => {
                let mut array = VarArray::new();
                for entry in &entries {
                    let mut item = VarDictionary::new();
                    dset(&mut item, "plugin", entry.plugin.to_variant());
                    dset(&mut item, "capabilities", strings_to_array(&entry.capabilities));
                    dset(&mut item, "signer", entry.signer.to_variant());
                    array.push(&item.to_variant());
                }
                array
            }
            Err(error) => {
                self.record(describe(&error));
                VarArray::new()
            }
        }
    }

    /// The loaded plugins, as an Array of `{ name, version, granted, signer }`.
    #[func]
    fn list(&mut self) -> VarArray {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return VarArray::new();
            }
        };
        match host.list() {
            Ok(plugins) => {
                let mut array = VarArray::new();
                for plugin in &plugins {
                    let mut item = VarDictionary::new();
                    dset(&mut item, "name", plugin.name.to_variant());
                    match &plugin.version {
                        Some(version) => dset(&mut item, "version", version.to_variant()),
                        None => dset(&mut item, "version", Variant::nil()),
                    }
                    dset(&mut item, "granted", strings_to_array(&plugin.granted));
                    dset(&mut item, "signer", plugin.signer.to_variant());
                    array.push(&item.to_variant());
                }
                array
            }
            Err(error) => {
                self.record(describe(&error));
                VarArray::new()
            }
        }
    }

    /// The loaded plugins' names.
    #[func]
    fn names(&mut self) -> PackedStringArray {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return PackedStringArray::new();
            }
        };
        match host.names() {
            Ok(names) => names.iter().map(|name| GString::from(name.as_str())).collect(),
            Err(error) => {
                self.record(describe(&error));
                PackedStringArray::new()
            }
        }
    }

    /// Calls one method on one plugin. Returns the plugin's result, or `null` on error
    /// (see `get_last_error`).
    #[func]
    fn call(&mut self, plugin: GString, method: GString, args: VarArray) -> Variant {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return Variant::nil();
            }
        };
        let arguments = match collect_args(&args) {
            Ok(arguments) => arguments,
            Err(message) => {
                self.record(message);
                return Variant::nil();
            }
        };
        match host.call(&plugin.to_string(), &method.to_string(), &arguments) {
            Ok(value) => to_variant(&value),
            Err(error) => {
                self.record(describe(&error));
                Variant::nil()
            }
        }
    }

    /// Calls the same method on every plugin, collecting one result each. Returns an
    /// Array of `{ plugin, value, error }`; one plugin failing never affects the rest.
    #[func]
    fn dispatch(&mut self, method: GString, args: VarArray) -> VarArray {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return VarArray::new();
            }
        };
        let arguments = match collect_args(&args) {
            Ok(arguments) => arguments,
            Err(message) => {
                self.record(message);
                return VarArray::new();
            }
        };
        match host.dispatch(&method.to_string(), &arguments) {
            Ok(outcomes) => {
                let mut array = VarArray::new();
                for outcome in &outcomes {
                    let mut item = VarDictionary::new();
                    dset(&mut item, "plugin", outcome.plugin.to_variant());
                    match &outcome.value {
                        Some(value) => dset(&mut item, "value", to_variant(value)),
                        None => dset(&mut item, "value", Variant::nil()),
                    }
                    match &outcome.error {
                        Some(error) => dset(&mut item, "error", error.to_variant()),
                        None => dset(&mut item, "error", Variant::nil()),
                    }
                    array.push(&item.to_variant());
                }
                array
            }
            Err(error) => {
                self.record(describe(&error));
                VarArray::new()
            }
        }
    }

    /// Re-reads one plugin from disk. Returns whether it reloaded.
    #[func]
    fn reload(&mut self, plugin: GString) -> bool {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return false;
            }
        };
        match host.reload(&plugin.to_string()) {
            Ok(()) => true,
            Err(error) => {
                self.record(describe(&error));
                false
            }
        }
    }

    /// Unbinds a granted capability from a live plugin. Returns whether anything was
    /// revoked.
    #[func]
    fn revoke(&mut self, plugin: GString, capability: GString) -> bool {
        let host = match self.opened() {
            Ok(host) => host,
            Err(message) => {
                self.record(message);
                return false;
            }
        };
        match host.revoke(&plugin.to_string(), &capability.to_string()) {
            Ok(revoked) => revoked,
            Err(error) => {
                self.record(describe(&error));
                false
            }
        }
    }

    /// Whether plugins share one Lua state: `"shared"` or `"per-plugin"`; empty on
    /// error.
    #[func]
    fn isolation(&mut self) -> GString {
        match self.opened() {
            Ok(host) => match host.isolation() {
                Ok(mode) => GString::from(mode),
                Err(error) => {
                    self.record(describe(&error));
                    GString::new()
                }
            },
            Err(message) => {
                self.record(message);
                GString::new()
            }
        }
    }

    /// How many plugins are loaded; `0` before `open` or on error.
    #[func]
    fn count(&mut self) -> i64 {
        match self.opened() {
            Ok(host) => match host.len() {
                Ok(count) => i64::try_from(count).unwrap_or(i64::MAX),
                Err(error) => {
                    self.record(describe(&error));
                    0
                }
            },
            Err(message) => {
                self.record(message);
                0
            }
        }
    }

    /// Whether `open` has been called and the sandbox built.
    #[func]
    fn is_open(&self) -> bool {
        self.host.is_some()
    }

    /// The most recent error message, or an empty string if the last call succeeded.
    #[func]
    fn get_last_error(&self) -> GString {
        GString::from(self.last_error.as_str())
    }
}

impl Stanchion {
    /// The live host, or the reason there is not one yet.
    fn opened(&self) -> Result<Arc<Host>, String> {
        match &self.host {
            Some(host) => Ok(Arc::clone(host)),
            None => Err("call open() before using this Stanchion".to_string()),
        }
    }

    /// Records an error for `get_last_error` and surfaces it in the Godot log.
    fn record(&mut self, message: String) {
        godot_error!("stanchion: {message}");
        self.last_error = message;
    }
}

/// Reads a configuration Dictionary into the shape the host is built from.
fn build_config(config: &VarDictionary) -> HostConfig {
    let mut host_config = HostConfig {
        plugins: dget(config, "plugins")
            .and_then(|value| value.try_to::<GString>().ok())
            .map(|path| std::path::PathBuf::from(path.to_string())),
        ..HostConfig::default()
    };
    if let Some(shared) = dget(config, "shared").and_then(|value| value.try_to::<bool>().ok()) {
        host_config.sandbox.shared = shared;
    }
    if let Some(required) =
        dget(config, "require_signatures").and_then(|value| value.try_to::<bool>().ok())
    {
        host_config.signatures.required = required;
    }
    if let Some(libs) = string_list(config, "libs") {
        host_config.sandbox.libs = Some(libs);
    }
    if let Some(deny) = string_list(config, "deny") {
        host_config.sandbox.deny = Some(deny);
    }
    if let Some(allow) = string_list(config, "allow") {
        host_config.capabilities.allow = allow;
    }
    // `0` is taken as "no limit", since a zero-byte allowance would refuse every
    // plugin; an absent key keeps the default ceiling.
    if let Some(bytes) = dget(config, "memory_limit").and_then(|value| value.try_to::<i64>().ok()) {
        host_config.sandbox.memory_limit = (bytes > 0).then_some(bytes.unsigned_abs() as usize);
    }
    if let Some(steps) =
        dget(config, "instruction_limit").and_then(|value| value.try_to::<i64>().ok())
    {
        host_config.sandbox.instruction_limit = (steps > 0).then_some(steps.unsigned_abs());
    }
    host_config
}

/// Reads a Dictionary key as a `Vec<String>`, or `None` when the key is absent.
fn string_list(config: &VarDictionary, key: &str) -> Option<Vec<String>> {
    let array = dget(config, key)?.try_to::<VarArray>().ok()?;
    Some(
        array
            .iter_shared()
            .map(|item| item.stringify().to_string())
            .collect(),
    )
}

/// Empty string means "the configured root"; anything else is a path.
fn optional_path(root: &GString) -> Option<std::path::PathBuf> {
    let text = root.to_string();
    (!text.is_empty()).then(|| std::path::PathBuf::from(text))
}

/// Converts a call's argument Array into the values a plugin receives.
fn collect_args(args: &VarArray) -> Result<Vec<Value>, String> {
    args.iter_shared().map(|arg| from_variant(&arg)).collect()
}

/// Builds a Godot Array of Strings (as a Variant) from a slice of Rust strings.
fn strings_to_array(strings: &[String]) -> Variant {
    let mut array = VarArray::new();
    for value in strings {
        array.push(&value.to_variant());
    }
    array.to_variant()
}

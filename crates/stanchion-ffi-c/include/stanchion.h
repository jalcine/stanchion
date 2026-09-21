#ifndef STANCHION_H
#define STANCHION_H

/// @file stanchion.h
/// C-ABI bindings for the stanchion plugin runtime.
///
/// Complex values (config, arguments, results) travel as JSON strings.
/// The library parses them with serde_json; you only need a JSON library
/// on the C side if you inspect or construct values — the round-trip
/// through JSON strings is what every binding already does.
///
/// Thread safety: a `stanchion_t*` is `Send + Sync`. You may share one
/// instance across threads and call it concurrently.
///
/// Memory: every `char*` returned by a stanchion function was allocated
/// by Rust and must be freed with `stanchion_string_free`. Every
/// `stanchion_t*` returned by `stanchion_init` must be freed with
/// `stanchion_destroy`.

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// ---- opaque handle -------------------------------------------------------

/// Opaque handle to a stanchion plugin registry.
///
/// Holds one or more Lua states (depending on isolation mode), registered
/// capability providers, and loaded plugin instances.
typedef struct stanchion stanchion_t;

// ---- error codes ---------------------------------------------------------

/// Operation succeeded.
#define STANCHION_OK 0
/// No plugin by that name is loaded.
#define STANCHION_ERR_UNKNOWN_PLUGIN 1
/// One plugin failed to load, reload, or verify.
#define STANCHION_ERR_PLUGIN 2
/// A plugin's Lua raised, or a value could not cross the boundary.
#define STANCHION_ERR_LUA 3
/// The plugin root could not be read.
#define STANCHION_ERR_IO 4
/// The host's own configuration is wrong.
#define STANCHION_ERR_CONFIG 5
/// A capability provider refused or failed.
#define STANCHION_ERR_CAPABILITY 6
/// A capability provider called back into the registry that invoked it.
#define STANCHION_ERR_REENTRANT 7
/// A WASM plugin failed to load, instantiate, run, or answer a call.
#define STANCHION_ERR_WASM 8

// ---- lifecycle -----------------------------------------------------------

/// Builds a stanchion registry from a JSON configuration string.
///
/// @param config_json  JSON object describing sandbox, capabilities,
///                     and plugin root. See the example below.
/// @param out_error    If non-NULL and the function returns NULL, this
///                     is set to a heap-allocated error message that
///                     you must free with `stanchion_string_free`.
/// @return An opaque handle, or NULL on error.
///
/// Config schema (all fields optional):
/// @code{json}
/// {
///   "plugins": "/etc/nginx/plugins",
///   "shared": false,
///   "libs": ["io", "os"],
///   "deny": ["os.execute"],
///   "memory_limit": 8388608,
///   "instruction_limit": 200000,
///   "allow": ["log", "http_request"],
///   "require_signatures": false,
///   "capabilities": [
///     { "name": "log", "provider": true },
///     { "name": "http_request", "provider": true }
///   ]
/// }
/// @endcode
stanchion_t* stanchion_init(const char* config_json, char** out_error);

/// Destroys a stanchion instance and frees all associated resources.
void stanchion_destroy(stanchion_t* s);

// ---- plugin management ---------------------------------------------------

/// Discovers and loads every plugin under a directory.
///
/// @param s          A stanchion instance.
/// @param root       Path to the plugin root directory (manifest discovery).
/// @param out_error  If non-NULL, receives a heap-allocated error message
///                   on failure (free with `stanchion_string_free`).
/// @return A JSON string with the load report, or NULL on error.
///         Caller must free with `stanchion_string_free`.
///
/// Result schema:
/// @code{json}
/// {"loaded":["plugin_a"], "failures":[{"plugin":"plugin_b","reason":"..."}]}
/// @endcode
char* stanchion_load(stanchion_t* s, const char* root, char** out_error);

/// Returns a JSON array of loaded plugins.
///
/// @return A JSON string, or NULL on error. Free with `stanchion_string_free`.
///
/// @code{json}
/// [{"name":"risk","version":"0.1.0","granted":["log"],"signer":"unsigned","runtime":"lua"}]
/// @endcode
char* stanchion_list(stanchion_t* s, char** out_error);

/// Reports what plugins under `root` would request, without running them.
///
/// @return A JSON string, or NULL on error. Free with `stanchion_string_free`.
char* stanchion_audit(stanchion_t* s, const char* root, char** out_error);

// ---- calling plugins -----------------------------------------------------

/// Calls one method on one plugin.
///
/// @param s          A stanchion instance.
/// @param plugin     Name of the plugin (matches manifest name).
/// @param method     Exported method name.
/// @param args_json  JSON array of arguments.
/// @param out_error  If non-NULL, receives a heap-allocated error message
///                   on failure (free with `stanchion_string_free`).
/// @return A JSON-encoded result value, or NULL on error.
///         Free with `stanchion_string_free`.
char* stanchion_call(stanchion_t* s,
                     const char* plugin,
                     const char* method,
                     const char* args_json,
                     char** out_error);

/// Calls the same method on every loaded plugin.
///
/// @return A JSON array of per-plugin outcomes, or NULL on error.
///         Free with `stanchion_string_free`.
///
/// @code{json}
/// [{"plugin":"a","value":42},{"plugin":"b","error":"..."}]
/// @endcode
char* stanchion_dispatch(stanchion_t* s,
                         const char* method,
                         const char* args_json,
                         char** out_error);

/// Reloads one plugin from disk.
///
/// @return 0 on success, or an error code.
int stanchion_reload(stanchion_t* s, const char* plugin, char** out_error);

/// Revokes a granted capability from a loaded plugin.
///
/// @return 0 on success (capability was held and revoked),
///         STANCHION_ERR_UNKNOWN_PLUGIN if the plugin does not hold it,
///         or another error code.
int stanchion_revoke(stanchion_t* s,
                     const char* plugin,
                     const char* capability,
                     char** out_error);

// ---- utilities -----------------------------------------------------------

/// Frees a string allocated by any stanchion function.
void stanchion_string_free(char* s);

/// Returns a human-readable description of an error code.
///
/// The returned pointer is a static string literal; it must not be freed.
const char* stanchion_error_string(int code);

#ifdef __cplusplus
}
#endif

#endif /* STANCHION_H */
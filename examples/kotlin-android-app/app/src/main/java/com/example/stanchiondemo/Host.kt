package com.example.stanchiondemo

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.stanchion_uniffi.Config
import uniffi.stanchion_uniffi.Outcome
import uniffi.stanchion_uniffi.Stanchion
import uniffi.stanchion_uniffi.Value

// One Stanchion registry behind the whole screen. All plugin traffic goes
// through here; every query is fail-closed to a built-in default.
//
// Pure JVM (no Android imports) so the same queries run in JVM checks and
// on-device alike. Asset installation lives in MainActivity.

data class PluginState(
    val themes: Map<String, AppTheme> = emptyMap(),
    val failures: List<String> = emptyList(),
    val pluginNames: List<String> = emptyList(),
    val isolation: String = "?",
    val policySeen: Int = 0,
)

object DemoHost {
    fun newHost(
        localeTag: () -> String,
        policy: AppPolicy,
    ): Stanchion {
        val config =
            Config(
                plugins = null,
                libs = null,
                deny = null,
                memoryLimit = null,
                instructionLimit = null,
                shared = false,
                requireSignatures = false,
                allow = listOf(),
            )
        return Stanchion(config, mapOf("locale" to LocaleProvider(localeTag)), policy)
    }

    suspend fun refresh(
        host: Stanchion,
        root: String,
        policy: AppPolicy,
    ): PluginState =
        withContext(Dispatchers.IO) {
            val report = runCatching { host.load(root) }.getOrNull()
            val failures = report?.failures?.map { "${it.plugin}: ${it.reason}" } ?: listOf("load failed")
            val themes = mutableMapOf<String, AppTheme>()
            for (outcome in runCatching { host.dispatch("theme", listOf()) }.getOrDefault(listOf())) {
                val value = outcome.value ?: continue
                val theme = themeFromValue(value) ?: continue
                themes[theme.name] = theme
            }
            PluginState(
                themes = themes,
                failures = failures,
                pluginNames = runCatching { host.names() }.getOrDefault(listOf()),
                isolation = runCatching { host.isolation() }.getOrDefault("?"),
                policySeen = policy.seen.size,
            )
        }

    suspend fun text(
        host: Stanchion,
        plugin: String,
        key: String,
    ): String? =
        withContext(Dispatchers.IO) {
            runCatching {
                host.call(plugin, "text", listOf(Value.Str(key))).asString()
            }.getOrNull()
        }

    suspend fun order(host: Stanchion): List<String> =
        withContext(Dispatchers.IO) {
            runCatching {
                host.dispatch("layout", listOf()).firstNotNullOfOrNull { outcome: Outcome ->
                    outcome.value?.let { orderFromValue(it) }
                }
            }.getOrNull() ?: listOf("header", "body", "flags", "diagnostics")
        }

    suspend fun newHeaderEnabled(host: Stanchion): Boolean =
        withContext(Dispatchers.IO) {
            runCatching {
                host.dispatch("flags", listOf()).firstNotNullOfOrNull { outcome: Outcome ->
                    outcome.value?.let { flagFromValue(it, "newHeader") }
                }
            }.getOrNull() ?: false
        }
}

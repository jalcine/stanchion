package com.example.stanchiondemo

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.stanchion_uniffi.Stanchion
import java.io.File
import java.util.Locale

class MainActivity : ComponentActivity() {
    companion object {
        init {
            System.loadLibrary("stanchion_uniffi")
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { DemoScreen() }
    }

    /** Copies `assets/plugins` to a files-dir root Stanchion can load. */
    private fun installAssets(): String {
        val dest = File(filesDir, "stanchion-plugins")
        if (dest.exists()) dest.deleteRecursively()
        copyAssetDir("plugins", dest)
        return dest.absolutePath
    }

    private fun copyAssetDir(
        assetPath: String,
        dest: File,
    ) {
        val listed = assets.list(assetPath) ?: return
        if (listed.isEmpty()) {
            dest.parentFile?.mkdirs()
            assets.open(assetPath).use { input ->
                dest.outputStream().use { output -> input.copyTo(output) }
            }
            return
        }
        dest.mkdirs()
        for (child in listed) copyAssetDir("$assetPath/$child", File(dest, child))
    }

    @Composable
    fun DemoScreen() {
        val scope = rememberCoroutineScope()
        val policy = remember { AppPolicy() }
        val host: Stanchion =
            remember {
                DemoHost.newHost(
                    localeTag = { Locale.getDefault().toLanguageTag() },
                    policy = policy,
                )
            }
        val root: String = remember { installAssets() }

        var state by remember { mutableStateOf(PluginState()) }
        var themeName by remember { mutableStateOf<String?>(null) }
        var lang by remember { mutableStateOf("en") }
        var greeting by remember { mutableStateOf("…") }
        var subtitle by remember { mutableStateOf("") }
        var headerText by remember { mutableStateOf("") }
        var order by remember { mutableStateOf(listOf("header", "body", "flags", "diagnostics")) }
        var newHeader by remember { mutableStateOf(false) }

        fun stringsPlugin() = if (lang == "es") "strings_es" else "strings_en"

        fun reload() {
            scope.launch {
                state = DemoHost.refresh(host, root, policy)
                if (themeName == null) {
                    themeName =
                        state.themes.keys
                            .sorted()
                            .firstOrNull()
                }
                order = DemoHost.order(host)
                newHeader = DemoHost.newHeaderEnabled(host)
                val plugin = stringsPlugin()
                val rawGreeting = DemoHost.text(host, plugin, "greeting") ?: "Hello, {name}!"
                greeting = interpolate(rawGreeting, mapOf("name" to "Ada"))
                subtitle = DemoHost.text(host, plugin, "subtitle") ?: ""
                val headerKey = if (newHeader) "header_new" else "header_old"
                headerText = DemoHost.text(host, plugin, headerKey) ?: ""
            }
        }

        LaunchedEffect(lang) { reload() }

        val theme = state.themes[themeName] ?: AppTheme.FALLBACK
        MaterialTheme(
            colorScheme =
                darkColorScheme(
                    background = theme.background,
                    surface = theme.surface,
                    primary = theme.primary,
                ),
        ) {
            Surface(modifier = Modifier.fillMaxSize()) {
                Column(
                    modifier =
                        Modifier
                            .verticalScroll(rememberScrollState())
                            .padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    for (section in order) {
                        when (section) {
                            "header" -> {
                                HeaderCard(headerText, greeting, newHeader)
                            }

                            "body" -> {
                                BodyCard(
                                    subtitle = subtitle,
                                    themes = state.themes.keys.sorted(),
                                    selected = themeName,
                                    onTheme = { themeName = it },
                                    lang = lang,
                                    onLang = { lang = it },
                                )
                            }

                            "flags" -> {
                                FlagsCard(newHeader = newHeader, onReload = ::reload)
                            }

                            "diagnostics" -> {
                                DiagnosticsCard(state = state)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun HeaderCard(
    headerText: String,
    greeting: String,
    newHeader: Boolean,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(headerText, style = MaterialTheme.typography.labelMedium)
            Text(greeting, style = MaterialTheme.typography.headlineMedium)
            if (newHeader) Text("● flag-driven header", style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun BodyCard(
    subtitle: String,
    themes: List<String>,
    selected: String?,
    onTheme: (String) -> Unit,
    lang: String,
    onLang: (String) -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(subtitle, style = MaterialTheme.typography.bodyMedium)
            Text("Theme", style = MaterialTheme.typography.labelMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (name in themes) {
                    FilterChip(
                        selected = name == selected,
                        onClick = { onTheme(name) },
                        label = { Text(name) },
                    )
                }
            }
            Text("Language", style = MaterialTheme.typography.labelMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(selected = lang == "en", onClick = { onLang("en") }, label = { Text("EN") })
                FilterChip(selected = lang == "es", onClick = { onLang("es") }, label = { Text("ES") })
            }
        }
    }
}

@Composable
private fun FlagsCard(
    newHeader: Boolean,
    onReload: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Flags", style = MaterialTheme.typography.labelMedium)
            Text("newHeader = $newHeader")
            Button(onClick = onReload) { Text("Reload plugins") }
        }
    }
}

@Composable
private fun DiagnosticsCard(state: PluginState) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Diagnostics", style = MaterialTheme.typography.labelMedium)
            Text("plugins: ${state.pluginNames.sorted().joinToString()}")
            Text("isolation: ${state.isolation} · policy saw: ${state.policySeen}")
            for (failure in state.failures) Text("load: $failure")
        }
    }
}

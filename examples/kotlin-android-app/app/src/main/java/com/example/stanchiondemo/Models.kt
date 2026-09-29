package com.example.stanchiondemo

import androidx.compose.ui.graphics.Color
import uniffi.stanchion_uniffi.Value

// Strict, fail-closed views over the dynamic Value a plugin returned.
// Anything misshapen parses to null and the caller keeps its built-in default.

fun Value.asString(): String? = (this as? Value.Str)?.value

fun Value.asBoolean(): Boolean? = (this as? Value.Bool)?.value

fun Value.asTable(): Map<String, Value>? = (this as? Value.Table)?.entries

fun Value.asList(): List<Value>? = (this as? Value.Seq)?.items

data class AppTheme(
    val name: String,
    val background: Color,
    val surface: Color,
    val primary: Color,
    val scale: Float,
) {
    companion object {
        val FALLBACK =
            AppTheme(
                name = "Built-in",
                background = Color(0xFF101010),
                surface = Color(0xFF1E1E1E),
                primary = Color(0xFF7FB4FF),
                scale = 1.0f,
            )
    }
}

private fun parseColor(raw: String?): Color? {
    if (raw == null || !raw.matches(Regex("#[0-9a-fA-F]{6}"))) return null
    return try {
        val rgb = raw.substring(1).toLong(16).toInt()
        Color(0xFF000000.toInt() or rgb)
    } catch (_: NumberFormatException) {
        null
    }
}

private fun Value.asDouble(): Double? =
    when (this) {
        is Value.Float -> value
        is Value.Int -> value.toDouble()
        else -> null
    }

/** Parses one plugin's `theme()` table; null when the shape is wrong. */
fun themeFromValue(value: Value): AppTheme? {
    val root = value.asTable() ?: return null
    val name = root["name"]?.asString() ?: return null
    val colors = root["colors"]?.asTable() ?: return null
    val fonts = root["fonts"]?.asTable() ?: return null
    val background = parseColor(colors["background"]?.asString()) ?: return null
    val surface = parseColor(colors["surface"]?.asString()) ?: return null
    val primary = parseColor(colors["primary"]?.asString()) ?: return null
    val scale = fonts["scale"]?.asDouble()?.toFloat() ?: 1.0f
    if (scale <= 0.0f || scale > 3.0f) return null
    return AppTheme(name, background, surface, primary, scale)
}

/** Known screen sections; unknown names are dropped, never rendered. */
val KNOWN_SECTIONS = setOf("header", "body", "flags", "diagnostics")

fun orderFromValue(value: Value): List<String>? {
    val items = value.asTable()?.get("order")?.asList() ?: return null
    return items.mapNotNull { it.asString() }.filter { it in KNOWN_SECTIONS }
}

fun flagFromValue(
    value: Value,
    key: String,
): Boolean? = value.asTable()?.get(key)?.asBoolean()

/** Fills `{name}`-style placeholders; unknown placeholders are left as-is. */
fun interpolate(
    template: String,
    args: Map<String, String>,
): String {
    var out = template
    for ((key, arg) in args) out = out.replace("{$key}", arg)
    return out
}

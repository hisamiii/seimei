package com.hisamiowo.seimei

import android.content.Context
import android.os.Build
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import org.json.JSONObject

// ---------- themes (#81): the web page's token files (web/themes, bundled as assets), as Compose colors ----------

val THEMES = listOf("catppuccin", "rose-pine", "gruvbox", "nord", "material-you")

/** One side (dark or light) of a theme: the same tokens the web page sets as CSS variables. */
class Tokens(m: Map<String, Color>, val tints: List<Color>) {
    private val m = m.withDefault { Color.Magenta }
    val surface = this.m.getValue("surface"); val surfaceLow = this.m.getValue("surface-low"); val surfaceLowest = this.m.getValue("surface-lowest")
    val container = this.m.getValue("container"); val containerHigh = this.m.getValue("container-high")
    val containerHighest = this.m.getValue("container-highest")
    val primary = this.m.getValue("primary"); val onPrimary = this.m.getValue("on-primary")
    val primaryContainer = this.m.getValue("primary-container"); val onPrimaryContainer = this.m.getValue("on-primary-container")
    val inversePrimary = this.m.getValue("inverse-primary"); val tint = this.m.getValue("tint")
    val tonal = this.m.getValue("tonal"); val onTonal = this.m.getValue("on-tonal")
    val ok = this.m.getValue("ok"); val warn = this.m.getValue("warn"); val error = this.m.getValue("error")
    val text = this.m.getValue("text"); val text2 = this.m.getValue("text-2"); val text3 = this.m.getValue("text-3")
    val outline = this.m.getValue("outline")
    val warnContainer = this.m.getValue("warn-container"); val onWarnContainer = this.m.getValue("on-warn-container")
    val okContainer = this.m.getValue("ok-container"); val onOkContainer = this.m.getValue("on-ok-container")
    val errorContainer = this.m.getValue("error-container"); val onErrorContainer = this.m.getValue("on-error-container")
    val onWarn = this.m.getValue("on-warn")
}

fun hex(s: String): Color = Color(("FF" + s.removePrefix("#")).toLong(16))
fun hexOf(c: Color): String = "#%06x".format(c.toArgb() and 0xFFFFFF)

/** t of a, the rest b: the rule contrib/themes.py and theme.js use. */
fun mix(a: Color, b: Color, t: Float) = Color(a.red * t + b.red * (1 - t), a.green * t + b.green * (1 - t), a.blue * t + b.blue * (1 - t))

private val files = mutableMapOf<String, JSONObject>()
fun themeFile(ctx: Context, name: String): JSONObject =
    files.getOrPut(name) { JSONObject(ctx.assets.open("$name.json").bufferedReader().use { it.readText() }) }

private fun tokensOf(o: JSONObject): Tokens {
    val m = mutableMapOf<String, Color>()
    for (k in o.keys()) (o.opt(k) as? String)?.takeIf { it.startsWith("#") }?.let { m[k] = hex(it) }
    val t = o.optJSONArray("tints")
    return Tokens(m, List(t?.length() ?: 0) { hex(t!!.getString(it)) })
}

/** A fixed theme's side, with Catppuccin's accent. */
fun fixedTokens(ctx: Context, name: String, dark: Boolean, accent: String?): Tokens {
    val f = themeFile(ctx, name)
    val side = if (dark) "dark" else "light"
    val o = JSONObject(f.getJSONObject(side).toString())
    val acc = f.optJSONObject("accents")?.optJSONObject(side)?.optJSONObject(accent ?: f.optString("accent"))
    if (acc != null) for (k in acc.keys()) o.put(k, acc.getString(k))
    return tokensOf(o)
}

/** An M3 scheme (Android's wallpaper colors) as seimei's tokens, by the same rules as theme.js. */
fun materialTokens(ctx: Context, cs: ColorScheme, dark: Boolean): Tokens {
    val side = themeFile(ctx, "material-you").getJSONObject(if (dark) "dark" else "light")
    val ok = hex(side.getString("ok")); val warn = hex(side.getString("warn"))
    val on = { c: Color -> if (dark) mix(c, Color.White, .46f) else mix(c, Color.Black, .5f) }
    val box = { c: Color -> if (dark) mix(c, cs.surface, .18f) else mix(c, cs.surface, .14f) }
    val m = mapOf(
        "surface" to cs.surface, "surface-low" to mix(cs.surface, cs.surfaceContainerLowest, .5f), "surface-lowest" to cs.surfaceContainerLowest,
        "container" to cs.surfaceContainer, "container-high" to cs.surfaceContainerHigh, "container-highest" to cs.surfaceContainerHighest,
        "primary" to cs.primary, "on-primary" to cs.onPrimary, "primary-container" to cs.primaryContainer,
        "on-primary-container" to cs.onPrimaryContainer, "inverse-primary" to cs.inversePrimary, "tint" to mix(cs.primary, cs.surface, .07f),
        "tonal" to cs.secondaryContainer, "on-tonal" to cs.onSecondaryContainer, "ok" to ok, "warn" to warn, "error" to cs.error,
        "text" to cs.onSurface, "text-2" to cs.onSurfaceVariant, "text-3" to mix(cs.onSurfaceVariant, cs.surface, .78f), "outline" to cs.outline,
        "warn-container" to box(warn), "on-warn-container" to on(warn), "ok-container" to box(ok), "on-ok-container" to on(ok),
        "error-container" to cs.errorContainer, "on-error-container" to cs.onErrorContainer,
        "on-warn" to if (dark) mix(warn, Color.Black, .22f) else Color.White,
    )
    val t = side.getJSONArray("tints")
    return Tokens(m, List(t.length()) { hex(t.getString(it)) })
}

/** Material You needs Android 12 (the wallpaper colors); older phones get Catppuccin. */
val dynamicColors get() = Build.VERSION.SDK_INT >= 31

fun themeTokens(ctx: Context, name: String, dark: Boolean, accent: String?): Tokens =
    if (name == "material-you" && dynamicColors) materialTokens(ctx, if (dark) dynamicDarkColorScheme(ctx) else dynamicLightColorScheme(ctx), dark)
    else fixedTokens(ctx, if (name in THEMES && name != "material-you") name else "catppuccin", dark, accent)

/** The wallpaper's seed for Material You, as the web page rebuilds it: Android's primary key color (tone 50 of the
 * accent palette), whose hue is the seed's. Null before Android 12. */
fun wallpaperSeed(ctx: Context): String? =
    if (dynamicColors) hexOf(Color(ctx.getColor(android.R.color.system_accent1_500))) else null

fun schemeOf(t: Tokens, dark: Boolean): ColorScheme {
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = t.primary, onPrimary = t.onPrimary, primaryContainer = t.primaryContainer, onPrimaryContainer = t.onPrimaryContainer,
        inversePrimary = t.inversePrimary,
        secondary = t.primary, onSecondary = t.onPrimary, secondaryContainer = t.tonal, onSecondaryContainer = t.onTonal,
        tertiary = t.warn, onTertiary = t.onWarn,
        background = t.surface, onBackground = t.text, surface = t.surface, onSurface = t.text, onSurfaceVariant = t.text3,
        surfaceVariant = t.containerHigh,
        surfaceContainerLowest = t.surfaceLowest, surfaceContainerLow = t.surfaceLow, surfaceContainer = t.container,
        surfaceContainerHigh = t.containerHigh, surfaceContainerHighest = t.containerHighest,
        inverseSurface = t.text, inverseOnSurface = t.surface,
        outline = t.outline, outlineVariant = t.containerHighest,
        error = t.error, errorContainer = t.errorContainer, onErrorContainer = t.onErrorContainer,
    )
}

/** Catppuccin Mocha with mauve, the default before the files are read (previews, tests). */
val MOCHA = Tokens(mapOf(
    "surface" to 0xFF1E1E2E, "surface-low" to 0xFF181825, "surface-lowest" to 0xFF11111B, "container" to 0xFF26263A,
    "container-high" to 0xFF313244, "container-highest" to 0xFF3A3B50, "primary" to 0xFFCBA6F7, "on-primary" to 0xFF2D1B47,
    "primary-container" to 0xFF3D3552, "on-primary-container" to 0xFFECDCFF, "inverse-primary" to 0xFF7028D9, "tint" to 0xFF2A2633,
    "tonal" to 0xFF35384D, "on-tonal" to 0xFFDFE3FF, "ok" to 0xFFA6E3A1, "warn" to 0xFFFAB387, "error" to 0xFFF38BA8,
    "text" to 0xFFCDD6F4, "text-2" to 0xFFBAC2DE, "text-3" to 0xFFA6ADC8, "outline" to 0xFF6C7086,
    "warn-container" to 0xFF45393E, "on-warn-container" to 0xFFFFDCC4, "ok-container" to 0xFF364142, "on-ok-container" to 0xFFC8F3C3,
    "error-container" to 0xFF45293A, "on-error-container" to 0xFFFFD0DC, "on-warn" to 0xFF3B2313,
).mapValues { Color(it.value) }, listOf(0xFF89B4FA, 0xFFA6E3A1, 0xFF94E2D5, 0xFFCBA6F7, 0xFFF5C2E7, 0xFFB4BEFE, 0xFFF9E2AF, 0xFF74C7EC,
    0xFFEBA0AC, 0xFFFAB387).map { Color(it) })

val LocalTokens = staticCompositionLocalOf { MOCHA }

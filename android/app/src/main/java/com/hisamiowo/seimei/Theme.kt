package com.hisamiowo.seimei

import android.content.Context
import android.os.Build
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color

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

/** The wallpaper's seed for Material You, as the web page rebuilds it: Android's primary key color (accent tone 50), whose hue is the seed's.
 * Null before Android 12. */
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

package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.ScrollableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.wear.compose.foundation.rotary.RotaryScrollableDefaults
import androidx.wear.compose.foundation.rotary.rotaryScrollable
import androidx.wear.compose.material3.ColorScheme
import androidx.wear.compose.material3.Icon
import androidx.wear.compose.material3.MaterialTheme
import androidx.wear.compose.material3.Text
import androidx.wear.compose.material3.Typography
import com.hisamiowo.seimei.shared.R
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.sqrt

// The watch's type scale, from the mockups (1 dp = 2.125 px on a 480 px, 340 dpi screen).
val BODY = 13.2.sp
val BODY_LH = 16.5.sp
val SMALL = 11.8.sp
val TITLE = 15.sp
val SMALL1 = 12.8.sp
val SMALL0 = 10.8.sp
val BODY1 = 14.2.sp
val BODY_H = 13.7.sp
val LABEL = 10.8.sp

@OptIn(ExperimentalTextApi::class)
val Sans = FontFamily(listOf(400, 500, 600, 700).map { w ->
    Font(R.font.google_sans_flex, FontWeight(w), variationSettings = FontVariation.Settings(FontVariation.weight(w)))
})

/** The theme's tokens; the watch is always dark. */
val LocalT = staticCompositionLocalOf { MOCHA }

@Composable
fun WearTheme(name: String, content: @Composable () -> Unit) {
    val ctx = LocalContext.current
    val t = remember(name) { fixedTokens(ctx, if (name in THEMES && name != "material-you") name else "catppuccin", true, null) }
    val cs = ColorScheme(
        primary = t.primary, primaryDim = t.primary, primaryContainer = t.primaryContainer, onPrimary = t.onPrimary,
        onPrimaryContainer = t.onPrimaryContainer, secondary = t.primary, secondaryContainer = t.tonal, onSecondaryContainer = t.onTonal,
        tertiary = t.warn, onTertiary = t.onWarn, surfaceContainerLow = t.surfaceLow, surfaceContainer = t.container,
        surfaceContainerHigh = t.containerHigh, onSurface = t.text, onSurfaceVariant = t.text2, outline = t.outline,
        outlineVariant = t.containerHighest, background = t.surface, onBackground = t.text, error = t.error,
    )
    MaterialTheme(colorScheme = cs, typography = Typography(defaultFontFamily = Sans)) {
        CompositionLocalProvider(LocalT provides t, LocalTokens provides t, content = content)
    }
}

/** A line of text in the watch's scale. */
@Composable
fun T(
    text: String, modifier: Modifier = Modifier, size: TextUnit = BODY, color: Color = LocalT.current.text, weight: FontWeight = FontWeight.Normal,
    lines: Int = Int.MAX_VALUE, lineHeight: TextUnit = TextUnit.Unspecified, mono: Boolean = false, align: TextAlign? = null,
) = Text(text, modifier, color = color, fontSize = size, fontWeight = weight, maxLines = lines, overflow = TextOverflow.Ellipsis, textAlign = align,
    lineHeight = if (lineHeight == TextUnit.Unspecified) size * 1.25f else lineHeight,
    style = TextStyle(fontFamily = if (mono) FontFamily.Monospace else Sans, letterSpacing = 0.sp))

@Composable
fun Ico(v: ImageVector, size: Dp = 16.dp, tint: Color = LocalT.current.text2, desc: String? = null) =
    Icon(v, desc, Modifier.size(size), tint = tint)

/** A row's corners in a segmented group: big outer ones on the group's ends, small ones between the rows. */
fun segment(i: Int, n: Int, outer: Dp = 12.dp, inner: Dp = 3.dp): Shape =
    RoundedCornerShape(if (i == 0) outer else inner, if (i == 0) outer else inner, if (i == n - 1) outer else inner, if (i == n - 1) outer else inner)

/**
 * Narrows a full-width row to what the round screen leaves at its height, as rows near the top and bottom edges of a circle must be.
 * The row's height stays; only its width follows its place on the screen (one frame behind while scrolling).
 */
@Composable
fun CircleRow(modifier: Modifier = Modifier, inset: Dp = 2.dp, widest: Dp = 166.dp, content: @Composable () -> Unit) {
    val conf = LocalConfiguration.current
    val screen = conf.screenHeightDp.dp
    var top by remember { mutableFloatStateOf(-1f) }
    Box(modifier.fillMaxWidth().onPlaced { top = it.positionInRoot().y }, contentAlignment = Alignment.Center) {
        Box(Modifier.layout { m, c ->
            val r = screen.toPx() / 2
            val y0 = top
            val y1 = top + m.minIntrinsicHeight(c.maxWidth).toFloat()
            // The edge of the row farther from the middle sets the width; the safe circle is 92% of the screen's.
            val dy = if (y0 < 0) 0f else max(abs(y0 - r), abs(y1 - r))
            val safe = r * 0.92f
            val half = sqrt(max(0f, safe * safe - dy * dy)) - inset.toPx()
            val w = minOf(c.maxWidth, widest.roundToPx(), max(0f, 2 * half).toInt()).coerceAtLeast(minOf(c.maxWidth, 96.dp.roundToPx()))
            val p = m.measure(c.copy(minWidth = w, maxWidth = w))
            layout(p.width, p.height) { p.place(0, 0) }
        }) { content() }
    }
}

/** Fades the top [top] and bottom [bottom] of a scrolling list into the background. */
fun Modifier.fadeEdges(top: Dp, bottom: Dp = 0.dp) = this
    .graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)
    .drawWithContent {
        drawContent()
        val h = size.height
        // The outer half of each edge is gone, the inner half ramps in: rows under the clock or the usage arc stay unread.
        if (top > 0.dp) drawRect(Brush.verticalGradient(0f to Color.Transparent, top.toPx() / 2 / h to Color.Transparent, top.toPx() / h to Color.Black), blendMode = BlendMode.DstIn)
        if (bottom > 0.dp) drawRect(Brush.verticalGradient(1f - bottom.toPx() / h to Color.Black, 1f - bottom.toPx() / 2 / h to Color.Transparent, 1f to Color.Transparent), blendMode = BlendMode.DstIn)
    }

/** Turns the crown or bezel into scrolling for [state]; the list takes the focus once shown. */
@Composable
fun Modifier.crown(state: ScrollableState, reverse: Boolean = false, active: Boolean = true): Modifier {
    val focus = remember { FocusRequester() }
    LaunchedEffect(active) { if (active) runCatching { focus.requestFocus() } }
    return this.rotaryScrollable(RotaryScrollableDefaults.behavior(state), focus, reverse)
}

/** A small state title over a list's group: "Needs you", "Working". */
@Composable
fun Label(text: String, color: Color, modifier: Modifier = Modifier) =
    T(text, modifier.fillMaxWidth().padding(top = 6.dp, bottom = 3.dp), LABEL, color, FontWeight.SemiBold, 1, align = TextAlign.Center)

/** One row of a list on a round screen: a lead, the name, a trailing note. */
@Composable
fun ListRow(
    name: String, modifier: Modifier = Modifier, height: Dp = 27.dp, bg: Color = Color.Transparent, shape: Shape = RoundedCornerShape(14.dp),
    lead: (@Composable () -> Unit)? = null, trail: (@Composable RowScope.() -> Unit)? = null, weight: FontWeight = FontWeight.Medium,
    color: Color = LocalT.current.text, onClick: (() -> Unit)? = null,
) = CircleRow(modifier) {
    Row(
        Modifier.height(height).clip(shape).background(bg).let { if (onClick != null) it.clickable(onClick = onClick) else it }
            .padding(start = 8.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (lead != null) Box(Modifier.size(16.dp), contentAlignment = Alignment.Center) { lead() }
        T(name, Modifier.weight(1f), BODY, color, weight, 1)
        trail?.invoke(this)
    }
}

@Composable
fun Dot(color: Color = LocalT.current.primary) = Box(Modifier.size(6.dp).clip(CircleShape).background(color))

@Composable
fun Gap(h: Dp) = Spacer(Modifier.height(h))

/** An agent's tile at list size: its identity shape in its state's tone, as on the phone. */
@Composable
fun Tile(a: Agent, size: Dp) {
    val t = LocalT.current
    val (bg, fg) = when (a.state) {
        "ask" -> t.warn to t.onWarn
        "work" -> t.primary to t.onPrimary
        "idle" -> t.containerHighest to t.text2
        else -> t.containerHigh to t.text2
    }
    val shape = if (a.state == "done") CircleShape else PolyShape(SHAPES.getValue(shapeOf(a.name)))
    Box(Modifier.size(size).clip(shape).background(bg), contentAlignment = Alignment.Center) {
        when (a.state) {
            "ask" -> Ico(Icons.question, size * .5f, fg)
            "idle" -> Ico(a.icon ?: Icons.clock, size * .5f, fg)
            "done" -> Ico(if (a.bad) Icons.alert else Icons.check, size * .5f, fg)
        }
    }
}

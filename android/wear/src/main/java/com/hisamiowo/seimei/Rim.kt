package com.hisamiowo.seimei

import android.graphics.Paint
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

// The status rim (mockup v5): thin arcs in the screen's edge, outside the content's circle.
// Angles count clockwise from 12 o'clock, in degrees; lengths are in 480 px units of the round screen.
private const val R = 229f
private const val W = 6f
private const val SEG = 6.5f
private const val GAP = 3f
private const val FIRST = 303f
private const val MAX_ARCS = 8

/** The rim over every screen: one arc per agent down the left edge (tap: the agents), the 5-hour window along the bottom. */
@Composable
fun Rim(vm: Model, usageArc: Boolean = true) {
    val t = LocalT.current
    val states = vm.agents().map { it.state }.sortedBy { listOf("ask", "work", "idle").indexOf(it) }.take(MAX_ARCS)
    val five = vm.usage?.optJSONObject("windows")?.optJSONObject("five_hour")
    val share = five?.optDouble("utilization", 0.0)?.toFloat()?.coerceIn(0f, 1f)
    val pulse = if (states.firstOrNull() == "ask") {
        val run = rememberInfiniteTransition("rim")
        run.animateFloat(.25f, .8f, infiniteRepeatable(tween(1100, easing = LinearEasing), RepeatMode.Reverse), "pulse").value
    } else 0f
    val idle = mix(t.containerHighest, t.text3, .55f)
    val paint = remember { Paint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER } }
    Box(Modifier.fillMaxSize()) {
        Canvas(Modifier.fillMaxSize()) {
            val k = size.minDimension / 480f
            var a = FIRST
            for ((i, s) in states.withIndex()) {
                val c = when (s) { "ask" -> t.warn; "work" -> t.primary; else -> idle }
                if (i == 0 && s == "ask") arc(a - SEG, SEG, R, 16f * k, c.copy(alpha = .35f * pulse), k)
                arc(a - SEG, SEG, R, W * k, c, k)
                a -= SEG + GAP
            }
            if (usageArc && share != null) {
                arc(157f, 46f, R, W * k, t.containerHigh, k)
                if (share > 0f) arc(203f - share * 46f, share * 46f, R, W * k, if (share >= .9f) t.error else t.text3, k)
                val r = (R - 12f) * k
                val path = android.graphics.Path().apply {
                    addArc(center.x - r, center.y - r, center.x + r, center.y + r, 212f - 90f, -64f)
                }
                paint.textSize = 19f * k
                paint.color = t.text3.toArgb()
                paint.isFakeBoldText = false
                paint.letterSpacing = .03f
                drawIntoCanvas { it.nativeCanvas.drawTextOnPath("${Math.round(share * 100)}%", path, 0f, 0f, paint) }
            }
        }
        // The left edge is the agents' door: a strip where the arcs are.
        if (states.isNotEmpty()) Box(
            Modifier.offset(0.dp, 46.dp).size(26.dp, 70.dp).semantics { contentDescription = "Agents" }
                .clickable(remember { MutableInteractionSource() }, null) { vm.push(Screen.Agents) },
        )
    }
}

/** An arc of the rim from [from] clockwise over [sweep] degrees. */
private fun DrawScope.arc(from: Float, sweep: Float, r: Float, width: Float, color: Color, k: Float) {
    val rr = r * k
    drawArc(color, from - 90f, sweep, false, Offset(center.x - rr, center.y - rr), Size(2 * rr, 2 * rr), style = Stroke(width, cap = StrokeCap.Round))
}

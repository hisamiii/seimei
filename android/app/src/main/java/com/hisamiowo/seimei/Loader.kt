package com.hisamiowo.seimei

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

// The M3 loading indicator, the same as web/loading.css: a shape that morphs through seven and turns.
// Generated from it (48 points per shape, so any two interpolate); the easing is the expressive spatial spring as a linear() fit.
private val LOADER_SHAPES = arrayOf(
    // soft-burst
    floatArrayOf(0.500f, 0.170f, 0.551f, 0.110f, 0.626f, 0.028f, 0.682f, 0.061f, 0.686f, 0.177f, 0.703f, 0.236f, 0.793f, 0.207f, 0.894f, 0.197f, 0.896f, 0.271f, 0.828f, 0.364f, 0.830f, 0.412f, 0.933f, 0.443f, 1.000f, 0.500f, 0.933f, 0.557f, 0.830f, 0.588f, 0.828f, 0.636f, 0.896f, 0.729f, 0.894f, 0.803f, 0.793f, 0.793f, 0.703f, 0.764f, 0.686f, 0.823f, 0.682f, 0.939f, 0.626f, 0.972f, 0.551f, 0.890f, 0.500f, 0.830f, 0.449f, 0.890f, 0.374f, 0.972f, 0.318f, 0.939f, 0.314f, 0.823f, 0.297f, 0.764f, 0.207f, 0.793f, 0.106f, 0.803f, 0.104f, 0.729f, 0.172f, 0.636f, 0.170f, 0.588f, 0.067f, 0.557f, 0.000f, 0.500f, 0.067f, 0.443f, 0.170f, 0.412f, 0.172f, 0.364f, 0.104f, 0.271f, 0.106f, 0.197f, 0.207f, 0.207f, 0.297f, 0.236f, 0.314f, 0.177f, 0.318f, 0.061f, 0.374f, 0.028f, 0.449f, 0.110f),
    // cookie-9
    floatArrayOf(0.500f, 0.055f, 0.565f, 0.008f, 0.625f, 0.033f, 0.662f, 0.108f, 0.695f, 0.162f, 0.758f, 0.164f, 0.842f, 0.158f, 0.893f, 0.198f, 0.885f, 0.277f, 0.864f, 0.349f, 0.892f, 0.395f, 0.962f, 0.439f, 1.000f, 0.500f, 0.962f, 0.561f, 0.892f, 0.605f, 0.864f, 0.651f, 0.885f, 0.722f, 0.893f, 0.802f, 0.842f, 0.842f, 0.758f, 0.836f, 0.695f, 0.838f, 0.662f, 0.892f, 0.625f, 0.967f, 0.565f, 0.992f, 0.500f, 0.945f, 0.449f, 0.891f, 0.395f, 0.892f, 0.322f, 0.931f, 0.250f, 0.933f, 0.216f, 0.870f, 0.213f, 0.787f, 0.187f, 0.740f, 0.115f, 0.722f, 0.042f, 0.690f, 0.033f, 0.625f, 0.080f, 0.555f, 0.110f, 0.500f, 0.080f, 0.445f, 0.033f, 0.375f, 0.042f, 0.310f, 0.115f, 0.277f, 0.187f, 0.260f, 0.213f, 0.213f, 0.216f, 0.130f, 0.250f, 0.067f, 0.322f, 0.069f, 0.395f, 0.108f, 0.449f, 0.109f),
    // pentagon
    floatArrayOf(0.500f, 0.000f, 0.562f, 0.030f, 0.618f, 0.059f, 0.670f, 0.089f, 0.720f, 0.119f, 0.767f, 0.152f, 0.812f, 0.188f, 0.856f, 0.227f, 0.901f, 0.269f, 0.947f, 0.315f, 0.972f, 0.374f, 0.962f, 0.439f, 0.951f, 0.500f, 0.939f, 0.558f, 0.923f, 0.613f, 0.905f, 0.668f, 0.884f, 0.722f, 0.860f, 0.776f, 0.832f, 0.832f, 0.801f, 0.892f, 0.739f, 0.914f, 0.676f, 0.925f, 0.616f, 0.932f, 0.557f, 0.936f, 0.500f, 0.938f, 0.443f, 0.936f, 0.384f, 0.932f, 0.324f, 0.925f, 0.261f, 0.914f, 0.199f, 0.892f, 0.168f, 0.832f, 0.140f, 0.776f, 0.116f, 0.722f, 0.095f, 0.668f, 0.077f, 0.613f, 0.061f, 0.558f, 0.049f, 0.500f, 0.038f, 0.439f, 0.028f, 0.374f, 0.053f, 0.315f, 0.099f, 0.269f, 0.144f, 0.227f, 0.188f, 0.188f, 0.233f, 0.152f, 0.280f, 0.119f, 0.330f, 0.089f, 0.382f, 0.059f, 0.438f, 0.030f),
    // pill
    floatArrayOf(0.500f, 0.190f, 0.541f, 0.190f, 0.583f, 0.190f, 0.628f, 0.191f, 0.677f, 0.194f, 0.731f, 0.198f, 0.792f, 0.208f, 0.856f, 0.227f, 0.916f, 0.260f, 0.963f, 0.308f, 0.990f, 0.369f, 0.999f, 0.434f, 1.000f, 0.500f, 0.999f, 0.566f, 0.990f, 0.631f, 0.963f, 0.692f, 0.916f, 0.740f, 0.856f, 0.773f, 0.792f, 0.792f, 0.731f, 0.802f, 0.677f, 0.806f, 0.628f, 0.809f, 0.583f, 0.810f, 0.541f, 0.810f, 0.500f, 0.810f, 0.459f, 0.810f, 0.417f, 0.810f, 0.372f, 0.809f, 0.323f, 0.806f, 0.269f, 0.802f, 0.208f, 0.792f, 0.144f, 0.773f, 0.084f, 0.740f, 0.037f, 0.692f, 0.010f, 0.631f, 0.001f, 0.566f, 0.000f, 0.500f, 0.001f, 0.434f, 0.010f, 0.369f, 0.037f, 0.308f, 0.084f, 0.260f, 0.144f, 0.227f, 0.208f, 0.208f, 0.269f, 0.198f, 0.323f, 0.194f, 0.372f, 0.191f, 0.417f, 0.190f, 0.459f, 0.190f),
    // sunny
    floatArrayOf(0.500f, 0.000f, 0.563f, 0.019f, 0.618f, 0.061f, 0.668f, 0.093f, 0.728f, 0.106f, 0.795f, 0.115f, 0.854f, 0.146f, 0.885f, 0.205f, 0.894f, 0.273f, 0.907f, 0.332f, 0.939f, 0.382f, 0.981f, 0.437f, 1.000f, 0.500f, 0.981f, 0.563f, 0.939f, 0.618f, 0.907f, 0.668f, 0.894f, 0.727f, 0.885f, 0.795f, 0.854f, 0.854f, 0.795f, 0.885f, 0.728f, 0.894f, 0.668f, 0.907f, 0.618f, 0.939f, 0.563f, 0.981f, 0.500f, 1.000f, 0.437f, 0.981f, 0.382f, 0.939f, 0.332f, 0.907f, 0.273f, 0.894f, 0.205f, 0.885f, 0.146f, 0.854f, 0.115f, 0.795f, 0.106f, 0.728f, 0.093f, 0.668f, 0.061f, 0.618f, 0.019f, 0.563f, 0.000f, 0.500f, 0.019f, 0.437f, 0.061f, 0.382f, 0.093f, 0.332f, 0.106f, 0.272f, 0.115f, 0.205f, 0.146f, 0.146f, 0.205f, 0.115f, 0.272f, 0.106f, 0.332f, 0.093f, 0.382f, 0.061f, 0.437f, 0.019f),
    // cookie-4
    floatArrayOf(0.500f, 0.000f, 0.564f, 0.013f, 0.621f, 0.048f, 0.666f, 0.098f, 0.701f, 0.151f, 0.731f, 0.200f, 0.762f, 0.238f, 0.800f, 0.269f, 0.849f, 0.299f, 0.902f, 0.334f, 0.952f, 0.379f, 0.987f, 0.436f, 1.000f, 0.500f, 0.987f, 0.564f, 0.952f, 0.621f, 0.902f, 0.666f, 0.849f, 0.701f, 0.800f, 0.731f, 0.762f, 0.762f, 0.731f, 0.800f, 0.701f, 0.849f, 0.666f, 0.902f, 0.621f, 0.952f, 0.564f, 0.987f, 0.500f, 1.000f, 0.436f, 0.987f, 0.379f, 0.952f, 0.334f, 0.902f, 0.299f, 0.849f, 0.269f, 0.800f, 0.238f, 0.762f, 0.200f, 0.731f, 0.151f, 0.701f, 0.098f, 0.666f, 0.048f, 0.621f, 0.013f, 0.564f, 0.000f, 0.500f, 0.013f, 0.436f, 0.048f, 0.379f, 0.098f, 0.334f, 0.151f, 0.299f, 0.200f, 0.269f, 0.238f, 0.238f, 0.269f, 0.200f, 0.299f, 0.151f, 0.334f, 0.098f, 0.379f, 0.048f, 0.436f, 0.013f),
    // oval
    floatArrayOf(0.500f, 0.110f, 0.551f, 0.112f, 0.602f, 0.118f, 0.654f, 0.129f, 0.705f, 0.144f, 0.757f, 0.165f, 0.808f, 0.192f, 0.856f, 0.226f, 0.902f, 0.268f, 0.942f, 0.317f, 0.973f, 0.373f, 0.993f, 0.435f, 1.000f, 0.500f, 0.993f, 0.565f, 0.973f, 0.627f, 0.942f, 0.683f, 0.902f, 0.732f, 0.856f, 0.774f, 0.808f, 0.808f, 0.757f, 0.835f, 0.705f, 0.856f, 0.654f, 0.871f, 0.602f, 0.882f, 0.551f, 0.888f, 0.500f, 0.890f, 0.449f, 0.888f, 0.398f, 0.882f, 0.346f, 0.871f, 0.295f, 0.856f, 0.243f, 0.835f, 0.192f, 0.808f, 0.144f, 0.774f, 0.098f, 0.732f, 0.058f, 0.683f, 0.027f, 0.627f, 0.007f, 0.565f, 0.000f, 0.500f, 0.007f, 0.435f, 0.027f, 0.373f, 0.058f, 0.317f, 0.098f, 0.268f, 0.144f, 0.226f, 0.192f, 0.192f, 0.243f, 0.165f, 0.295f, 0.144f, 0.346f, 0.129f, 0.398f, 0.118f, 0.449f, 0.112f),
)
private val SPRING = floatArrayOf(0f, .103f, .3f, .506f, .684f, .817f, .907f, .962f, .994f, 1.009f, 1.014f, 1.014f, 1.012f, 1.008f, 1.006f, 1.003f, 1f)

private fun spring(t: Float): Float {
    val x = t * (SPRING.size - 1)
    val i = x.toInt().coerceAtMost(SPRING.size - 2)
    return SPRING[i] + (SPRING[i + 1] - SPRING[i]) * (x - i)
}

/** The loading indicator; with a container it sits in a primary-container circle, as the contained variant. */
@Composable
fun Loader(size: Dp = 48.dp, contained: Boolean = true, color: Color = MaterialTheme.colorScheme.primary, modifier: Modifier = Modifier) {
    val run = rememberInfiniteTransition("loader")
    val t by run.animateFloat(0f, 1f, infiniteRepeatable(tween(4900, easing = LinearEasing), RepeatMode.Restart), "t")
    Box(
        modifier.size(size).then(if (contained) Modifier.clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(size * (38f / 48f)).rotate(720f * t)) {
            val n = LOADER_SHAPES.size
            val x = t * n
            val i = x.toInt() % n
            val f = spring(x - x.toInt())
            val a = LOADER_SHAPES[i]
            val b = LOADER_SHAPES[(i + 1) % n]
            val path = Path()
            var k = 0
            while (k < a.size) {
                val px = (a[k] + (b[k] - a[k]) * f) * this.size.width
                val py = (a[k + 1] + (b[k + 1] - a[k + 1]) * f) * this.size.height
                if (k == 0) path.moveTo(px, py) else path.lineTo(px, py)
                k += 2
            }
            path.close()
            drawPath(path, color)
        }
    }
}

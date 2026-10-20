package com.hisamiowo.seimei

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

val APP_TONES = listOf(0xFFCBA6F7, 0xFF89B4FA, 0xFFA6E3A1, 0xFFF9E2AF, 0xFFFAB387, 0xFFF5C2E7, 0xFF94E2D5, 0xFFB4BEFE).map { Color(it) }

fun appTone(app: String): Color {
    var h = 0L
    for (ch in app.ifEmpty { "?" }) h = (h * 31 + ch.code) and 0xFFFFFFFFL
    return APP_TONES[(h % APP_TONES.size).toInt()]
}

/** "Laptop screen" and "Portrait, 1600 × 2560" instead of an output name and a mode. */
fun outputLabel(o: JSONObject): Pair<String, String> {
    val name = if (o.str("name").startsWith("eDP")) "Laptop screen" else "Screen ${o.str("name")}"
    if (o.optInt("width") == 0) return name to ""
    val turned = Regex("90|270").containsMatchIn(o.str("transform"))
    val (w, h) = if (turned) o.optInt("height") to o.optInt("width") else o.optInt("width") to o.optInt("height")
    return name to "${if (h > w) "Portrait" else "Landscape"}, $w × $h"
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ScreenTab(vm: Seimei) {
    var mode by rememberSaveable { mutableStateOf("screen") }
    var window by rememberSaveable { mutableStateOf<Long?>(null) }
    var paused by rememberSaveable { mutableStateOf(false) }
    var desk by remember { mutableStateOf<JSONObject?>(null) }
    var frame by remember { mutableStateOf<ImageBitmap?>(null) }
    var msg by remember { mutableStateOf("") }
    var zoom by remember { mutableStateOf(false) }
    val scroll = rememberScrollState()
    val ctx = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()

    val windows = desk?.optJSONArray("workspaces").objects().flatMap { it.optJSONArray("windows").objects() }
    val target = if (mode == "screen") null else windows.firstOrNull { it.optLong("id") == window } ?: windows.firstOrNull { it.optBoolean("focused") }
    val targetId = target?.optLong("id")

    // The window list every 5 s, frames about once a second (a window shot borrows the clipboard, so every 4 s).
    LaunchedEffect(vm.visible, paused) {
        if (desk == null) desk = vm.desktop()
        while (vm.visible && !paused) {
            delay(5000)
            vm.desktop()?.let { desk = it }
        }
    }
    LaunchedEffect(mode, targetId, paused, vm.visible) {
        while (vm.visible && !paused) {
            // Each frame is ~180 KB: on a metered network one every 3 s instead of every second.
            val metered = ctx.getSystemService(android.net.ConnectivityManager::class.java)?.isActiveNetworkMetered == true
            var wait = if (mode == "screen") (if (metered) 3000L else 1000L) else 4000L
            if (mode == "window" && targetId == null) {
                msg = if (desk == null) "" else "Pick a window below."
            } else {
                when (val f = vm.frame(if (mode == "screen") "mode=screen" else "mode=window&id=$targetId")) {
                    is Frame.Image -> {
                        frame = f.bmp
                        msg = if (desk?.optBoolean("locked") == true) "Screen locked" else ""
                    }
                    is Frame.Problem -> {
                        msg = f.text
                        wait = 5000
                    }
                }
            }
            delay(wait)
        }
    }

    fun pick(m: String, id: Long?) {
        if (m != mode || id != window) frame = null
        mode = m
        window = id
        scope.launch { scroll.animateScrollTo(0) }
    }

    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        BoxWithConstraints(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(cs.surfaceContainerLowest).heightIn(min = 160.dp),
            contentAlignment = Alignment.Center,
        ) {
            frame?.let { b ->
                Image(b, "The laptop screen", Modifier.fillMaxWidth().heightIn(max = maxWidth * 1.6f).aspectRatio(b.width.toFloat() / b.height)
                    .clickable { zoom = true }, contentScale = ContentScale.Fit)
            }
            LiveChip(!paused, Modifier.align(Alignment.TopEnd).padding(12.dp))
            if (msg.isNotEmpty()) Text(msg, fontSize = 14.sp, color = Color.White,
                modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(Color.Black.copy(alpha = .6f)).padding(horizontal = 12.dp, vertical = 8.dp))
        }
        val out = desk?.optJSONArray("outputs").objects().firstOrNull()
        val (title, sub) = if (mode == "screen") (out?.let { outputLabel(it) } ?: ("" to "")) else (target?.str("title") ?: "No window") to (target?.str("app") ?: "")
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(title, fontWeight = FontWeight.Medium, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (sub.isNotEmpty()) Text(sub, fontSize = 13.sp, color = LocalExtra.current.text3)
            }
            FilledTonalButton({ paused = !paused }) { Text(if (paused) "Resume" else "Pause") }
            Spacer(Modifier.width(8.dp))
            FilledTonalIconButton({ if (frame != null) zoom = true }) { Icon(Icons.full, "Full screen", Modifier.size(18.dp)) }
        }
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            SegmentedButton(mode == "screen", { pick("screen", null) }, SegmentedButtonDefaults.itemShape(0, 2)) { Text("Whole screen") }
            SegmentedButton(mode == "window", { pick("window", window) }, SegmentedButtonDefaults.itemShape(1, 2)) { Text("Window") }
        }
        if (desk?.optBoolean("up", true) == false) Text("The desktop is not running.", fontSize = 13.sp, color = LocalExtra.current.text3)
        for (ws in desk?.optJSONArray("workspaces").objects()) {
            val wins = ws.optJSONArray("windows").objects()
            if (wins.isEmpty()) continue
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(Modifier.padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    val n = ws.optInt("idx")
                    Text(if (ws.str("name").isNotEmpty()) "Workspace $n · ${ws.str("name")}" else "Workspace $n",
                        fontSize = 14.sp, fontWeight = FontWeight.Medium, color = cs.primary)
                    if (ws.optBoolean("active")) {
                        Spacer(Modifier.width(8.dp))
                        Text("active", fontSize = 11.sp, color = cs.onPrimaryContainer,
                            modifier = Modifier.clip(RoundedCornerShape(6.dp)).background(cs.primaryContainer).padding(horizontal = 6.dp, vertical = 1.dp))
                    }
                }
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(cs.surfaceContainer)) {
                    for (w in wins) WindowRow(w, mode == "window" && targetId == w.optLong("id")) { pick("window", w.optLong("id")) }
                }
            }
        }
        Spacer(Modifier.size(8.dp))
    }
    if (zoom) frame?.let { ZoomBitmap(it) { zoom = false } }
}

@Composable
fun LiveChip(live: Boolean, modifier: Modifier) {
    val ok = TONE["ok"]!!
    Row(
        modifier.clip(RoundedCornerShape(8.dp)).background(if (live) ok.bg else MaterialTheme.colorScheme.surfaceContainerHigh)
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(6.dp).clip(CircleShape).background(if (live) ok.dot else LocalExtra.current.text3))
        Spacer(Modifier.width(6.dp))
        Text(if (live) "Live" else "Paused", fontSize = 12.sp, fontWeight = FontWeight.Medium, color = if (live) ok.fg else LocalExtra.current.text3)
    }
}

@Composable
fun WindowRow(w: JSONObject, on: Boolean, onClick: () -> Unit) {
    val app = w.str("app")
    val tone = appTone(app)
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).background(if (on) cs.surfaceContainerHigh else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(tone.copy(alpha = .16f)), contentAlignment = Alignment.Center) {
            Text(app.take(1).uppercase().ifEmpty { "?" }, color = tone, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Medium, fontSize = 15.sp)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(w.str("title").ifEmpty { app }, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOfNotNull(app, "focused".takeIf { w.optBoolean("focused") }, "needs attention".takeIf { w.optBoolean("urgent") },
                "opened by seimei".takeIf { w.optBoolean("ours") }).joinToString(" · "),
                fontSize = 13.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Icon(Icons.chev, null, Modifier.size(18.dp), tint = LocalExtra.current.text3)
    }
}

/** A frame full screen: pinch to zoom, drag to look around, tap to close. */
@Composable
fun ZoomBitmap(b: ImageBitmap, close: () -> Unit) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    Dialog(close, DialogProperties(usePlatformDefaultWidth = false)) {
        Box(
            Modifier.fillMaxSize().background(Color.Black)
                .pointerInput(Unit) { detectTapGestures(onTap = { close() }, onDoubleTap = { scale = 1f; offset = Offset.Zero }) }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        scale = (scale * zoom).coerceIn(1f, 6f)
                        offset = if (scale == 1f) Offset.Zero else offset + pan
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            Image(b, null, Modifier.fillMaxWidth().graphicsLayer {
                scaleX = scale
                scaleY = scale
                translationX = offset.x
                translationY = offset.y
            }, contentScale = ContentScale.Fit)
        }
    }
}

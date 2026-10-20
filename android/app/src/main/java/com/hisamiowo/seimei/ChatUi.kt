package com.hisamiowo.seimei

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.gestures.detectTapGestures
import kotlinx.coroutines.launch
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlinx.coroutines.delay
import org.json.JSONObject

// ---------- the chat list ----------

@Composable
fun ChatTab(vm: Seimei) {
    // Newest at the bottom: a reversed list keeps the reader pinned to the end while things arrive.
    val state = rememberLazyListState()
    var zoom by remember { mutableStateOf<String?>(null) }
    val rows = vm.items.asReversed()
    val scope = rememberCoroutineScope()
    // The list keeps its first visible row in place when rows are added below it, so after a burst of new rows
    // the first visible index says nothing about where the reader is. Whether they follow the end is judged only
    // while the list scrolls (their finger, or our own jump to the end).
    var follow by remember { mutableStateOf(true) }
    LaunchedEffect(state) {
        snapshotFlow { Triple(state.isScrollInProgress, state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            .collect { (moving, i, off) -> if (moving) follow = i == 0 && off < 160 }
    }
    val newest = vm.items.lastOrNull()
    LaunchedEffect(newest?.key, vm.live.isEmpty(), vm.busy) {
        if (vm.focus != null) return@LaunchedEffect
        // A card waiting for the user's OK always comes into view.
        if (follow || (newest is Item.Approval && newest.outcome == null)) {
            state.scrollToItem(0)
            follow = true
        }
    }
    // Sending always jumps to the end, even from far up.
    LaunchedEffect(vm.sent) {
        if (vm.sent > 0) {
            follow = true
            state.scrollToItem(0)
        }
    }
    // Rows sit after the away card and the live or working row, when those show.
    val lead = listOf(!vm.online && vm.awaySince != null, vm.live.isNotEmpty() || (vm.online && vm.busy && vm.waiting == 0)).count { it }
    LaunchedEffect(vm.focus, rows.size) {
        val id = vm.focus ?: return@LaunchedEffect
        val i = rows.indexOfFirst { it.key == "e$id" }
        if (i >= 0) {
            state.scrollToItem(lead + i)
            vm.focus = null
        }
    }
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = state,
        reverseLayout = true,
        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (!vm.online && vm.awaySince != null) item(key = "away") { Away(vm) }
        if (vm.live.isNotEmpty()) item(key = "live") { Md(vm.live) }
        else if (vm.online && vm.busy && vm.waiting == 0) item(key = "working") { Working() }
        items(rows, key = { it.key }, contentType = { it::class }) { row ->
            Row(row, vm) { zoom = it }
        }
        if (vm.hasOlder) item(key = "older") {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                TextButton({ vm.loadOlder() }, enabled = !vm.loadingOlder) { Text("Load earlier") }
            }
        }
    }
    // Scrolled up: a way back to the end, which also shows that something new may be there.
    if (!follow) SmallFloatingActionButton(
        { scope.launch { state.animateScrollToItem(0); follow = true } },
        Modifier.align(Alignment.BottomEnd).padding(16.dp),
        containerColor = MaterialTheme.colorScheme.secondaryContainer,
    ) { Icon(Icons.down, "Jump to the newest") }
    }
    zoom?.let { Zoom(vm, it) { zoom = null } }
}

@Composable
fun Row(item: Item, vm: Seimei, live: JSONObject? = null, onPicture: (String) -> Unit) {
    when (item) {
        is Item.Day -> NotePill(dayLabel(item.t), "", day = true)
        is Item.Msg -> if (item.role == "user") UserMsg(item, vm, onPicture) else AssistantMsg(item)
        is Item.Note -> NotePill(item.text + if (item.n > 1) "  ×${item.n}" else "", item.level)
        is Item.Activity -> ActivityChip(item, live)
        is Item.Worker -> WorkerCard(item)
        is Item.Approval -> ApprovalCard(item, vm)
        is Item.Sent -> Column(Modifier.fillMaxWidth()) {
            FileChip(vm, item.file, Modifier.widthIn(max = 320.dp), expired = expired(item.t))
            if (item.caption.isNotEmpty()) Text(item.caption, fontSize = 13.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(top = 6.dp))
        }
        is Item.Shown -> Column(Modifier.fillMaxWidth()) {
            // Never more than twice its own size (a 16 px icon stays small, at least 48dp) and at most 480dp tall.
            val density = LocalDensity.current.density
            val maxW = if (item.w > 0) maxOf(48f, 2f * item.w / density).dp else Dp.Infinity
            Picture(vm, item.file, Modifier.widthIn(max = maxW).heightIn(max = 480.dp).clip(RoundedCornerShape(16.dp)).clickable { onPicture(item.file) },
                ratio = if (item.w > 0 && item.h > 0) item.w.toFloat() / item.h else null)
            if (item.caption.isNotEmpty()) Text(item.caption, fontSize = 13.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(top = 6.dp))
        }
    }
}

fun dayLabel(t: Double): String {
    val d = dayOf(t)
    val today = LocalDate.now()
    return when (d) {
        today -> "Today"
        today.minusDays(1) -> "Yesterday"
        else -> d.format(DateTimeFormatter.ofPattern("EEEE, d MMMM"))
    }
}

@Composable
fun NotePill(text: String, level: String, day: Boolean = false) {
    val color = when (level) {
        "error" -> MaterialTheme.colorScheme.error
        "warn" -> LocalExtra.current.warn
        else -> LocalExtra.current.text3
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Text(
            text, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = color, textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp)
                .then(if (day) Modifier else Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surfaceContainer))
                .padding(horizontal = 12.dp, vertical = 4.dp),
        )
    }
}

@Composable
fun Working() {
    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(10.dp))
        Text("Working", fontSize = 14.sp, color = LocalExtra.current.text3)
    }
}

@Composable
fun Away(vm: Seimei) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(16.dp),
    ) {
        Text("The laptop is not answering", fontWeight = FontWeight.SemiBold)
        Text("Since ${clock(vm.awaySince!! / 1000.0)}. It may be asleep or offline; seimei keeps trying.",
            fontSize = 14.sp, color = LocalExtra.current.text2)
    }
}

// ---------- messages ----------

@Composable
fun UserMsg(m: Item.Msg, vm: Seimei, onPicture: (String) -> Unit) {
    var time by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Column(
            Modifier.widthIn(max = 300.dp)
                .clip(RoundedCornerShape(20.dp, 20.dp, 4.dp, 20.dp))
                .background(MaterialTheme.colorScheme.primaryContainer)
                .clickable { time = !time }
                .padding(horizontal = 16.dp, vertical = 11.dp),
            horizontalAlignment = Alignment.End,
        ) {
            if (m.images.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    for (name in m.images.take(3)) {
                        Picture(vm, name, Modifier.size(if (m.images.size == 1) 200.dp else 88.dp).clip(RoundedCornerShape(12.dp))
                            .clickable { onPicture(name) }, crop = true)
                    }
                }
                if (m.text.isNotEmpty() || m.files.isNotEmpty()) Spacer(Modifier.size(8.dp))
            }
            for (f in m.files) {
                FileChip(vm, f, Modifier.fillMaxWidth(), onUser = true, expired = expired(m.t))
                Spacer(Modifier.size(6.dp))
            }
            if (m.text.isNotEmpty()) SelectionContainer {
                Text(m.text, color = MaterialTheme.colorScheme.onPrimaryContainer, lineHeight = 21.sp, fontSize = 15.sp)
            }
            if (time) Text(clock(m.t), fontSize = 12.sp, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = .7f))
        }
    }
}

val KIND_LABEL = mapOf("pdf" to "PDF", "archive" to "Archive", "audio" to "Audio", "video" to "Video", "image" to "Image",
    "code" to "Code", "text" to "Text")

fun kindIcon(kind: String) = when (kind) {
    "archive" -> Icons.archive
    "audio" -> Icons.audio
    "video" -> Icons.video
    "image" -> Icons.photo
    "code" -> Icons.code
    else -> Icons.doc
}

/** A laptop file: first tap saves it to Downloads, then a tap opens it. */
@Composable
fun FileChip(vm: Seimei, f: FileRef, modifier: Modifier = Modifier, onUser: Boolean = false, expired: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    val fg = if (onUser) cs.onPrimaryContainer else cs.onSurface
    val sub = if (onUser) cs.onPrimaryContainer.copy(alpha = .75f) else LocalExtra.current.text3
    val saving = f.id in vm.saving
    val saved = f.id in vm.savedIds
    // A copy already in Downloads still opens after the laptop deleted its file.
    val gone = expired && !saved
    Row(
        modifier.clip(RoundedCornerShape(16.dp))
            .background(if (onUser) cs.onPrimaryContainer.copy(alpha = .1f) else cs.surfaceContainerHigh)
            .alpha(if (gone) .6f else 1f)
            .clickable(enabled = !gone) { vm.fileTap(f) }
            .padding(start = 10.dp, end = 14.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(cs.secondaryContainer), contentAlignment = Alignment.Center) {
            Icon(kindIcon(f.kind), null, Modifier.size(20.dp), tint = cs.onSecondaryContainer)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(f.name, color = fg, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(when {
                gone -> "Expired"
                saving -> "Saving to Downloads…"
                saved -> "In Downloads · tap to open"
                else -> "${sizeText(f.size)} · ${KIND_LABEL[f.kind] ?: "File"}"
            }, color = sub, fontSize = 12.sp)
        }
        Spacer(Modifier.width(8.dp))
        if (gone) Unit
        else if (saving) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = sub)
        else Icon(if (saved) Icons.open else Icons.download, if (saved) "Open" else "Download", Modifier.size(18.dp), tint = sub)
    }
}

@Composable
fun AssistantMsg(m: Item.Msg) {
    var time by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().clickable(indication = null, interactionSource = null) { time = !time }) {
        Md(m.text)
        if (time) Text(clock(m.t), fontSize = 12.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(top = 4.dp))
    }
}

// ---------- markdown ----------

@Composable
fun Md(src: String, size: Int = 15, color: Color = MaterialTheme.colorScheme.onSurface) {
    val colors = MdColors(MaterialTheme.colorScheme.primary, MaterialTheme.colorScheme.surfaceContainerHigh)
    val blocks = remember(src, colors) { Markdown(colors).parse(src) }
    SelectionContainer { Blocks(blocks, size, color) }
}

@Composable
fun Blocks(blocks: List<Block>, size: Int, color: Color) {
    Column(verticalArrangement = Arrangement.spacedBy((size * 0.6f).dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> Text(b.text, fontSize = size.sp, lineHeight = (size * 1.5f).sp, color = color)
            is Block.Heading -> Text(b.text, fontSize = (if (b.level == 1) size + 2 else size + 1).sp, fontWeight = FontWeight.SemiBold,
                color = color, modifier = Modifier.padding(top = 6.dp))
            is Block.Code -> Text(
                b.text, fontFamily = FontFamily.Monospace, fontSize = (size - 2).sp, lineHeight = ((size - 2) * 1.55f).sp,
                color = color, softWrap = false,
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceContainerLowest)
                    .horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 10.dp),
            )
            is Block.Quote -> Row(Modifier.padding(start = 2.dp)) {
                Box(Modifier.width(2.dp).heightIn(min = 20.dp).background(MaterialTheme.colorScheme.outline))
                Spacer(Modifier.width(12.dp))
                Blocks(b.blocks, size, LocalExtra.current.text2)
            }
            is Block.Bullets -> Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                b.items.forEachIndexed { i, it ->
                    Row {
                        Text(if (b.ordered) "${b.start + i}." else "•", fontSize = size.sp, lineHeight = (size * 1.5f).sp, color = color,
                            modifier = Modifier.width(if (b.ordered) 26.dp else 18.dp))
                        Blocks(it, size, color)
                    }
                }
            }
            Block.Rule -> Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).size(1.dp).background(MaterialTheme.colorScheme.surfaceContainerHigh))
            is Block.Table -> Box(Modifier.clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
                .horizontalScroll(rememberScrollState())) {
                Column {
                    TableRow(b.head, head = true, size)
                    for (r in b.rows) {
                        Box(Modifier.fillMaxWidth().size(1.dp).background(MaterialTheme.colorScheme.surfaceContainerHighest))
                        TableRow(r, head = false, size)
                    }
                }
            }
        }
    }
}

@Composable
fun TableRow(cells: List<androidx.compose.ui.text.AnnotatedString>, head: Boolean, size: Int) {
    Row {
        for (c in cells) Text(
            c, fontSize = (size - 1).sp, fontWeight = if (head) FontWeight.SemiBold else null,
            color = if (head) LocalExtra.current.text2 else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.width(140.dp).padding(horizontal = 12.dp, vertical = 8.dp),
        )
    }
}

// ---------- tool activity ----------

fun iconFor(name: String) = when {
    name == "Bash" -> Icons.bash
    name == "Read" -> Icons.read
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> Icons.edit
    name in setOf("Grep", "Glob") -> Icons.search
    name.startsWith("Web") -> Icons.web
    else -> Icons.tool
}

@Composable
fun ActivityChip(a: Item.Activity, live: JSONObject? = null) {
    var open by rememberSaveable(a.key) { mutableStateOf(false) }
    val running = a.calls.any { it.running }
    val id = live?.str("tool_id")
    if (id != null && a.calls.any { it.id == id && it.running }) return LivePill(a, live, open) { open = !open }
    Column(Modifier.animateContentSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(
            Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surfaceContainer).clickable { open = !open }
                .padding(start = 12.dp, end = 10.dp, top = 7.dp, bottom = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (running) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            else Icon(iconFor(a.calls.first().name), null, Modifier.size(16.dp), tint = LocalExtra.current.text3)
            Spacer(Modifier.width(8.dp))
            Text(activityLabel(a.calls), fontSize = 13.sp, color = LocalExtra.current.text2, modifier = Modifier.weight(1f, fill = false),
                maxLines = 1, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.width(4.dp))
            Icon(Icons.chev, null, Modifier.size(14.dp).then(if (open) Modifier.rotate90() else Modifier), tint = LocalExtra.current.text3)
        }
        if (open) Column(Modifier.padding(start = 4.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for (c in a.calls) CallRow(c)
        }
    }
}

/** A group whose last call still runs: what it does and for how long, the command, and its last output lines. */
@Composable
fun LivePill(a: Item.Activity, live: JSONObject, open: Boolean, toggle: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(1000)
            value = System.currentTimeMillis()
        }
    }
    val id = live.str("tool_id")
    val out = live.optJSONArray("output")?.let { o -> List(o.length()) { o.getString(it) } }.orEmpty()
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(cs.surfaceContainer).animateContentSize()) {
        Row(Modifier.fillMaxWidth().clickable(onClick = toggle).padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            Spacer(Modifier.width(8.dp))
            Text(activityLabel(a.calls, id), fontSize = 13.5.sp, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            val since = live.optDouble("since", 0.0)
            if (since > 0) Text(elapsed(since, now), fontSize = 13.sp, color = LocalExtra.current.text2)
            Spacer(Modifier.width(6.dp))
            Icon(Icons.chev, null, Modifier.size(14.dp).then(if (open) Modifier.rotate90() else Modifier), tint = LocalExtra.current.text3)
        }
        Text(live.str("what"), fontFamily = FontFamily.Monospace, fontSize = 11.5.sp, lineHeight = 16.sp,
            modifier = Modifier.padding(start = 14.dp, end = 14.dp, bottom = 8.dp))
        if (out.isNotEmpty()) Text(out.joinToString("\n"), fontFamily = FontFamily.Monospace, fontSize = 11.sp, lineHeight = 16.sp,
            color = LocalExtra.current.text3, maxLines = 6, softWrap = false, overflow = TextOverflow.Clip,
            modifier = Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, bottom = 8.dp).clip(RoundedCornerShape(12.dp))
                .background(cs.surface).padding(horizontal = 10.dp, vertical = 7.dp))
        if (open) Column(Modifier.padding(start = 8.dp, end = 8.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for (c in a.calls) CallRow(c)
        }
    }
}

fun Modifier.rotate90() = this.rotate(90f)

@Composable
fun CallRow(c: Call) {
    var out by remember { mutableStateOf(false) }
    Column {
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).clickable(enabled = !c.running) { out = !out }.padding(vertical = 4.dp, horizontal = 6.dp)) {
            Text(if (c.name.startsWith("mcp__")) c.name.split("__").last() else c.name, fontWeight = FontWeight.SemiBold, fontSize = 13.sp,
                color = if (c.error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface)
            Spacer(Modifier.width(8.dp))
            Text(c.summary, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = LocalExtra.current.text3, maxLines = 1,
                overflow = TextOverflow.Ellipsis)
        }
        if (out && c.out.isNotEmpty()) SelectionContainer {
            Text(c.out.take(4000), fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = LocalExtra.current.text2,
                modifier = Modifier.fillMaxWidth().heightIn(max = 280.dp).clip(RoundedCornerShape(10.dp))
                    .background(MaterialTheme.colorScheme.surfaceContainerLowest).verticalScroll(rememberScrollState()).padding(10.dp))
        }
    }
}

// ---------- cards ----------

@Composable
fun Card(highlight: Boolean = false, content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
            .then(if (highlight) Modifier.border(1.5.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(20.dp)) else Modifier)
            .padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

@Composable
fun CardHead(icon: androidx.compose.ui.graphics.vector.ImageVector, bg: Color, fg: Color, title: String, sub: String?, mono: Boolean = false) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(30.dp).clip(RoundedCornerShape(10.dp)).background(bg), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(18.dp), tint = fg)
        }
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, lineHeight = 18.sp)
            if (!sub.isNullOrEmpty()) Text(sub, fontSize = 12.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis,
                fontFamily = if (mono) FontFamily.Monospace else null)
        }
    }
}

val BAD_BG = Color(0xFF45293A)
val OK_BG = Color(0xFF2B3B33)

@Composable
fun WorkerCard(w: Item.Worker) {
    val cs = MaterialTheme.colorScheme
    val (icon, title) = when (w.event) {
        "summoned" -> Icons.star to "Summoned ${w.name}"
        "done" -> Icons.check to "${w.name} finished"
        "failed" -> Icons.fail to "${w.name} failed"
        else -> Icons.ask to "${w.name} asks"
    }
    val (bg, fg) = when (w.event) {
        "done" -> cs.surfaceContainerHigh to LocalExtra.current.text3
        "failed" -> BAD_BG to cs.error
        else -> cs.primaryContainer to cs.onPrimaryContainer
    }
    var open by rememberSaveable(w.key) { mutableStateOf(false) }
    var clipped by remember { mutableStateOf(false) }
    Card(highlight = w.event == "asking" && w.settled == null) {
        CardHead(icon, bg, fg, title, if (w.event == "summoned") listOf(w.model, w.effort, w.contextCap, home(w.cwd)).filter { it.isNotEmpty() }.joinToString(" · ")
            else w.settled, mono = w.event == "summoned")
        if (w.text.isNotEmpty()) {
            val max = if (open) Int.MAX_VALUE else if (w.event == "summoned") 3 else 6
            if (w.event == "summoned") Text(w.text, fontSize = 14.sp, color = LocalExtra.current.text2, maxLines = max, overflow = TextOverflow.Ellipsis,
                onTextLayout = { if (!open) clipped = it.hasVisualOverflow })
            else Box(Modifier.animateContentSize().heightIn(max = if (open) 10_000.dp else 140.dp)) { Md(w.text, 14, LocalExtra.current.text2) }
            if (clipped || w.event != "summoned" && w.text.length > 400) {
                Text(if (open) "Show less" else "Show more", color = cs.primary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable { open = !open }.padding(vertical = 2.dp))
            }
        }
    }
}

val OUTCOME = mapOf("approved" to "Approved", "denied" to "Denied", "expired" to "No answer in 10 min, denied",
    "restart" to "Cancelled by a seimei restart")

@Composable
fun ApprovalCard(a: Item.Approval, vm: Seimei) {
    val cs = MaterialTheme.colorScheme
    var sent by remember(a.aid) { mutableStateOf(false) }
    val message = a.action == "message"
    val sub = mapOf("message" to "a session you started", "edit" to "a file edit", "desktop" to "a desktop action",
        "discord" to "a Discord action")[a.action]
        ?: if (Regex("niri|window").containsMatchIn(a.target)) "a desktop command" else "a session command"
    val (bg, fg) = when (a.outcome) {
        null -> cs.primaryContainer to cs.onPrimaryContainer
        "approved" -> OK_BG to LocalExtra.current.ok
        else -> BAD_BG to cs.error
    }
    Card(highlight = a.outcome == null) {
        CardHead(Icons.lock, bg, fg, "${a.who} wants to ${if (message) "message ${a.target}" else a.target}", sub)
        Text(
            a.detail, fontSize = 13.sp, lineHeight = 19.sp,
            fontFamily = if (message || a.action == "discord") null else FontFamily.Monospace,
            color = if (a.outcome == null) cs.onSurface else LocalExtra.current.text2,
            maxLines = if (a.outcome == null) Int.MAX_VALUE else 3, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth().heightIn(max = if (a.outcome == null) 160.dp else 80.dp)
                .then(if (message) Modifier else Modifier.clip(RoundedCornerShape(10.dp)).background(cs.surface).padding(horizontal = 10.dp, vertical = 8.dp))
                .verticalScroll(rememberScrollState()),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (a.outcome != null) {
                Text(OUTCOME[a.outcome] ?: a.outcome, fontSize = 13.sp,
                    color = if (a.outcome == "approved") LocalExtra.current.ok else cs.error, modifier = Modifier.weight(1f).padding(vertical = 10.dp))
            } else {
                val left by produceState(minutesLeft(a.expires), a.expires) {
                    while (true) {
                        delay(20_000)
                        value = minutesLeft(a.expires)
                    }
                }
                Text(if (left > 0) "Auto-denies in $left min" else "Expiring", fontSize = 13.sp, color = LocalExtra.current.text3,
                    modifier = Modifier.weight(1f))
                TextButton({ sent = true; vm.decide(a.aid, false) { sent = false } }, enabled = !sent) { Text("Deny") }
                Spacer(Modifier.width(4.dp))
                Button({ sent = true; vm.decide(a.aid, true) { sent = false } }, enabled = !sent) { Text("Approve") }
            }
        }
    }
}

fun minutesLeft(expires: Double) = kotlin.math.ceil((expires - System.currentTimeMillis() / 1000.0) / 60).toInt()

// ---------- pictures ----------

@Composable
fun Picture(vm: Seimei, name: String, modifier: Modifier, crop: Boolean = false, ratio: Float? = null) {
    val bmp by produceState<ImageBitmap?>(null, name) { value = vm.image(name) }
    val b = bmp
    if (b == null) Box(modifier.then(if (crop) Modifier else Modifier.aspectRatio(ratio ?: (4f / 3))).background(MaterialTheme.colorScheme.surfaceContainerHigh))
    else Image(b, null, modifier.then(if (crop) Modifier else Modifier.aspectRatio(b.width.toFloat() / b.height)),
        contentScale = if (crop) ContentScale.Crop else ContentScale.Fit)
}

/** A picture full screen: the thumbnail at once, the original once loaded. Pinch or double-tap to zoom, drag to
 *  look around, tap to close (when not zoomed); Save puts the original in Pictures/seimei. */
@Composable
fun Zoom(vm: Seimei, name: String, close: () -> Unit) {
    val thumb by produceState<ImageBitmap?>(null, name) { value = vm.image(name) }
    val full by produceState<ImageBitmap?>(null, name) { value = vm.image(name, full = true) }
    val b = full ?: thumb
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var size by remember { mutableStateOf(IntSize.Zero) }
    // Keeps the zoomed picture over the screen: no dragging it off into the black.
    fun clamp(o: Offset, s: Float) = Offset(
        o.x.coerceIn(-size.width * (s - 1) / 2, size.width * (s - 1) / 2),
        o.y.coerceIn(-size.height * (s - 1) / 2, size.height * (s - 1) / 2),
    )
    Dialog(close, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Box(
            Modifier.fillMaxSize().background(Color.Black).onSizeChanged { size = it }
                .pointerInput(Unit) {
                    detectTapGestures(onTap = { if (scale == 1f) close() }, onDoubleTap = { p ->
                        if (scale > 1f) {
                            scale = 1f
                            offset = Offset.Zero
                        } else {
                            scale = 2.5f
                            offset = clamp((Offset(size.width / 2f, size.height / 2f) - p) * 1.5f, 2.5f)
                        }
                    })
                }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        scale = (scale * zoom).coerceIn(1f, 8f)
                        offset = if (scale == 1f) Offset.Zero else clamp(offset + pan, scale)
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            if (b == null) CircularProgressIndicator(color = Color.White)
            else Image(b, null, Modifier.fillMaxSize().graphicsLayer {
                scaleX = scale
                scaleY = scale
                translationX = offset.x
                translationY = offset.y
            }, contentScale = ContentScale.Fit)
            Row(Modifier.align(Alignment.TopEnd).statusBarsPadding().padding(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val saving = name in vm.picturesSaving
                val saved = name in vm.picturesSaved
                FilledTonalIconButton({ vm.savePicture(name) }, enabled = !saving && !saved) {
                    when {
                        saving -> CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        saved -> Icon(Icons.check, "Saved to Pictures/seimei", Modifier.size(20.dp))
                        else -> Icon(Icons.download, "Save the original", Modifier.size(20.dp))
                    }
                }
                FilledTonalIconButton(close) { Icon(Icons.close, "Close", Modifier.size(20.dp)) }
            }
        }
    }
}

// ---------- composer ----------

@Composable
fun Composer(vm: Seimei) {
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Seimei.MAX_IMAGES)) { vm.attach(it) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.attachFiles(it) }
    var menu by remember { mutableStateOf(false) }
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 8.dp)) {
        if (vm.attachments.isNotEmpty()) Row(Modifier.padding(bottom = 8.dp).horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (a in vm.attachments) Box {
                when (a) {
                    is Attachment.Photo -> Image(a.preview, null, Modifier.size(64.dp).clip(RoundedCornerShape(12.dp)), contentScale = ContentScale.Crop)
                    is Attachment.Doc -> Row(
                        Modifier.height(64.dp).widthIn(max = 200.dp).clip(RoundedCornerShape(12.dp)).background(cs.surfaceContainerHigh)
                            .padding(start = 12.dp, end = 30.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        if (a.uploading) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        else Icon(Icons.doc, null, Modifier.size(20.dp), tint = LocalExtra.current.text2)
                        Spacer(Modifier.width(8.dp))
                        Text(a.name, fontSize = 13.sp, color = LocalExtra.current.text2, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                }
                Box(Modifier.align(Alignment.TopEnd).padding(2.dp).size(22.dp).clip(CircleShape).background(Color.Black.copy(alpha = .6f))
                    .clickable { vm.detach(a) }, contentAlignment = Alignment.Center) {
                    Icon(Icons.close, "Remove", Modifier.size(12.dp), tint = Color.White)
                }
            }
        }
        Row(verticalAlignment = Alignment.Bottom) {
            TextField(
                value = vm.draft,
                onValueChange = vm::edit,
                placeholder = { Text("Message seimei") },
                leadingIcon = {
                    Box {
                        IconButton({ menu = true }) { Icon(Icons.attach, "Attach") }
                        DropdownMenu(menu, { menu = false }) {
                            DropdownMenuItem({ Text("Photos") }, {
                                menu = false
                                pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                            }, leadingIcon = { Icon(Icons.photo, null) })
                            DropdownMenuItem({ Text("Files") }, {
                                menu = false
                                pickFiles.launch(arrayOf("*/*"))
                            }, leadingIcon = { Icon(Icons.doc, null) })
                        }
                    }
                },
                maxLines = 7,
                shape = RoundedCornerShape(28.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ),
                modifier = Modifier.weight(1f).heightIn(min = 56.dp),
            )
            Spacer(Modifier.width(8.dp))
            val typing = vm.draft.isNotBlank() || vm.attachments.isNotEmpty()
            if (vm.busy && !typing) {
                FilledTonalIconButton(vm::interrupt, Modifier.size(56.dp)) {
                    Box(Modifier.size(14.dp).clip(RoundedCornerShape(3.dp)).background(cs.onSecondaryContainer))
                }
            } else {
                FilledIconButton(vm::send, Modifier.size(56.dp), enabled = typing && vm.online,
                    colors = IconButtonDefaults.filledIconButtonColors()) {
                    Icon(Icons.send, "Send", Modifier.size(22.dp))
                }
            }
        }
    }
}

package com.hisamiowo.seimei

import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.draw.shadow
import androidx.compose.runtime.DisposableEffect
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.animation.core.animate
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.ui.graphics.Brush
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.ButtonDefaults
import androidx.compose.ui.graphics.Shape
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
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
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
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
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.graphics.SolidColor

// ---------- the chat list ----------

@Composable
fun ChatTab(vm: Seimei, lane: String) {
    // Newest at the bottom: a reversed list keeps the reader pinned to the end while things arrive.
    val state = rememberLazyListState()
    var zoom by remember { mutableStateOf<String?>(null) }
    val traj = lane == "t"
    val rows = (if (traj) vm.trail else vm.items).asReversed()
    val scope = rememberCoroutineScope()
    // The list keeps its first visible row in place when rows are added below it, so after a burst of new rows
    // the first visible index says nothing about where the reader is. Whether they follow the end is judged only
    // while the list scrolls (their finger, or our own jump to the end).
    var follow by remember { mutableStateOf(true) }
    LaunchedEffect(state) {
        snapshotFlow { Triple(state.isScrollInProgress, state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            .collect { (moving, i, off) -> if (moving) follow = i == 0 && off < 160 }
    }
    val newest = rows.firstOrNull()
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
    val liveHere = vm.live.isNotEmpty() && vm.liveLane == lane
    val lead = listOf(!vm.online && vm.awaySince != null, liveHere || (vm.online && vm.busy && vm.waiting == 0)).count { it } +
        if (traj) 0 else vm.pending.size
    // A tapped notification names an event; it may sit in either lane.
    LaunchedEffect(vm.focus, rows.size) {
        if (lane != vm.lane) return@LaunchedEffect
        val id = vm.focus ?: return@LaunchedEffect
        val i = rows.indexOfFirst { it.key == "e$id" }
        if (i >= 0) {
            state.scrollToItem(lead + i)
            vm.focus = null
        } else if ((if (traj) vm.items else vm.trail).any { it.key == "e$id" }) vm.showLane(if (traj) "c" else "t")
    }
    // A fold line or an anchor was tapped: go to the row it names and flash it.
    var flashing by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(vm.goto) {
        // Only the lane on screen takes it (the one sliding out doesn't)
        if (lane != vm.lane) return@LaunchedEffect
        val key = vm.goto ?: return@LaunchedEffect
        vm.goto = null
        val i = rows.indexOfFirst { it.key == key }
        if (i < 0) return@LaunchedEffect
        follow = false
        state.scrollToItem(lead + i, -200)
        flashing = key
        delay(1400)
        flashing = null
    }
    // Each lane comes back where it was left; at its newest row if the reader was following then (BEHAVIOR.md 8)
    LaunchedEffect(Unit) {
        if (vm.goto != null) return@LaunchedEffect
        val spot = vm.laneSpot[lane]
        if (spot == null || spot.third) {
            state.scrollToItem(0)
            follow = true
        } else {
            state.scrollToItem(spot.first, spot.second)
            follow = false
        }
    }
    // Going away brings the reader to the end, where the away block says so (as the web does)
    val away = !vm.online && vm.awaySince != null
    LaunchedEffect(away) { if (away) state.animateScrollToItem(0) }
    DisposableEffect(Unit) { onDispose { vm.laneSpot[lane] = Triple(state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset, follow) } }
    LaunchedEffect(state) { snapshotFlow { state.canScrollBackward }.collect { vm.lifted = it } }
    // Rows that came in below since the reader left the end: the jump button counts them
    var leftAt by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(follow) { leftAt = if (follow) null else rows.firstOrNull()?.key }
    val missed = leftAt?.let { k -> rows.indexOfFirst { it.key == k }.takeIf { it > 0 } } ?: 0
    Column(Modifier.fillMaxSize()) {
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = state,
        reverseLayout = true,
        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (!vm.online && vm.awaySince != null) item(key = "away") { Away(vm) }
        if (liveHere) item(key = "live") { Md(vm.live) }
        else if (vm.online && vm.busy && vm.waiting == 0) item(key = "working") { Working() }
        if (!traj) items(vm.pending.asReversed(), key = { "p" + it.cid }) { PendingMsg(it, vm) }
        items(rows, key = { it.key }, contentType = { it::class }) { row ->
            Box(if (row.key == flashing) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(16.dp)) else Modifier) {
                val quote = row.quote(if (traj) "t" else "c")
                if (quote != null) SwipeReply({ vm.replyIn(quote) }) { Row(row, vm) { zoom = it } }
                else Row(row, vm) { zoom = it }
            }
        }
        if (vm.hasOlder) item(key = "older") {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                TextButton({ vm.loadOlder() }, enabled = !vm.loadingOlder) { Text("Load earlier") }
            }
        }
    }
    // An empty chat greets instead of showing a blank sheet; an empty trajectory says what will show.
    if (traj && vm.trail.isEmpty() && !vm.hasOlder) Text("Nothing yet",
        Modifier.align(Alignment.Center).padding(horizontal = 48.dp), fontSize = 14.sp, color = LocalExtra.current.text3, textAlign = TextAlign.Center)
    // Scrolled up: a way back, with how many rows came in below. An unanswered card above the view comes first.
    if (!follow) {
        val cs = MaterialTheme.colorScheme
        Row(
            Modifier.align(Alignment.BottomEnd).padding(16.dp).height(48.dp).shadow(3.dp, RoundedCornerShape(16.dp)).clip(RoundedCornerShape(16.dp))
                .background(cs.secondaryContainer)
                .clickable {
                    scope.launch {
                        val open = rows.indexOfFirst { (it is Item.Approval && it.outcome == null) || (it is Item.Worker && it.event == "asking" && it.settled == null) }
                        val last = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
                        if (open >= 0 && lead + open > last) state.animateScrollToItem(lead + open)
                        else {
                            state.animateScrollToItem(0)
                            follow = true
                        }
                    }
                }
                .semantics(mergeDescendants = true) { contentDescription = if (missed > 0) "Jump to latest, $missed new" else "Jump to latest" }
                .padding(start = 14.dp, end = 18.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(Icons.down, null, Modifier.size(20.dp), tint = cs.onSecondaryContainer)
            Text("Latest", fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = cs.onSecondaryContainer)
            if (missed > 0) Box(Modifier.heightIn(min = 20.dp).clip(CircleShape).background(cs.primary).padding(horizontal = 6.dp), contentAlignment = Alignment.Center) {
                Text("$missed", fontSize = 12.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, color = cs.onPrimary)
            }
        }
    }
    }
    }
    zoom?.let { Zoom(vm, it) { zoom = null } }
}

/** Chat | Trajectory | Tasks: the conversation, what happened on its own, and seimei's board. */
@Composable
fun LaneTabs(vm: Seimei) {
    Row(Modifier.fillMaxWidth().background(barColor(vm)).padding(start = 12.dp, end = 12.dp, bottom = 2.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        LaneTab("Chat", vm.lane == "c", 0) { vm.showLane("c") }
        LaneTab("Trajectory", vm.lane == "t", vm.unseen) { vm.showLane("t") }
        LaneTab("Tasks", vm.lane == "k", 0) { vm.showLane("k") }
    }
}

@Composable
fun LaneTab(label: String, on: Boolean, count: Int, click: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(
        Modifier.width(IntrinsicSize.Max).clip(RoundedCornerShape(12.dp)).clickable(onClick = click)
            .semantics(mergeDescendants = true) {
                role = Role.Tab
                selected = on
                if (count > 0) stateDescription = "$count new"
            }
            .padding(horizontal = 12.dp),
    ) {
        Row(Modifier.height(41.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(label, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = if (on) cs.onSurface else LocalExtra.current.text3)
            if (count > 0) {
                Spacer(Modifier.width(8.dp))
                Box(Modifier.heightIn(min = 18.dp).widthIn(min = 18.dp).clip(RoundedCornerShape(9.dp)).background(cs.primary).padding(horizontal = 5.dp),
                    contentAlignment = Alignment.Center) {
                    Text(if (count > 99) "99+" else "$count", fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = cs.onPrimary)
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(3.dp).clip(RoundedCornerShape(topStart = 3.dp, topEnd = 3.dp)).background(if (on) cs.primary else Color.Transparent))
    }
}

/** A chip quoting the message this one answers; a tap goes to it. */
@Composable
fun AnchorChip(a: Anchor, vm: Seimei, onUser: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    val fg = if (onUser) cs.onPrimaryContainer else LocalExtra.current.text3
    Row(
        Modifier.padding(bottom = 6.dp).clip(RoundedCornerShape(10.dp))
            .background(if (onUser) cs.onPrimaryContainer.copy(alpha = .12f) else cs.surfaceContainer)
            .clickable { vm.showLane(a.lane, a.goto) }.padding(start = 8.dp, end = 10.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.reply, null, Modifier.size(14.dp), tint = fg.copy(alpha = .8f))
        Spacer(Modifier.width(6.dp))
        Text(a.text, fontSize = 12.5.sp, color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Swipe left to reply: the row follows the finger, the reply icon shows behind it, and past the mark it replies. */
@Composable
fun SwipeReply(onReply: () -> Unit, content: @Composable () -> Unit) {
    // How far the row has moved left.
    var dx by remember { mutableFloatStateOf(0f) }
    val mark = with(LocalDensity.current) { 64.dp.toPx() }
    val haptic = LocalHapticFeedback.current
    Box(Modifier.fillMaxWidth().draggable(
        rememberDraggableState { d ->
            val was = dx
            dx = (dx - d).coerceIn(0f, mark * 1.3f)
            if (was < mark && dx >= mark) haptic.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
        },
        Orientation.Horizontal,
        onDragStopped = {
            if (dx >= mark) onReply()
            animate(dx, 0f) { v, _ -> dx = v }
        },
    )) {
        Icon(Icons.reply, "Reply", Modifier.align(Alignment.CenterEnd).padding(end = 4.dp).size(22.dp)
            .alpha((dx / mark).coerceIn(0f, 1f)), tint = MaterialTheme.colorScheme.primary)
        // The swipe has a spoken twin for TalkBack (BEHAVIOR.md 6)
        Box(Modifier.offset { IntOffset(-dx.roundToInt(), 0) }.semantics {
            customActions = listOf(CustomAccessibilityAction("Reply") { onReply(); true })
        }) { content() }
    }
}

/** A run of trajectory events, as one line in the chat. */
@Composable
fun FoldPill(f: Item.Fold, vm: Seimei) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(28.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
            .clickable { vm.showLane("t", f.goto) }.padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(f.text, Modifier.weight(1f), fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3,
            maxLines = 1, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.width(12.dp))
        Text("Trajectory", fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = MaterialTheme.colorScheme.primary)
    }
}

@Composable
fun TrigRow(t: Item.Trig) {
    // final/: the line alone, no dot
    Text(t.text, Modifier.padding(top = 8.dp), fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium, color = LocalExtra.current.text3)
}

@Composable
fun Row(item: Item, vm: Seimei, live: JSONObject? = null, onPicture: (String) -> Unit) {
    when (item) {
        is Item.Day -> NotePill(dayLabel(item.t), "", day = true)
        is Item.Msg -> if (item.role == "user") UserMsg(item, vm, onPicture) else AssistantMsg(item, vm)
        is Item.Fold -> FoldPill(item, vm)
        is Item.Trig -> TrigRow(item)
        is Item.Note -> NotePill(item.text + if (item.n > 1) "  ×${item.n}" else "", item.level)
        is Item.Activity -> ActivityChip(item, live)
        is Item.Worker -> WorkerCard(item, vm)
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
    // final/: a note is a centred line, no pill; a usage warning reads in text-2, an error in the error colour
    val color = when (level) {
        "error" -> MaterialTheme.colorScheme.error
        "warn" -> LocalExtra.current.text2
        else -> LocalExtra.current.text3
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Text(
            text, fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium, color = color, textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp).padding(horizontal = 12.dp, vertical = 4.dp),
        )
    }
}

@Composable
fun Working() {
    Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Loader(40.dp)
        Spacer(Modifier.width(12.dp))
        Text("Working…", fontSize = 14.sp, fontWeight = FontWeight.Medium, color = LocalExtra.current.text2)
    }
}

@Composable
fun Away(vm: Seimei) {
    // final/ 17: centred, the line at 22, when and what next at 16 in text-2, then Try now on tonal
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Text("Can't reach the server", fontSize = 22.sp, lineHeight = 34.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center)
        Text("Since ${clock(vm.awaySince!! / 1000.0)}. Reconnects on its own.", Modifier.padding(top = 0.dp),
            fontSize = 16.sp, lineHeight = 24.sp, color = LocalExtra.current.text2, textAlign = TextAlign.Center)
        Text("Try now", Modifier.heightIn(min = 48.dp).clip(CircleShape).background(LocalTokens.current.tonal).clickable { vm.retryNow() }
            .padding(horizontal = 24.dp, vertical = 14.dp), fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium,
            color = LocalTokens.current.onTonal)
    }
}

// ---------- messages ----------

@Composable
fun UserMsg(m: Item.Msg, vm: Seimei, onPicture: (String) -> Unit) {
    var time by remember { mutableStateOf(false) }
    // At most 88% of the lane, as the web's bubbles (final/ 02, 09)
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) { Box(Modifier.fillMaxWidth(.88f), contentAlignment = Alignment.CenterEnd) {
        Column(
            Modifier
                .clip(RoundedCornerShape(20.dp, 20.dp, 4.dp, 20.dp))
                .background(MaterialTheme.colorScheme.primaryContainer)
                .clickable { time = !time }
                .padding(horizontal = 16.dp, vertical = 11.dp),
            horizontalAlignment = Alignment.End,
        ) {
            m.anchor?.let { AnchorChip(it, vm, onUser = true) }
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
                Text(m.text, color = MaterialTheme.colorScheme.onPrimaryContainer, lineHeight = 23.2.sp, fontSize = 16.sp)
            }
            if (time) Text(clock(m.t), fontSize = 12.sp, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = .7f))
        }
    } }
}

/** A sent message the server has not echoed yet: dimmed, or outlined with a retry when it failed. */
@Composable
fun PendingMsg(p: Pending, vm: Seimei) {
    val cs = MaterialTheme.colorScheme
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) { Box(Modifier.fillMaxWidth(.88f), contentAlignment = Alignment.CenterEnd) {
        Column(
            Modifier.alpha(if (p.failed) 1f else .6f)
                .clip(RoundedCornerShape(20.dp, 20.dp, 4.dp, 20.dp))
                .background(cs.primaryContainer)
                .then(if (p.failed) Modifier.border(1.5.dp, cs.error, RoundedCornerShape(20.dp, 20.dp, 4.dp, 20.dp)) else Modifier)
                .padding(horizontal = 16.dp, vertical = 11.dp),
            horizontalAlignment = Alignment.End,
        ) {
            p.reply?.let { AnchorChip(Anchor(anchorText(it.text), "e${it.id}", it.lane), vm, onUser = true) }
            val photos = p.sending.filterIsInstance<Attachment.Photo>()
            if (photos.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    for (ph in photos.take(3)) Image(ph.preview, null,
                        Modifier.size(if (photos.size == 1) 200.dp else 88.dp).clip(RoundedCornerShape(12.dp)), contentScale = ContentScale.Crop)
                }
                if (p.text.isNotEmpty()) Spacer(Modifier.size(8.dp))
            }
            for (d in p.sending.filterIsInstance<Attachment.Doc>()) {
                Text(d.name, fontSize = 13.sp, color = cs.onPrimaryContainer, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.size(6.dp))
            }
            if (p.text.isNotEmpty()) Text(p.text, color = cs.onPrimaryContainer, lineHeight = 23.2.sp, fontSize = 16.sp)
            if (p.failed) Text("Not sent. Tap to retry", fontSize = 13.sp, fontWeight = FontWeight.Medium, color = cs.error,
                modifier = Modifier.padding(top = 4.dp).clip(RoundedCornerShape(8.dp)).clickable { vm.retry(p) }.padding(vertical = 12.dp, horizontal = 4.dp))
        }
    } }
}

val KIND_LABEL = mapOf("pdf" to "PDF", "archive" to "Zip", "audio" to "Audio", "video" to "Video", "image" to "Image",
    "code" to "Code", "text" to "Text")

fun kindIcon(kind: String) = when (kind) {
    "archive" -> Icons.archive
    "audio" -> Icons.audio
    "video" -> Icons.video
    "image" -> Icons.photo
    "code" -> Icons.code
    else -> Icons.doc
}

/** A server file: first tap saves it to Downloads, then a tap opens it. */
@Composable
fun FileChip(vm: Seimei, f: FileRef, modifier: Modifier = Modifier, onUser: Boolean = false, expired: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    val fg = if (onUser) cs.onPrimaryContainer else cs.onSurface
    val sub = if (onUser) cs.onPrimaryContainer.copy(alpha = .75f) else LocalExtra.current.text3
    val saving = f.id in vm.saving
    val saved = f.id in vm.savedIds
    // A copy already in Downloads still opens after the server deleted its file.
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
fun AssistantMsg(m: Item.Msg, vm: Seimei) {
    var time by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().clickable(indication = null, interactionSource = null) { time = !time }) {
        m.anchor?.let { AnchorChip(it, vm) }
        Md(m.text)
        if (time) Text(clock(m.t), fontSize = 12.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(top = 4.dp))
        if (m.reply > 0) Text("Reply in chat", Modifier.padding(top = 11.dp).heightIn(min = 48.dp).clip(CircleShape).background(LocalTokens.current.tonal)
            .clickable { vm.replyIn(ReplyTo(m.reply, m.text, "t")) }.padding(horizontal = 16.dp, vertical = 14.dp),
            fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = LocalTokens.current.onTonal)
    }
}

// ---------- markdown ----------

@Composable
fun Md(src: String, size: Int = 16, color: Color = MaterialTheme.colorScheme.onSurface) {
    val colors = MdColors(MaterialTheme.colorScheme.primary, MaterialTheme.colorScheme.surfaceContainerHigh)
    val blocks = remember(src, colors) { Markdown(colors).parse(src) }
    SelectionContainer { Blocks(blocks, size, color) }
}

@Composable
fun Blocks(blocks: List<Block>, size: Int, color: Color) {
    Column(verticalArrangement = Arrangement.spacedBy((size * 0.6f).dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> Text(b.text, fontSize = size.sp, lineHeight = (size * 1.5f).sp, color = color)
            // ### and below at the text's own size (final/ 11: "Plan" at 16 / 600 / 24)
            is Block.Heading -> Text(b.text, fontSize = (when (b.level) { 1 -> size + 4; 2 -> size + 2; else -> size }).sp,
                lineHeight = (size * 1.5f).sp, fontWeight = FontWeight.SemiBold, color = color, modifier = Modifier.padding(top = 6.dp))
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
                    // The web's outside markers: a 1.4 em indent, the marker right-aligned in it
                    Row {
                        Text(if (b.ordered) "${b.start + i}." else "•", fontSize = size.sp, lineHeight = (size * 1.5f).sp, color = color,
                            textAlign = TextAlign.End, modifier = Modifier.width((size * 1.4f).dp).padding(end = 5.dp))
                        Blocks(it, size, color)
                    }
                }
            }
            Block.Rule -> Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).size(1.dp).background(MaterialTheme.colorScheme.surfaceContainerHigh))
            is Block.Table -> androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth()) {
                val scroll = rememberScrollState()
                // At least the width of the text (final/ 11): the columns share it, 140dp each at the least
                val cols = maxOf(1, b.head.size, b.rows.maxOfOrNull { it.size } ?: 0)
                val cell = maxOf(140.dp, maxWidth / cols)
                // A shade at an edge with more columns beyond it, so a cut on a column edge does not look complete.
                Box(Modifier.clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).height(IntrinsicSize.Min)) {
                    Box(Modifier.horizontalScroll(scroll)) {
                        // Max intrinsic width, so the row dividers span the whole table.
                        Column(Modifier.width(IntrinsicSize.Max)) {
                            TableRow(b.head, head = true, size, cell)
                            for (r in b.rows) {
                                Box(Modifier.fillMaxWidth().size(1.dp).background(MaterialTheme.colorScheme.surfaceContainerHighest))
                                TableRow(r, head = false, size, cell)
                            }
                        }
                    }
                    val shade = Color.Black.copy(alpha = .45f)
                    if (scroll.canScrollForward) Box(Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(16.dp)
                        .background(Brush.horizontalGradient(listOf(Color.Transparent, shade))))
                    if (scroll.canScrollBackward) Box(Modifier.align(Alignment.CenterStart).fillMaxHeight().width(16.dp)
                        .background(Brush.horizontalGradient(listOf(shade, Color.Transparent))))
                }
            }
        }
    }
}

@Composable
fun TableRow(cells: List<androidx.compose.ui.text.AnnotatedString>, head: Boolean, size: Int, cell: androidx.compose.ui.unit.Dp = 140.dp) {
    Row {
        for (c in cells) Text(
            c, fontSize = (size - 2).sp, lineHeight = 20.sp, fontWeight = if (head) FontWeight.SemiBold else null,
            color = if (head) LocalExtra.current.text2 else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.width(cell).padding(horizontal = 12.dp, vertical = 10.dp),
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
    // A failure shows on the collapsed chip too, not only in its row.
    val failed = a.calls.count { it.error }
    val err = MaterialTheme.colorScheme.error
    Column(Modifier.animateContentSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        // final/ 09: a 48 chip with 12 corners, the tool's icon (the shape loader while it runs), 14sp, a 20 chevron
        Row(
            Modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp))
                .background(if (failed > 0) err.copy(alpha = .14f).compositeOver(MaterialTheme.colorScheme.surfaceContainer) else MaterialTheme.colorScheme.surfaceContainer)
                .clickable { open = !open }
                .padding(start = if (running) 8.dp else 12.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (running) Loader(32.dp, contained = false)
            else Icon(iconFor(a.calls.first().name), null, Modifier.size(16.dp), tint = if (failed > 0) err else LocalExtra.current.text2)
            Spacer(Modifier.width(8.dp))
            Text(activityLabel(a.calls) + if (failed > 0) " · $failed failed" else "", fontSize = 14.sp, lineHeight = 20.sp,
                color = if (failed > 0) err else LocalExtra.current.text2, modifier = Modifier.weight(1f, fill = false),
                maxLines = 1, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.width(8.dp))
            Icon(Icons.chev, null, Modifier.size(20.dp).then(if (open) Modifier.rotate90() else Modifier), tint = LocalExtra.current.text3)
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
        Row(Modifier.fillMaxWidth().clickable(onClick = toggle).padding(start = 6.dp, end = 12.dp, top = 4.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Loader(32.dp, contained = false)
            Spacer(Modifier.width(8.dp))
            Text(activityLabel(a.calls, id), fontSize = 13.5.sp, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            val since = live.optDouble("since", 0.0)
            if (since > 0) Text(elapsed(since, now), fontSize = 13.sp, color = LocalExtra.current.text2, style = TNUM)
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
fun Card(highlight: Boolean = false, corner: Dp = 28.dp, pad: Dp = 16.dp, content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(corner)).background(MaterialTheme.colorScheme.surfaceContainer)
            .then(if (highlight) Modifier.border(1.5.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(corner)) else Modifier)
            .padding(pad),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) { content() }
}

@Composable
fun CardHead(icon: androidx.compose.ui.graphics.vector.ImageVector, bg: Color, fg: Color, title: String, sub: String?, mono: Boolean = false, big: Boolean = false) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(if (big) 40.dp else 30.dp).clip(RoundedCornerShape(if (big) 12.dp else 10.dp)).background(bg), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(if (big) 20.dp else 18.dp), tint = fg)
        }
        Spacer(Modifier.width(if (big) 12.dp else 10.dp))
        Column(Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.SemiBold, fontSize = if (big) 16.sp else 14.sp, lineHeight = if (big) 21.sp else 18.sp)
            if (!sub.isNullOrEmpty()) Text(sub, fontSize = 12.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis,
                fontFamily = if (mono) FontFamily.Monospace else null)
        }
    }
}

val BAD_BG: Color @Composable get() = LocalTokens.current.errorContainer
val OK_BG: Color @Composable get() = LocalTokens.current.okContainer

/** A quiet line naming a shikigami, one tap from its page: what seimei's own sentence above didn't say. */
@Composable
fun EvLine(name: String, rest: String, small: String, failed: Boolean = false, shape: Shape = RoundedCornerShape(12.dp),
    modifier: Modifier = Modifier, fill: Boolean = failed, open: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val fg = if (failed) cs.onErrorContainer else LocalExtra.current.text2
    Row(
        modifier.heightIn(min = 48.dp).clip(shape).background(if (failed) cs.errorContainer else cs.surfaceContainer).clickable(onClick = open)
            .padding(start = if (failed) 20.dp else 16.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val line = androidx.compose.ui.text.buildAnnotatedString {
            pushStyle(androidx.compose.ui.text.SpanStyle(color = if (failed) cs.onErrorContainer else cs.onSurface, fontWeight = FontWeight.Medium))
            append(name); pop()
            append(rest)
            if (small.isNotEmpty() && !failed) {
                pushStyle(androidx.compose.ui.text.SpanStyle(fontSize = 12.sp, color = LocalExtra.current.text3)); append("  $small"); pop()
            }
        }
        // A full-width line (a finished or failed shikigami) keeps its chevron at the end (final/ 05)
        Column(if (fill) Modifier.weight(1f) else Modifier.weight(1f, fill = false)) {
            Text(line, fontSize = 14.sp, lineHeight = 20.sp, color = fg)
            if (failed && small.isNotEmpty()) Text(small, fontSize = 12.sp, lineHeight = 16.sp, color = fg.copy(alpha = .85f), modifier = Modifier.padding(top = 2.dp))
        }
        Spacer(Modifier.width(8.dp))
        Icon(Icons.chev, null, Modifier.size(20.dp), tint = if (failed) cs.onErrorContainer else LocalExtra.current.text3)
    }
}

/** "opus" reads as the product name it is: "Opus · high". */
fun modelLine(w: Item.Worker) = listOf(w.model.replaceFirstChar { it.uppercase() }, w.effort).filter { it.isNotEmpty() }.joinToString(" · ")

/** The first sentence of a failure, without its period. */
fun firstSentence(t: String) = t.lineSequence().firstOrNull { it.isNotBlank() }.orEmpty().trim().split(Regex("(?<=[.!?])\\s")).first().removeSuffix(".")

@Composable
fun WorkerCard(w: Item.Worker, vm: Seimei) {
    val open = { vm.openWorker(w.worker) }
    when (w.event) {
        "summoned" -> EvLine(w.name, "", modelLine(w), open = open)
        "failed" -> EvLine(w.name, " failed", firstSentence(w.text), failed = true, shape = RoundedCornerShape(28.dp), modifier = Modifier.fillMaxWidth(), open = open)
        "done" -> Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            EvLine(w.name, " finished", "", shape = RoundedCornerShape(28.dp, 28.dp, 4.dp, 4.dp), modifier = Modifier.fillMaxWidth(), fill = true, open = open)
            if (w.text.isNotEmpty()) Clipped(w.key, RoundedCornerShape(4.dp, 4.dp, 28.dp, 28.dp), MaterialTheme.colorScheme.surfaceContainer) {
                Md(w.text, 14, MaterialTheme.colorScheme.onSurface)
            }
        }
        else -> QuestionCard(w, open)
    }
}

/** A long body clips with a fade and "Show more": measured, since large text clips far below 400 characters. */
@Composable
fun Clipped(key: String, shape: Shape, bg: Color, content: @Composable () -> Unit) {
    val cs = MaterialTheme.colorScheme
    var open by rememberSaveable(key) { mutableStateOf(false) }
    var full by remember { mutableIntStateOf(0) }
    var shown by remember { mutableIntStateOf(0) }
    Column(Modifier.fillMaxWidth().clip(shape).background(bg).padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 14.dp)) {
        Box(Modifier.animateContentSize().heightIn(max = if (open) 10_000.dp else 200.dp).clipToBounds().onSizeChanged { shown = it.height }) {
            Box(Modifier.wrapContentHeight(Alignment.Top, unbounded = true).onSizeChanged { full = it.height }) { content() }
            if (!open && full > shown) {
                Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(28.dp).background(Brush.verticalGradient(listOf(Color.Transparent, bg))))
            }
        }
        if (open || full > shown) {
            Text(if (open) "Show less" else "Show more", color = cs.primary, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                modifier = Modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(8.dp)).clickable { open = !open }.padding(vertical = 14.dp, horizontal = 4.dp))
        }
    }
}

/** A question: a decision, so the warm card, the asker's name as its title, the answer one tap away. */
@Composable
fun QuestionCard(w: Item.Worker, open: () -> Unit) {
    val t = LocalTokens.current
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(t.warnContainer).clickable(onClick = open).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            ShapeTile(w.name, 40.dp, t.warn) {
                Icon(Icons.question, null, Modifier.size(22.dp), tint = t.onWarn)
            }
            Spacer(Modifier.width(12.dp))
            Text(w.name, fontSize = 16.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold, color = t.onWarnContainer, modifier = Modifier.weight(1f))
            w.settled?.let { Text(it, fontSize = 12.sp, color = t.onWarnContainer.copy(alpha = .8f)) }
        }
        if (w.text.isNotEmpty()) Md(w.text, 14, t.onWarnContainer)
        if (w.settled == null) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(open, Modifier.weight(1f).heightIn(min = 48.dp), colors = ButtonDefaults.buttonColors(
                containerColor = t.warn.copy(alpha = .16f).compositeOver(t.warnContainer), contentColor = t.onWarnContainer)) {
                Text("Open ${w.name}", fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Button(open, Modifier.weight(1.4f).heightIn(min = 48.dp), colors = ButtonDefaults.buttonColors(containerColor = t.warn, contentColor = t.onWarn)) {
                Text("Answer", fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

val OUTCOME = mapOf("approved" to "Allowed", "denied" to "Denied", "expired" to "No answer in time, denied",
    "restart" to "Canceled by a restart", "cancelled" to "Canceled")

/** What a settled card says: the rule it allowed, the note sent with a deny, or that a chat message settled it. */
fun settledText(a: Item.Approval): String {
    val d = a.done
    val o = a.outcome ?: return ""
    // seimei's AskUserQuestion: what was answered, or that it was left
    if (a.action == "question") return when (o) {
        "approved" -> "Answered: " + (d?.chosen?.ifEmpty { null } ?: "“${d?.note.orEmpty()}”")
        "expired" -> "No answer in time"
        "denied" -> if (d?.by == "chat") "You answered in the chat" else "Skipped"
        else -> OUTCOME[o] ?: o
    }
    return when {
        o == "expired" && a.expires == 0.0 -> "No answer, denied"
        o == "approved" && d?.chosen == "Allow" -> "Allowed once"
        o == "approved" && d != null && d.chosen.isNotEmpty() -> d.chosen.replace(Regex("^Always allow"), "Always allowed").replace(Regex("^Allow "), "Allowed ")
        o == "approved" && a.action == "plan" -> "Went ahead"
        o == "denied" && d?.by == "chat" -> "Denied: you wrote in the chat"
        o == "denied" && !d?.note.isNullOrEmpty() -> "Denied: “${d!!.note}”"
        else -> OUTCOME[o] ?: o
    }
}

/** A rule option: "Always allow `rule` in this project", marked when the rule lets it run anything. */
@Composable
fun optionText(o: Opt): androidx.compose.ui.text.AnnotatedString {
    val ex = LocalExtra.current
    val code = androidx.compose.ui.text.SpanStyle(fontFamily = FontFamily.Monospace, background = MaterialTheme.colorScheme.surfaceContainerHighest)
    val small = androidx.compose.ui.text.SpanStyle(color = ex.text3, fontSize = 12.sp)
    return androidx.compose.ui.text.buildAnnotatedString {
        if (o.kind == "rule" && o.rule.isNotEmpty()) {
            append(if (o.label.startsWith("Always")) "Always allow " else "Allow ")
            pushStyle(code); append(o.rule); pop()
            if (o.where.isNotEmpty()) { append(" "); pushStyle(small); append(o.where); pop() }
            if (o.broad) { append(" "); pushStyle(small.copy(color = MaterialTheme.colorScheme.error)); append("· runs any code"); pop() }
        } else if (o.where.isNotEmpty() && o.label.endsWith(o.where)) {
            append(o.label.dropLast(o.where.length).trim()); append(" ")
            pushStyle(small); append(o.where); pop()
        } else {
            append(o.label)
            // A question's option may explain itself
            if (o.kind == "choice" && o.where.isNotEmpty()) { append(" "); pushStyle(small); append(o.where); pop() }
        }
    }
}

/** An edit's detail: the path, a blank line, then a diff (- and + lines) or the new file. */
@Composable
fun diffText(detail: String): androidx.compose.ui.text.AnnotatedString {
    val ex = LocalExtra.current
    val err = MaterialTheme.colorScheme.error
    val lines = detail.split('\n')
    val rest = lines.drop(1).let { if (it.firstOrNull() == "") it.drop(1) else it }
    return androidx.compose.ui.text.buildAnnotatedString {
        pushStyle(androidx.compose.ui.text.SpanStyle(color = ex.text3)); append(home(lines[0])); pop()
        for (l in rest) {
            append("\n")
            when {
                l.startsWith("-") -> { pushStyle(androidx.compose.ui.text.SpanStyle(color = err, background = BAD_BG)); append(l); pop() }
                l.startsWith("+") -> { pushStyle(androidx.compose.ui.text.SpanStyle(color = ex.ok, background = OK_BG)); append(l); pop() }
                else -> append(l)
            }
        }
    }
}

val PEN = icon("M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4")
val PLAN = icon("M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01")

@Composable
fun ApprovalCard(a: Item.Approval, vm: Seimei) {
    val cs = MaterialTheme.colorScheme
    var sent by remember(a.aid) { mutableStateOf(false) }
    var noting by remember(a.aid) { mutableStateOf(false) }
    var note by remember(a.aid) { mutableStateOf("") }
    val message = a.action == "message"
    // seimei's AskUserQuestion: the options are the answers, the note writes one, Skip leaves it
    val question = a.action == "question"
    // A permission card carries Claude Code's choices; a guard card is Approve/Deny.
    val perm = a.options.isNotEmpty()
    // The subtitle names what it touches, not the title again: the session a command names, the file an edit writes.
    val sub = when {
        perm -> a.sub.ifEmpty { if (a.action == "edit") home(a.detail.substringBefore('\n')).substringAfterLast('/') else "" }
        a.action == "edit" -> home(a.detail.substringBefore('\n'))
        a.action == "command" -> a.detail.split(Regex("[\\s'\"=]+")).toSet().let { words ->
            vm.sessions.firstOrNull { it.str("name").isNotEmpty() && it.str("name") in words }
                ?.let { s -> "session ${s.str("name")}" + s.str("project").let { if (it.isEmpty()) "" else " · $it" } }
        }
        else -> mapOf("message" to "a session you started", "desktop" to "a desktop action", "discord" to "a Discord action")[a.action]
    }
    val (bg, fg) = when (a.outcome) {
        null -> cs.primaryContainer to cs.onPrimaryContainer
        "approved" -> OK_BG to LocalExtra.current.ok
        else -> BAD_BG to cs.error
    }
    // Which button was tapped, and whether the answer took over 200 ms: that button then shows it is on its way.
    var tapped by remember(a.aid) { mutableStateOf("") }
    val slow by produceState(false, sent) {
        value = false
        if (sent) {
            delay(200)
            value = true
        }
    }
    val haptic = LocalHapticFeedback.current
    fun answer(approve: Boolean, option: String?, note: String? = null, key: String = option ?: "") {
        if (sent) return
        sent = true
        tapped = key
        // Confirm once the server took the answer, Reject when it didn't go through (BEHAVIOR.md 7); nothing on the tap
        vm.decide(a.aid, approve, option, note, landed = { ok ->
            haptic.performHapticFeedback(if (ok) HapticFeedbackType.Confirm else HapticFeedbackType.Reject)
        }) { sent = false }
    }
    // The indicator sits over the hidden label, so the button keeps its size (final/ 18: the shape loader in the tapped button's colour).
    @Composable
    fun Label(key: String, text: String) {
        val busy = slow && tapped == key
        Box(contentAlignment = Alignment.Center) {
            Text(text, Modifier.alpha(if (busy) 0f else 1f))
            if (busy) Loader(24.dp, contained = false, color = LocalContentColor.current)
        }
    }
    // While an answer is on its way the tapped control keeps its colours and the rest are held at .38 (final/ 18)
    fun held(key: String) = Modifier.alpha(if (sent && tapped != key) .38f else 1f)
    val who = if (a.subagent) "${a.who}'s subagent" else a.who
    Card(highlight = a.outcome == null, corner = 28.dp, pad = 16.dp) {
        CardHead(if (a.action == "plan") PLAN else if (question) Icons.question else Icons.lock, bg, fg,
            if (question) "$who asks you" else "$who wants to ${if (message) "message ${a.target}" else a.target}", sub, big = true)
        val box = Modifier.fillMaxWidth().heightIn(max = if (a.outcome == null) 220.dp else 80.dp)
        if (perm && a.action == "plan") {
            Box(box.verticalScroll(rememberScrollState())) { Md(a.detail, 14, if (a.outcome == null) cs.onSurface else LocalExtra.current.text2) }
        } else {
            val mono = !(message || question || a.action == "discord")
            val text = if (perm && a.action == "edit") diffText(a.detail) else androidx.compose.ui.text.AnnotatedString(a.detail)
            Text(
                text, fontSize = 13.sp, lineHeight = 19.sp,
                fontFamily = if (mono) FontFamily.Monospace else null,
                color = if (a.outcome == null) cs.onSurface else LocalExtra.current.text2,
                maxLines = if (a.outcome == null) Int.MAX_VALUE else 3, overflow = TextOverflow.Ellipsis,
                modifier = box
                    .then(if (message) Modifier else Modifier.clip(RoundedCornerShape(10.dp)).background(cs.surface).padding(horizontal = 10.dp, vertical = 8.dp))
                    .verticalScroll(rememberScrollState()),
            )
        }
        val once = a.options.firstOrNull { it.id == "once" }
        val deny = a.options.firstOrNull { it.id == "deny" }
        val extra = a.options.filter { it.id != "once" && it.id != "deny" }
        if (a.outcome == null && extra.isNotEmpty() && !noting) {
            // One segmented list: 2dp gaps, 20dp outer and 4dp inner corners; the rule that runs anything wears the warning container
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                for ((n, o) in extra.withIndex()) {
                    val top = if (n == 0) 20.dp else 4.dp
                    val bottom = if (n == extra.lastIndex) 20.dp else 4.dp
                    val t = LocalTokens.current
                    Text(optionText(o), fontSize = 14.sp, lineHeight = 22.sp, color = if (o.broad) t.onWarnContainer else cs.onSurface,
                        modifier = held(o.id).fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(top, top, bottom, bottom))
                            .background(if (o.broad) t.warnContainer else cs.surfaceContainerHigh)
                            .clickable { answer(true, o.id) }.padding(horizontal = 16.dp, vertical = 13.dp))
                }
            }
        }
        if (a.outcome == null && noting) {
            // The note goes to the agent with a deny, like the terminal's "tell Claude what to do differently".
            val focus = remember { FocusRequester() }
            LaunchedEffect(Unit) { focus.requestFocus() }
            TextField(note, { note = it }, Modifier.fillMaxWidth().focusRequester(focus), placeholder = { Text(if (question) "Answer ${a.who}" else "Note for ${a.who}") }, minLines = 2, maxLines = 5,
                shape = RoundedCornerShape(12.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ))
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (a.outcome != null) {
                Text(settledText(a), fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium,
                    color = if (a.outcome == "approved") LocalExtra.current.ok else cs.error, modifier = Modifier.weight(1f).padding(vertical = 10.dp))
            } else if (noting) {
                Spacer(Modifier.weight(1f))
                TextButton({ noting = false }, enabled = !sent) { Text("Cancel") }
                Spacer(Modifier.width(4.dp))
                Button({ if (question) answer(true, "other", note.trim(), "note") else answer(false, "deny", note.trim(), "note") }, enabled = !sent) {
                    Label("note", if (question) "Answer" else if (a.action == "plan") "Keep planning" else "Deny with note")
                }
            } else {
                // final/ hides the auto-deny line while the card waits: the buttons say it waits
                // A full-width group: the main action largest and on the thumb side
                if (perm) FilledTonalIconButton({ if (!sent) noting = true }, Modifier.size(52.dp).then(held("pen"))) {
                    Icon(PEN, if (question) "Write an answer" else "Add a note", Modifier.size(20.dp))
                }
                if (perm) Spacer(Modifier.width(8.dp))
                // Under the finger a button takes 12 corners and widens while the other gives way (final.css, the fast spring)
                val denyPress = remember { MutableInteractionSource() }
                val allowPress = remember { MutableInteractionSource() }
                val denyDown by denyPress.collectIsPressedAsState()
                val allowDown by allowPress.collectIsPressedAsState()
                val fast = spring<Float>(dampingRatio = .6f, stiffness = 1400f)
                val denyW by animateFloatAsState(if (allowDown) .8f else if (denyDown) 1.3f else 1f, fast, label = "denyW")
                val allowW by animateFloatAsState(if (allowDown) 1.9f else if (denyDown) 1.1f else 1.4f, fast, label = "allowW")
                val denyR by animateDpAsState(if (denyDown) 12.dp else 26.dp, label = "denyR")
                val allowR by animateDpAsState(if (allowDown) 12.dp else 26.dp, label = "allowR")
                FilledTonalButton({ answer(false, if (perm) "deny" else null, key = "deny") }, Modifier.weight(denyW).heightIn(min = 52.dp).then(held("deny")),
                    shape = RoundedCornerShape(denyR), interactionSource = denyPress) {
                    Label("deny", deny?.label ?: "Deny")
                }
                if (!question) {
                    Spacer(Modifier.width(8.dp))
                    Button({ answer(true, if (perm) "once" else null, key = "once") }, Modifier.weight(allowW).heightIn(min = 52.dp).then(held("once")),
                        shape = RoundedCornerShape(allowR), interactionSource = allowPress) {
                        Icon(Icons.check, null, Modifier.size(20.dp).alpha(if (slow && tapped == "once") 0f else 1f))
                        Spacer(Modifier.width(8.dp))
                        Label("once", once?.label ?: "Allow")
                    }
                }
            }
        }
    }
}

// ---------- pictures ----------

@Composable
fun Picture(vm: Seimei, name: String, modifier: Modifier, crop: Boolean = false, ratio: Float? = null) {
    val bmp by produceState<ImageBitmap?>(null, name) { value = vm.image(name) }
    val b = bmp
    if (b == null) Box(modifier.then(if (crop) Modifier else Modifier.aspectRatio(ratio ?: (4f / 3))).background(MaterialTheme.colorScheme.surfaceContainerHigh))
    else Image(b, "Picture, tap to open", modifier.then(if (crop) Modifier else Modifier.aspectRatio(b.width.toFloat() / b.height)),
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
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    // A reply goes straight to typing.
    LaunchedEffect(vm.focusComposer) {
        if (vm.focusComposer) {
            vm.focusComposer = false
            focus.requestFocus()
            keyboard?.show()
        }
    }
    Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 2.dp)) {
        vm.replyTo?.let { (_, text) ->
            Row(
                Modifier.fillMaxWidth().padding(bottom = 8.dp).clip(RoundedCornerShape(14.dp)).background(cs.surfaceContainerHigh)
                    .padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.reply, null, Modifier.size(16.dp), tint = LocalExtra.current.text2)
                Spacer(Modifier.width(8.dp))
                Text("Replying to: ${text.replace(Regex("\\s+"), " ").trim()}", Modifier.weight(1f), fontSize = 13.sp, color = LocalExtra.current.text2,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                IconButton(vm::cancelReply, Modifier.size(40.dp)) { Icon(Icons.close, "Cancel the reply", Modifier.size(16.dp)) }
            }
        }
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
                // A 44dp touch area in the corner around the small visible circle.
                Box(Modifier.align(Alignment.TopEnd).size(44.dp).clickable { vm.detach(a) }, contentAlignment = Alignment.TopEnd) {
                    Box(Modifier.padding(2.dp).size(22.dp).clip(CircleShape).background(Color.Black.copy(alpha = .6f)), contentAlignment = Alignment.Center) {
                        Icon(Icons.close, "Remove", Modifier.size(12.dp), tint = Color.White)
                    }
                }
            }
        }
        // final/ 02: a 52 pill on surface-container-high (attach 48 inside it), then the 52 round send
        Row(verticalAlignment = Alignment.Bottom) {
            val x = LocalTokens.current
            Row(Modifier.weight(1f).heightIn(min = 52.dp).clip(RoundedCornerShape(26.dp)).background(cs.surfaceContainerHigh),
                verticalAlignment = Alignment.Bottom) {
                Box(Modifier.padding(start = 4.dp, bottom = 2.dp)) {
                    IconButton({ menu = true }, Modifier.size(48.dp)) { Icon(Icons.attach, "Attach", Modifier.size(22.dp), tint = x.text3) }
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
                BasicTextField(
                    value = vm.draft,
                    onValueChange = vm::edit,
                    maxLines = 7,
                    textStyle = TextStyle(fontSize = 15.sp, lineHeight = 22.sp, color = x.text),
                    cursorBrush = SolidColor(cs.primary),
                    modifier = Modifier.weight(1f).padding(start = 8.dp, end = 16.dp, top = 15.dp, bottom = 15.dp).focusRequester(focus)
                        .semantics { contentDescription = "Message seimei" },
                    decorationBox = { field ->
                        Box {
                            if (vm.draft.isEmpty()) Text("Message seimei", fontSize = 15.sp, lineHeight = 22.sp, color = x.text3)
                            field()
                        }
                    },
                )
            }
            Spacer(Modifier.width(8.dp))
            val typing = vm.draft.isNotBlank() || vm.attachments.isNotEmpty()
            if (vm.busy && !typing) {
                // The square Stop on the tonal FAB, 16 corners
                Box(Modifier.size(52.dp).clip(RoundedCornerShape(16.dp)).background(cs.primaryContainer)
                    .clickable(onClickLabel = "Stop", onClick = vm::interrupt).semantics { contentDescription = "Stop" },
                    contentAlignment = Alignment.Center) {
                    Box(Modifier.size(16.dp).clip(RoundedCornerShape(4.dp)).background(cs.onPrimaryContainer))
                }
            } else {
                // Offline it stays tappable: send() then says why nothing went out.
                FilledIconButton(vm::send, Modifier.size(52.dp), enabled = typing,
                    colors = IconButtonDefaults.filledIconButtonColors()) {
                    Icon(Icons.send, "Send", Modifier.size(20.dp))
                }
            }
        }
    }
}

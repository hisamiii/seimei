package com.hisamiowo.seimei

import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.flow.first
import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.wear.compose.material3.CircularProgressIndicator
import androidx.wear.compose.material3.ScrollIndicator
import kotlinx.coroutines.launch

// The geometry of the mockups, in dp of the 480 px watch: the log's column and the composer's pill above the bottom edge.
val LOG_W = 158.dp
val PILL_W = 162.dp
val PILL_BOTTOM = 34.dp
val LOG_TOP = 31.dp

/** The chat (page 0) and the trajectory (page 1), side by side. */
@Composable
fun Home(vm: Model) {
    val pager = rememberPagerState(vm.homePage) { 2 }
    val scope = rememberCoroutineScope()
    var goto by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(pager) { snapshotFlow { pager.currentPage }.collect { vm.pageShown(it) } }
    LaunchedEffect(vm.homePage) { if (pager.currentPage != vm.homePage) pager.animateScrollToPage(vm.homePage) }
    Box(Modifier.fillMaxSize()) {
        HorizontalPager(pager, Modifier.fillMaxSize(), beyondViewportPageCount = 1) { p ->
            if (p == 0) ChatPage(vm, pager.currentPage == 0) { key ->
                goto = key
                scope.launch { pager.animateScrollToPage(1) }
            } else TrajectoryPage(vm, pager.currentPage == 1, goto) { goto = null }
        }
        Dots(pager.currentPage, 2, Modifier.align(Alignment.BottomCenter).padding(bottom = 26.dp))
    }
}

@Composable
fun Dots(on: Int, n: Int, modifier: Modifier) {
    val t = LocalT.current
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        for (i in 0 until n) Box(Modifier.size(3.dp).clip(CircleShape).background(if (i == on) t.text else t.outline))
    }
}

/** A header that scrolls away with the content and comes back as soon as the list moves back toward its start. */
class Lift(shown: Boolean, private val h: Float) : NestedScrollConnection {
    var offset by mutableFloatStateOf(if (shown) 0f else -h)
    val shown get() = offset > -h / 2

    override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
        offset = (offset + available.y).coerceIn(-h, 0f)
        return Offset.Zero
    }

    fun fraction() = 1f + offset / h
}

@Composable
fun rememberLift(shown: Boolean): Lift {
    val h = with(LocalDensity.current) { 40.dp.toPx() }
    return remember { Lift(shown, h) }
}

/** The header row: the menu (with its dot when something waits), and the title. */
@Composable
fun BoxScope.Header(vm: Model, lift: Lift, title: @Composable () -> Unit) {
    val t = LocalT.current
    val f = lift.fraction()
    if (f <= 0f) return
    Box(
        Modifier.align(Alignment.TopCenter).fillMaxWidth().height(LOG_TOP + 34.dp)
            .background(Brush.verticalGradient(0f to t.surface, .8f to t.surface, 1f to t.surface.copy(alpha = 0f))).alpha(f),
    )
    Row(
        Modifier.align(Alignment.TopCenter).padding(top = LOG_TOP - 2.dp).offset(y = (-12).dp * (1 - f)).alpha(f).height(28.dp)
            .clip(RoundedCornerShape(14.dp)).clickable(enabled = f > .5f) { vm.push(Screen.Drawer) }.padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.semantics { contentDescription = "Menu" }) {
            Ico(Icons.menu, 16.dp, t.text)
            if (news(vm)) Box(Modifier.align(Alignment.TopEnd).offset(x = 2.dp, y = (-1).dp).size(5.dp).clip(CircleShape).background(t.primary))
        }
        title()
    }
}

/** The menu's dot: a thread or an agent has news. */
fun news(vm: Model) = vm.threads.any { it.optInt("unread") > 0 } || vm.agents().any { it.state == "ask" }

/** A log that ends at the bottom (the newest last), cut and faded under the clock; [room] keeps its oldest row clear of the header. */
@Composable
fun Log(
    state: LazyListState, lift: Lift, modifier: Modifier = Modifier, room: Dp = 12.dp, bottom: Dp = 8.dp, active: Boolean = true,
    content: androidx.compose.foundation.lazy.LazyListScope.() -> Unit,
) {
    LazyColumn(
        modifier.padding(top = LOG_TOP).width(LOG_W).fillMaxHeight().nestedScroll(lift).fadeEdges(17.dp).crown(state, reverse = true, active = active),
        state = state, reverseLayout = true, verticalArrangement = Arrangement.spacedBy(6.dp),
        contentPadding = PaddingValues(top = room, bottom = bottom), content = content,
    )
}

@Composable
fun ChatPage(vm: Model, active: Boolean, onFold: (String) -> Unit) {
    val t = LocalT.current
    val list = rememberLazyListState()
    val lift = rememberLift(false)
    val rows = vm.items.asReversed()
    val liveHere = vm.live.isNotEmpty() && vm.liveLane == "c"
    val lead = (if (liveHere) 1 else 0) + vm.pending.size
    val newest = rows.firstOrNull()
    val follow = rememberFollow(list, if (liveHere) "live" else vm.pending.lastOrNull()?.cid ?: newest?.key, vm.sent,
        force = newest is Item.Approval && newest.outcome == null)
    val open = openCard(rows)
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().padding(bottom = PILL_BOTTOM), horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.weight(1f)) {
                Log(list, lift, active = active) {
                    if (liveHere) item("live") { Md(vm.live) }
                    items(vm.pending.reversed(), key = { "p" + it.cid }) { p ->
                        Bubble(p.text, dim = !p.failed, failed = p.failed, onClick = if (p.failed) ({ vm.retrySend(p) }) else null)
                    }
                    items(rows, key = { it.key }, contentType = { it::class }) { ItemRow(it, vm, "c", onFold) }
                }
                JumpDown(list, follow, if (open >= 0) lead + open else -1)
            }
            Gap(6.dp)
            val call = CallService.call
            if (call != null) CallBar(call)
            else Composer(vm.draft, vm::edit, "Message seimei", "chat", vm, onSend = vm::send, autoDictate = vm.dictateNow, talk = true)
        }
        Header(vm, lift) { T("seimei", size = TITLE, weight = FontWeight.Medium, lines = 1) }
        ScrollIndicator(list, Modifier.align(Alignment.CenterEnd))
        if (!vm.online && vm.items.isEmpty()) T("Connecting…", Modifier.align(Alignment.Center), SMALL, t.text3)
    }
}

@Composable
fun TrajectoryPage(vm: Model, active: Boolean, goto: String?, done: () -> Unit) {
    val list = rememberLazyListState()
    val lift = rememberLift(true)
    val rows = vm.trail.asReversed()
    LaunchedEffect(goto, rows.size) {
        if (goto == null) return@LaunchedEffect
        val i = rows.indexOfFirst { it.key == goto || it.key == "g$goto" }
        if (i >= 0) list.scrollToItem(i)
        done()
    }
    val liveHere = vm.live.isNotEmpty() && vm.liveLane == "t"
    val follow = rememberFollow(list, if (liveHere) "live" else rows.firstOrNull()?.key, 0)
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Box(Modifier.padding(bottom = PILL_BOTTOM + 4.dp)) {
            Log(list, lift, room = 30.dp, active = active) {
                if (liveHere) item("live") { Md(vm.live) }
                items(rows, key = { it.key }, contentType = { it::class }) { ItemRow(it, vm, "t") }
            }
            JumpDown(list, follow)
        }
        Header(vm, lift) {
            Ico(Icons.list, 16.dp, LocalT.current.text)
            T("Trajectory", size = TITLE, weight = FontWeight.Medium, lines = 1)
        }
        ScrollIndicator(list, Modifier.align(Alignment.CenterEnd))
    }
}

// ---------- following the newest row ----------

/** Whether a newest-at-the-bottom list keeps to its end as rows come in, as the phone's chat does. */
class Follow {
    var on by mutableStateOf(true)
}

/**
 * Keeps [state] at its end while the reader is there: a new newest row ([newest]) brings the list down.
 * Text streaming into the newest row grows in place at the bottom. Sending ([sent] going up) always jumps to the end; [force] (an open card) always shows.
 * Rows added below keep the first visible row in place, so whether the reader follows is judged only while the list moves.
 */
@Composable
fun rememberFollow(state: LazyListState, newest: Any?, sent: Int, force: Boolean = false): Follow {
    val f = remember { Follow() }
    val near = with(LocalDensity.current) { 48.dp.toPx() }
    LaunchedEffect(state) {
        snapshotFlow { Triple(state.isScrollInProgress, state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            .collect { (moving, i, off) -> if (moving) f.on = i == 0 && off < near }
    }
    LaunchedEffect(newest) {
        if (f.on || force) {
            state.scrollToItem(0)
            f.on = true
        }
    }
    var seen by remember { mutableStateOf(sent) }
    LaunchedEffect(sent) {
        if (sent > seen) {
            f.on = true
            state.scrollToItem(0)
        }
        seen = sent
    }
    return f
}

/** Scrolled up: a small round way back down. An open card above the view comes first, as on the phone. */
@Composable
fun BoxScope.JumpDown(state: LazyListState, f: Follow, open: Int = -1) {
    if (f.on) return
    val t = LocalT.current
    val scope = rememberCoroutineScope()
    // A 26dp circle in a 40dp touch area.
    Box(
        Modifier.align(Alignment.BottomCenter).size(40.dp).clip(CircleShape)
            .clickable {
                scope.launch {
                    val last = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
                    if (open > last) state.animateScrollToItem(open)
                    else {
                        state.animateScrollToItem(0)
                        f.on = true
                    }
                }
            }
            .semantics { contentDescription = "Jump to latest" },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(26.dp).clip(CircleShape).background(t.containerHighest), contentAlignment = Alignment.Center) { Ico(Icons.down, 14.dp, t.text) }
    }
}

/** Where an unanswered card sits among [rows] (newest first), or -1. */
fun openCard(rows: List<Item>) = rows.indexOfFirst { (it is Item.Approval && it.outcome == null) || (it is Item.Worker && it.event == "asking" && it.settled == null) }

// ---------- the composer ----------

/**
 * The composer: one slim pill with the field, the mic and send inside, as v4 draws it.
 * Typing goes through the system keyboard; the mic dictates into the draft (docs/asr.md), and nothing is sent on its own.
 * [stop] shows a stop button while the agent works and the draft is empty; else [talk] puts the voice call's button in send's place, as on the phone.
 */
@Composable
fun Composer(
    text: String, onChange: (String) -> Unit, hint: String, target: String, vm: Model, onSend: () -> Unit,
    stop: (() -> Unit)? = null, autoDictate: Boolean = false, talk: Boolean = false,
) {
    val t = LocalT.current
    val d = vm.dictation
    val ctx = LocalContext.current
    val focus = remember { FocusRequester() }
    val focusManager = LocalFocusManager.current
    fun hintNow() = vm.asrHint(target, text)
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) d.start(target, hintNow()) else vm.snack = "Allow the microphone to dictate"
    }
    val askCall = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) vm.startCall() else vm.snack = "Allow the microphone to talk"
    }
    val call = if (!talk || !vm.voiceOn) null else ({
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) vm.startCall()
        else askCall.launch(Manifest.permission.RECORD_AUDIO)
    })
    val mic = {
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) d.start(target, hintNow())
        else ask.launch(Manifest.permission.RECORD_AUDIO)
    }
    // Dictated phrases land at the draft's end, in the order they were said.
    LaunchedEffect(d.results.size, d.target) {
        if (d.target != target || d.results.isEmpty()) return@LaunchedEffect
        val add = d.results.joinToString(" ")
        d.results.clear()
        onChange(if (text.isEmpty() || text.last().isWhitespace()) text + add else "$text $add")
    }
    LaunchedEffect(autoDictate) {
        if (!autoDictate) return@LaunchedEffect
        // At a cold start the server's answer on dictation may still be on the way; with dictation off, the keyboard opens instead.
        // The flag clears only after the wait: clearing it restarts this effect.
        val on = vm.asrOn || withTimeoutOrNull(1500) { snapshotFlow { vm.asrOn }.first { it } } == true
        vm.dictateNow = false
        if (!on) runCatching { focus.requestFocus() }
        else if (d.state == Dictation.State.IDLE) mic()
    }
    val recording = d.state == Dictation.State.RECORDING && d.target == target
    val grown = text.isNotEmpty() || recording
    val shape = RoundedCornerShape(15.dp)
    Column(
        Modifier.width(PILL_W).heightIn(min = 30.dp).clip(shape).background(t.containerHigh)
            .let { if (recording) it.border(1.5.dp, t.primary, shape) else it }
            .padding(start = if (grown) 10.dp else 12.dp, end = 4.dp, top = if (grown) 5.dp else 0.dp, bottom = if (grown) 4.dp else 0.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            BasicTextField(
                text, onChange, Modifier.weight(1f).focusRequester(focus).let { if (!grown) it.height(30.dp).padding(top = 7.dp) else it },
                textStyle = TextStyle(color = t.text, fontSize = if (grown) BODY else SMALL1, lineHeight = BODY_LH, fontFamily = Sans),
                cursorBrush = SolidColor(t.primary), maxLines = if (grown) 4 else 1,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { focusManager.clearFocus() }),
                decorationBox = { field ->
                    Box {
                        if (text.isEmpty() && !recording) T(hint, size = SMALL1, color = t.text3, lines = 1)
                        if (recording && d.results.isNotEmpty()) T(d.results.joinToString(" "), size = BODY, color = t.primary)
                        field()
                    }
                },
            )
            if (!grown) Buttons(vm, text, target, mic, onSend, stop, call)
        }
        if (recording) RecordingRow(d)
        else if (grown) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) { Buttons(vm, text, target, mic, onSend, stop, call) }
    }
}

@Composable
private fun Buttons(vm: Model, text: String, target: String, mic: () -> Unit, onSend: () -> Unit, stop: (() -> Unit)?, call: (() -> Unit)?) {
    val t = LocalT.current
    val d = vm.dictation
    Row(horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
        if (vm.asrOn) Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
            if (d.state == Dictation.State.TRANSCRIBING && d.target == target) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
            else Box(Modifier.size(24.dp).clip(CircleShape).clickable(enabled = d.state == Dictation.State.IDLE, onClick = mic)
                .semantics { contentDescription = "Dictate" }, contentAlignment = Alignment.Center) { Ico(Icons.mic, 18.dp, t.text3) }
        }
        when {
            text.isNotBlank() -> Box(Modifier.size(24.dp).clip(CircleShape).background(t.primary).clickable(onClick = onSend)
                .semantics { contentDescription = "Send" }, contentAlignment = Alignment.Center) { Ico(Icons.send, 15.dp, t.onPrimary) }
            stop != null -> Box(Modifier.size(24.dp).clip(CircleShape).background(t.primaryContainer).clickable(onClick = stop)
                .semantics { contentDescription = "Stop" }, contentAlignment = Alignment.Center) {
                Box(Modifier.size(10.dp).clip(RoundedCornerShape(3.dp)).background(t.onPrimaryContainer))
            }
            call != null -> Box(Modifier.size(24.dp).clip(CircleShape).background(t.primary).clickable(onClick = call)
                .semantics { contentDescription = "Talk" }, contentAlignment = Alignment.Center) { Ico(Icons.wave, 14.dp, t.onPrimary) }
            else -> Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) { Ico(Icons.send, 17.dp, mix(t.text3, t.containerHigh, .45f)) }
        }
    }
}

/** While recording: drop, the level, the time and stop, in one row under the words so far. */
@Composable
private fun RecordingRow(d: Dictation) {
    val t = LocalT.current
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 2.dp)) {
        Box(Modifier.size(24.dp).clip(CircleShape).clickable { d.drop() }.semantics { contentDescription = "Drop" }, contentAlignment = Alignment.Center) {
            Ico(Icons.close, 14.dp, t.text3)
        }
        Row(Modifier.weight(1f).height(16.dp), horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
            for (l in d.levels.takeLast(14)) Box(Modifier.width(2.dp).fillMaxHeight(.15f + .85f * l).clip(RoundedCornerShape(1.dp)).background(t.primary))
        }
        T("%d:%02d".format(d.seconds / 60, d.seconds % 60), size = SMALL0, color = t.text2, lines = 1)
        Box(Modifier.size(24.dp).clip(RoundedCornerShape(9.dp)).background(t.primary).clickable { d.stop() }
            .semantics { contentDescription = "Stop dictating" }, contentAlignment = Alignment.Center) {
            Box(Modifier.size(10.dp).clip(RoundedCornerShape(3.dp)).background(t.onPrimary))
        }
    }
}

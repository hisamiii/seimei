package com.hisamiowo.seimei

import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.animation.core.Animatable
import androidx.activity.BackEventCompat
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.runtime.snapshotFlow
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.withStyle
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.graphics.Shape
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.Button
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.graphics.SolidColor

// ---------- tones and words, as on the web page ----------

// The theme's tokens (Theme.kt) under the names the pages use.
val PEACH: Color @Composable get() = LocalTokens.current.warn
val MAUVE: Color @Composable get() = LocalTokens.current.primary
val ASK_BG: Color @Composable get() = LocalTokens.current.tint

/** "at 10:28 PM" today, "on 18 Oct at 10:28 PM" another day. */
fun atTime(t: Double): String =
    if (dayOf(t) == LocalDate.now()) "at ${clock(t)}" else "on ${dayOf(t).format(DateTimeFormatter.ofPattern("d MMM"))} at ${clock(t)}"

fun stateText(w: JSONObject): String {
    val at = atTime(w.optDouble("updated", 0.0))
    val since = at.removePrefix("at ").removePrefix("on ")
    return when (w.str("state")) {
        "starting" -> "Starting"
        "running" -> "Working since $since"
        "asking" -> "Needs you"
        "done" -> "Finished $at"
        "failed" -> "Failed $at"
        "stopped" -> "Stopped $at"
        "interrupted" -> "Interrupted $at"
        "laptop" -> "In a terminal on the server since $since"
        "released" -> "Your session since $since"
        else -> w.str("state")
    }
}

fun String.cap() = replaceFirstChar { it.uppercase() }

/** What a running shikigami is up to: its latest step. */
fun workerLine(w: JSONObject): String {
    val d = w.optJSONObject("doing") ?: return if (w.str("state") == "starting") "Starting" else ""
    if (d.str("text").isNotEmpty()) return d.str("text")
    // A command is the news itself
    if (d.str("name") == "Bash" && d.str("summary").isNotEmpty()) return d.str("summary")
    val what = doingPhrase(d.str("name")).cap()
    return if (d.str("summary").isNotEmpty()) "$what: ${d.str("summary")}" else what
}

/** What a session is up to: its open call and how long it has run. */
fun sessionLine(d: JSONObject?): String {
    d ?: return ""
    val what = if (d.str("tool") == "Bash") d.str("what") else "${doingPhrase(d.str("tool"))}: ${d.str("what")}"
    val since = d.optDouble("since", 0.0)
    return (if (d.str("tool") == "Bash") what else what.cap()) + if (since > 0) " · ${elapsed(since).substringBefore(' ')}" else ""
}

/** Seconds ago as now, 5m, 3h, 2d. */
fun short(t: Double): String {
    val s = maxOf(0.0, System.currentTimeMillis() / 1000.0 - t)
    return when {
        s < 60 -> "now"
        s < 3600 -> "${(s / 60).toInt()}m"
        s < 86400 -> "${(s / 3600).toInt()}h"
        else -> "${(s / 86400).toInt()}d"
    }
}

/** A clock time today, a date before that. */
fun whenText(t: Double): String =
    if (dayOf(t) == LocalDate.now()) clock(t) else dayOf(t).format(DateTimeFormatter.ofPattern("d MMM"))

/** The theme's project colors; a project keeps its place in the list everywhere (the same hash as the web page). */
@Composable
fun tint(project: String): Color = tintOf(project, LocalTokens.current)

fun tintOf(project: String, t: Tokens): Color {
    if (project.isEmpty()) return t.text3
    var h = 0L
    for (c in project) h = (h * 31 + c.code) and 0xFFFFFFFFL
    return t.tints[(h % t.tints.size).toInt()]
}

/** One row of the list and the page head, from a shikigami or one of the user's sessions. */
data class Agent(
    val kind: String, val id: String, val name: String, val project: String, val context: Int?, val state: String,
    val since: Double, val line: String, val fresh: Boolean = false, val bad: Boolean = false,
    /** A finished shikigami's end state (stopped, interrupted, ...), for its tile icon. */
    val end: String = "",
    /** Hidden from the list; find: what the Archived search looks in. */
    val archived: Boolean = false,
    val find: List<String> = emptyList(),
)

fun JSONObject.context(): Int? = if (has("context") && !isNull("context")) optInt("context") else null

fun fromWorker(w: JSONObject): Agent {
    val st = w.str("state")
    val state = if (st == "asking") "ask" else if (st in ACTIVE) "work" else "done"
    return Agent(
        "worker", w.str("id"), w.str("name"), w.str("project"), w.context(), state, w.optDouble("updated", 0.0),
        when (state) {
            "ask" -> w.str("question")
            "work" -> workerLine(w)
            else -> w.str("summary").ifEmpty { stateText(w) }
        },
        bad = st == "failed", end = st, archived = w.optBoolean("archived"),
        find = listOf(w.str("name"), w.str("task"), w.str("summary"),
            w.optJSONObject("now")?.let { "${it.str("id")} ${it.str("title")}" } ?: ""),
    )
}

fun fromSession(s: JSONObject, seen: JSONObject): Agent {
    val q = s.optBoolean("question")
    val asking = q || s.str("status") == "waiting"
    val state = if (asking) "ask" else if (s.str("status") == "busy") "work" else "idle"
    return Agent(
        "session", s.optLong("pid").toString(), s.str("name"), s.str("project"), s.context(), state, s.optDouble("since", 0.0),
        when {
            q -> s.str("ask")
            asking -> s.str("waiting_text").ifEmpty { "Waiting on the server" }
            state == "work" -> sessionLine(s.optJSONObject("doing"))
            else -> s.str("line")
        },
        fresh = state == "idle" && s.optDouble("active", 0.0) > seen.optDouble(s.str("session_id"), 0.0) + 1,
        archived = s.optBoolean("archived"), find = listOf(s.str("name"), s.str("project"), s.str("line")),
    )
}

/** Working agents, and how many things wait on the user. */
fun agentCounts(vm: Seimei): Pair<Int, Int> {
    val all = (vm.workers.map(::fromWorker) + vm.sessions.map { fromSession(it, vm.seen) }).filter { !it.archived }
    return all.count { it.state == "work" } to all.count { it.state == "ask" }
}

// ---------- the list ----------

@Composable
fun AgentsTab(vm: Seimei) {
    // Relative times move on by themselves.
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(30_000)
            value = System.currentTimeMillis()
        }
    }
    if (vm.archOpen) return ArchiveList(vm, now)
    var showAll by remember { mutableStateOf(false) }
    val items = (vm.workers.filter { it.str("state") in ACTIVE }.map(::fromWorker) + vm.sessions.map { fromSession(it, vm.seen) })
        .filter { !it.archived }
    fun by(st: String) = items.filter { it.state == st }.sortedByDescending { it.since }
    val ask = by("ask")
    val work = by("work")
    val idle = by("idle")
    // One given to the user (released) is with their sessions now.
    val done = (vm.allWorkers ?: vm.workers).filter { it.str("state") !in ACTIVE && !it.optBoolean("archived") && it.str("state") != "released" }
        .sortedByDescending { it.optDouble("updated") }
    val archived = archivedAgents(vm)
    if (items.isEmpty() && done.isEmpty() && archived.isEmpty()) {
        Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            Text("No shikigami or sessions yet", fontWeight = FontWeight.Medium, fontSize = 16.sp)
        }
        return
    }
    val groups = listOf(Triple("Needs you", PEACH, ask), Triple("Working", MAUVE, work), Triple("Idle", LocalExtraText3, idle))
    val list = rememberLazyListState()
    LaunchedEffect(list) { snapshotFlow { list.canScrollBackward }.collect { vm.lifted = it } }
    LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        for ((title, color, list) in groups) {
            if (list.isEmpty()) continue
            item("h-$title") { GroupTitle(title, color) }
            itemsIndexed(list, key = { _, it -> "${it.kind}-${it.id}" }) { i, a ->
                val shape = segment(i, list.size, if (a.state == "ask") 28.dp else 20.dp, if (a.state == "ask") 8.dp else 4.dp)
                if (a.state == "idle") SwipeToArchive(a, vm) { AgentRow(a, now, shape = shape) { vm.open(a) } } else AgentRow(a, now, shape = shape) { vm.open(a) }
            }
        }
        if (done.isNotEmpty()) {
            val shown = if (showAll) done else done.take(15)
            item("h-done") { GroupTitle("Finished", LocalExtraText3) }
            itemsIndexed(shown.map(::fromWorker), key = { _, it -> "done-${it.id}" }) { i, a ->
                SwipeToArchive(a, vm) { AgentRow(a, now, shape = segment(i, shown.size)) { vm.open(a) } }
            }
            if (done.size > shown.size) item("all") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    TextButton({ showAll = true }) { Text("Show all ${done.size}") }
                }
            }
        }
        if (archived.isNotEmpty()) item("archived") { ArchivedRow(archived.size) { vm.archOpen = true } }
    }
}

// ---------- the archive: hidden from the list, never stopped or deleted ----------

fun archivedAgents(vm: Seimei): List<Agent> =
    ((vm.allWorkers ?: vm.workers).filter { it.optBoolean("archived") && it.str("state") !in ACTIVE && it.str("state") != "released" }.map(::fromWorker) +
        vm.sessions.filter { it.optBoolean("archived") }.map { fromSession(it, vm.seen) }).sortedByDescending { it.since }

/** The text a search match shows in, and where the match sits: the line, else the task or project around it. */
fun searchLine(a: Agent, q: String): Pair<String, Int> {
    if (q.isEmpty()) return a.line to -1
    val text = if (a.line.lowercase().contains(q)) a.line else a.find.drop(1).firstOrNull { it.lowercase().contains(q) } ?: a.line
    val i = text.lowercase().indexOf(q)
    // A match far into the line keeps a few words before it in view; the cut falls between words.
    if (i <= 40) return text to i
    val from = text.lastIndexOf(' ', i - 24) + 1
    return "…" + text.substring(from) to i - from + 1
}

/** Swipe left to archive (in the Archived list: to bring it back). */
@Composable
fun SwipeToArchive(a: Agent, vm: Seimei, content: @Composable () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val state = rememberSwipeToDismissBoxState()
    val scope = rememberCoroutineScope()
    // A tick when the swipe crosses the mark where letting go archives (BEHAVIOR.md 7)
    val haptic = LocalHapticFeedback.current
    LaunchedEffect(state) {
        snapshotFlow { state.targetValue }.collect { if (it == SwipeToDismissBoxValue.EndToStart) haptic.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate) }
    }
    SwipeToDismissBox(
        state,
        backgroundContent = {
            Row(Modifier.fillMaxSize().clip(RoundedCornerShape(18.dp)).background(cs.secondaryContainer).padding(end = 24.dp),
                horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.shelf, null, Modifier.size(20.dp), tint = cs.onSecondaryContainer)
                Spacer(Modifier.width(8.dp))
                Text(if (a.archived) "Unarchive" else "Archive", fontSize = 14.sp, fontWeight = FontWeight.Medium, color = cs.onSecondaryContainer)
            }
        },
        enableDismissFromStartToEnd = false,
        onDismiss = {
            if (it == SwipeToDismissBoxValue.EndToStart) vm.setArchived(a, !a.archived)
            // The list keeps a row's saved state by key: left dismissed, a row brought back by Undo would dismiss again.
            scope.launch { state.snapTo(SwipeToDismissBoxValue.Settled) }
        },
    ) {
        Box(Modifier.clip(RoundedCornerShape(18.dp)).background(cs.surface).semantics {
            customActions = listOf(CustomAccessibilityAction(if (a.archived) "Unarchive" else "Archive") { vm.setArchived(a, !a.archived); true })
        }) { content() }
    }
}

@Composable
fun ArchivedRow(n: Int, sub: String = "$n shikigami and session${if (n == 1) "" else "s"}", onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(top = 10.dp).heightIn(min = 64.dp).clip(RoundedCornerShape(18.dp)).clickable(onClick = onClick).padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(42.dp).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh), contentAlignment = Alignment.Center) {
            Icon(Icons.shelf, null, Modifier.size(20.dp), tint = LocalExtra.current.text3)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text("Archived", fontSize = 15.5.sp, fontWeight = FontWeight.Medium)
            Text(sub, fontSize = 13.5.sp, color = LocalExtra.current.text3)
        }
        Icon(Icons.chev, null, Modifier.size(20.dp), tint = LocalExtra.current.text3)
    }
}

@Composable
fun ArchiveList(vm: Seimei, now: Long) {
    val close = {
        vm.archOpen = false
        vm.archQuery = ""
    }
    BackHandler(onBack = close)
    val cs = MaterialTheme.colorScheme
    val all = archivedAgents(vm)
    val q = vm.archQuery.trim().lowercase()
    val hits = if (q.isEmpty()) all else all.filter { a -> a.find.any { it.lowercase().contains(q) } }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        item("ahead") {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(close, Modifier.size(48.dp)) { Icon(Icons.back, "Back to agents", Modifier.size(22.dp)) }
                Text("Archived", Modifier.weight(1f), fontSize = 18.sp, fontWeight = FontWeight.Medium)
                Text(if (q.isEmpty()) "${all.size}" else "${hits.size} of ${all.size}", fontSize = 13.sp, color = LocalExtra.current.text3,
                    modifier = Modifier.padding(end = 8.dp))
            }
        }
        item("search") {
            TextField(vm.archQuery, { vm.archQuery = it }, Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 8.dp), singleLine = true,
                placeholder = { Text("Search") },
                leadingIcon = { Icon(Icons.search, null, Modifier.size(20.dp)) },
                trailingIcon = { if (vm.archQuery.isNotEmpty()) IconButton({ vm.archQuery = "" }) { Icon(Icons.close, "Clear the search", Modifier.size(18.dp)) } },
                shape = RoundedCornerShape(24.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ))
        }
        if (hits.isEmpty()) item("none") {
            Text(if (q.isEmpty()) "Nothing archived. Idle ones move here after 12 hours." else "No matches",
                Modifier.fillMaxWidth().padding(32.dp), color = LocalExtra.current.text3, fontSize = 14.sp, textAlign = TextAlign.Center)
        }
        items(hits, key = { "arch-${it.kind}-${it.id}" }) { a -> SwipeToArchive(a, vm) { AgentRow(a, now, q) { vm.open(a) } } }
    }
}

val LocalExtraText3: Color @Composable get() = LocalTokens.current.text3

fun Seimei.open(a: Agent) = if (a.kind == "worker") openWorker(a.id) else openSession(a.id.toLong())

@Composable
fun GroupTitle(title: String, color: Color) =
    Text(title, color = color, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 16.dp, top = 20.dp, bottom = 8.dp).semantics { heading() })

/** A row's corners in a segmented group: big outer ones on the group's ends, small ones between the rows. */
fun segment(i: Int, n: Int, outer: Dp = 20.dp, inner: Dp = 4.dp): Shape =
    RoundedCornerShape(if (i == 0) outer else inner, if (i == 0) outer else inner, if (i == n - 1) outer else inner, if (i == n - 1) outer else inner)

/** The user turned animations off (Remove animations, or animator duration scale 0): motion goes, states stay. */
@Composable
fun reducedMotion(): Boolean {
    val ctx = LocalContext.current
    return remember { android.provider.Settings.Global.getFloat(ctx.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

/** The expressive spatial spring as an easing, the same linear() fit as the web's --spring. */
val SpringEasing = Easing { t ->
    val k = floatArrayOf(0f, .103f, .3f, .506f, .684f, .817f, .907f, .962f, .994f, 1.009f, 1.014f, 1.014f, 1.012f, 1.008f, 1.006f, 1.003f, 1f)
    val x = t * (k.size - 1)
    val i = x.toInt().coerceAtMost(k.size - 2)
    k[i] + (k[i + 1] - k[i]) * (x - i)
}

/** A tile in its owner's identity shape. A working one morphs between that shape and the soft burst and turns slowly,
 * as on the web (final.css): the morph is its activity indicator. */
@Composable
fun ShapeTile(name: String, size: Dp, bg: Color, working: Boolean = false, content: @Composable () -> Unit) {
    val me = SHAPES.getValue(shapeOf(name))
    if (!working || reducedMotion()) {
        Box(Modifier.size(size).clip(PolyShape(me)).background(bg), contentAlignment = Alignment.Center) { content() }
        return
    }
    val run = rememberInfiniteTransition("tile")
    // The morph goes there and back on the spring (2.4 s each way), started at the name's phase, as on the web
    val clock by run.animateFloat(0f, 4800f, infiniteRepeatable(tween(4800, easing = LinearEasing)), "morph")
    val phase = remember(name) { phaseOf(name) }
    val at = (clock + phase) % 4800f
    val t = SpringEasing.transform(if (at < 2400f) at / 2400f else (4800f - at) / 2400f)
    val turn by run.animateFloat(0f, 360f, infiniteRepeatable(tween(9000, easing = LinearEasing)), "turn")
    val burst = SHAPES.getValue("soft-burst")
    Box(Modifier.size(size).graphicsLayer { rotationZ = turn }.clip(PolyShape(me, burst, t)).background(bg), contentAlignment = Alignment.Center) { content() }
}

@Composable
fun AgentTile(a: Agent) {
    // Shape and tone say the state: a warm squircle asks, the loading indicator works, a circle waits or is done
    val t = LocalTokens.current
    val cs = MaterialTheme.colorScheme
    val (bg, color) = when {
        a.state == "ask" -> t.warn to t.onWarn
        a.state == "work" -> cs.primaryContainer to cs.primary
        a.bad -> cs.surfaceContainerHigh to cs.error
        else -> cs.surfaceContainerHigh to LocalExtra.current.text2
    }
    // A live one wears its identity shape (a working one morphs it, the morph is its activity); finished ones are circles
    if (a.state != "done") {
        val work = a.state == "work"
        ShapeTile(a.name, 44.dp, if (work) cs.primary else if (a.state == "idle") t.containerHighest else bg, work) {
            when (a.state) {
                "ask" -> Icon(Icons.question, null, Modifier.size(20.dp), tint = color)
                "idle" -> Icon(Icons.clock, null, Modifier.size(20.dp), tint = color)
            }
        }
        return
    }
    Box(Modifier.size(44.dp).clip(CircleShape).background(bg), contentAlignment = Alignment.Center) {
        when (a.state) {
            "ask" -> Icon(Icons.question, null, Modifier.size(20.dp), tint = color)
            "idle" -> Icon(Icons.clock, null, Modifier.size(20.dp), tint = color)
            else -> Icon(when {
                a.bad -> Icons.alert
                a.end == "stopped" -> Icons.stopped
                a.end == "interrupted" -> Icons.restarted
                else -> Icons.check
            }, null, Modifier.size(20.dp), tint = color)
        }
    }
}

@Composable
fun AgentRow(a: Agent, now: Long, query: String = "", shape: Shape = RoundedCornerShape(20.dp), onClick: () -> Unit) {
    val t = LocalTokens.current
    val cs = MaterialTheme.colorScheme
    val ask = a.state == "ask"
    Column {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clip(shape)
            .background(if (ask) t.warnContainer else if (a.state == "done") cs.surfaceContainer.copy(alpha = .55f).compositeOver(cs.surfaceContainerLow) else cs.surfaceContainer)
            .clickable(onClick = onClick)
            .padding(start = 10.dp, end = 16.dp, top = if (ask) 12.dp else 10.dp, bottom = if (ask) 12.dp else 10.dp),
        verticalAlignment = if (ask) Alignment.Top else Alignment.CenterVertically,
    ) {
        AgentTile(a)
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // Name and tag take the room left of the time, so the time lines up at the right edge.
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    // Who asks has an emphasized name (final/ 08)
                    Text(a.name, fontSize = 16.sp, lineHeight = 24.sp, fontWeight = if (ask) FontWeight.Bold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        color = if (ask) t.onWarnContainer else if (a.state == "done" || a.archived) LocalExtra.current.text2 else Color.Unspecified, modifier = Modifier.weight(1f, fill = false))
                }
                Spacer(Modifier.width(8.dp))
                // New since you looked: the time in primary, no dot
                Text(if (a.state == "done") whenText(a.since) else short(a.since).also { now }, fontSize = 12.sp,
                    color = if (a.fresh) cs.primary else if (ask) t.onWarnContainer.copy(alpha = .8f) else LocalExtra.current.text3,
                    fontWeight = if (a.fresh) FontWeight.Medium else null)
            }
            Spacer(Modifier.height(3.dp))
            // One paragraph, the project inline, so a wrapped question starts its second line at the edge (final/ 08)
            val (line, at) = searchLine(a, query)
            val mark = MaterialTheme.colorScheme.primary.copy(alpha = .3f)
            Text(buildAnnotatedString {
                if (a.project.isNotEmpty() && a.project != a.name) withStyle(SpanStyle(fontWeight = FontWeight.Medium,
                    color = if (ask) t.onWarnContainer else LocalExtra.current.text2)) { append("${a.project} · ") }
                val start = length
                append(line)
                if (at >= 0) addStyle(SpanStyle(background = mark), start + at, start + at + query.length)
            }, fontSize = 14.sp, lineHeight = 20.sp, maxLines = if (ask) 2 else 1, overflow = TextOverflow.Ellipsis,
                color = when (a.state) { "ask" -> t.onWarnContainer; "work" -> cs.onSurface; else -> LocalExtra.current.text3 })
        }
    }
    Spacer(Modifier.height(2.dp))
    }
}

// ---------- the page of one agent ----------

@Composable
fun ContextRing(pct: Int?, onClick: () -> Unit) {
    val n = (pct ?: 0).coerceIn(0, 100)
    val track = MaterialTheme.colorScheme.surfaceContainerHighest
    val fill = if (n >= 80) PEACH else LocalExtra.current.text3
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onClick)
            .semantics { contentDescription = if (pct == null) "Details" else "Context $n% used, session details" },
        contentAlignment = Alignment.Center,
    ) {
        // No context figure yet: an info icon, not an empty track that reads as a spinner.
        if (pct == null) {
            Icon(Icons.info, null, Modifier.size(22.dp), tint = LocalExtra.current.text2)
            return@Box
        }
        Canvas(Modifier.size(32.dp)) {
            val w = 3.dp.toPx()
            val r = Size(size.width - w, size.height - w)
            drawArc(track, 0f, 360f, false, Offset(w / 2, w / 2), r, style = Stroke(w))
            drawArc(fill, -90f, 360f * n / 100, false, Offset(w / 2, w / 2), r, style = Stroke(w, cap = StrokeCap.Round))
        }
        Text("$n", fontSize = 10.5.sp, fontWeight = FontWeight.SemiBold, color = LocalExtra.current.text2, style = TNUM)
    }
}

/** Why the composer cannot type into this session, or null. */
fun blocked(pg: Page): String? {
    if (pg.kind == "worker") return null
    val ask = pg.ask
    // Without kitty a message still goes through seimei (SendMessage), an answer does not.
    pg.reach?.let { if (pg.via != "seimei" || ask != null) return if (ask != null) "Answer on the server" else it }
    if (ask != null && !ask.optBoolean("answerable")) return "Answer on the server"
    if (pg.row.str("status") == "waiting" && ask == null) return "${pg.row.str("waiting_text").ifEmpty { "Waiting on the server" }}. Answer it there."
    return null
}

@Composable
fun AgentPage(vm: Seimei, pg: Page) {
    // The details toggle is a state change, not a screen: a plain handler
    BackHandler(enabled = pg.details) { pg.details = false }
    // Predictive back (BEHAVIOR.md 2): while the gesture runs the page shrinks toward 90% and leans to the swipe edge
    // with the 8dp margin and rounded corners; on commit it leaves on emphasized accelerate, on cancel it comes back
    val back = remember { Animatable(0f) }
    var edge by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    PredictiveBackHandler(enabled = !pg.details) { events ->
        try {
            events.collect { e ->
                edge = e.swipeEdge
                back.snapTo(e.progress)
            }
            back.animateTo(1f, tween(150, easing = EMPHASIZED_ACCEL))
            vm.closePage()
        } catch (e: kotlin.coroutines.cancellation.CancellationException) {
            scope.launch { back.animateTo(0f, tween(200, easing = EMPHASIZED_DECEL)) }
            throw e
        }
    }
    val cs = MaterialTheme.colorScheme
    val a = if (pg.kind == "worker") fromWorker(pg.row) else fromSession(pg.row, vm.seen)
    var zoom by remember { mutableStateOf<String?>(null) }
    val p = back.value
    Surface(Modifier.fillMaxSize().graphicsLayer {
        val s = 1f - .1f * p
        scaleX = s
        scaleY = s
        translationX = (if (edge == BackEventCompat.EDGE_LEFT) 1 else -1) * 8.dp.toPx() * p
        shape = RoundedCornerShape(28.dp * p)
        clip = p > 0f
        alpha = if (p >= 1f) 0f else 1f
    }, color = cs.surface) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(vm::closePage, Modifier.size(48.dp)) { Icon(Icons.back, "Back to agents", Modifier.size(22.dp)) }
                Column(Modifier.weight(1f)) {
                    // final/ 10: the name at 22, then the project (text-2, medium) and the state in its colour, at 12
                    Text(a.name, fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Row(Modifier.padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (a.project.isNotEmpty() && a.project != a.name) {
                            Text(a.project, fontSize = 12.sp, lineHeight = 16.sp, color = LocalExtra.current.text2, fontWeight = FontWeight.Medium)
                            Text(" · ", fontSize = 12.sp, lineHeight = 16.sp, color = LocalExtra.current.text3)
                        }
                        val word = when (a.state) {
                            "ask" -> "Needs you"
                            "work" -> "Working"
                            "idle" -> "Idle"
                            else -> stateText(pg.row)
                        }
                        val ago = short(a.since)
                        Text(if (a.state == "done" || ago == "now") word else "$word · $ago", fontSize = 12.sp, lineHeight = 16.sp,
                            color = when (a.state) { "ask" -> PEACH; "work" -> cs.primary; else -> LocalExtra.current.text3 }, maxLines = 1)
                    }
                }
                // One tap each way between seimei and the user.
                val canHand = if (pg.kind == "worker") pg.row.optBoolean("started", true) && pg.row.str("state") !in setOf("released", "starting")
                    else pg.row.str("kind") in setOf("interactive", "background")
                if (canHand) FilledTonalButton({ vm.handoff(pg) }, Modifier.height(40.dp).padding(end = 0.dp)
                    .semantics { contentDescription = if (pg.kind == "worker") "Take over ${a.name}" else "Hand ${a.name} off to seimei" },
                    contentPadding = PaddingValues(horizontal = 16.dp)) {
                    Text(if (pg.kind == "worker") "Take over" else "Hand off", fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                }
                Spacer(Modifier.width(8.dp))
                ContextRing(a.context) { pg.details = true }
            }
            if (a.archived) Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 4.dp).clip(RoundedCornerShape(16.dp))
                    .background(cs.surfaceContainer).padding(start = 14.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.shelf, null, Modifier.size(20.dp), tint = LocalExtra.current.text3)
                Spacer(Modifier.width(12.dp))
                Text(if (pg.kind == "worker") "Archived. A message brings it back." else "Archived, still running",
                    Modifier.weight(1f).padding(vertical = 6.dp), fontSize = 14.sp, lineHeight = 19.sp, color = LocalExtra.current.text2)
                TextButton({ vm.setArchived(a, false) }) { Text("Unarchive") }
            }
            // Like a chat: newest at the bottom, and a reader near the end follows it.
            val list = rememberLazyListState()
            val rows = pg.items.asReversed()
            LaunchedEffect(pg.items.lastOrNull()?.key, pg.ask?.str("tool_id"), pg.live?.str("tool_id")) {
                if (list.firstVisibleItemIndex <= 2) list.scrollToItem(0)
            }
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth(), state = list, reverseLayout = true,
                // A short thread sits on the composer, as a chat does (final/ 09)
                contentPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.Bottom),
            ) {
                pg.ask?.let { ask -> item("ask-${ask.str("tool_id")}") { QuestionCard(vm, pg, ask) } }
                if (pg.kind == "worker" && pg.row.str("state") == "asking" && pg.row.str("question").isNotEmpty()) item("wask") {
                    QuestionCard(vm, pg, JSONObject().put("text", pg.row.str("question")))
                }
                items(rows, key = { it.key }, contentType = { it::class }) { Row(it, vm, pg.live) { f -> zoom = f } }
                if (rows.isEmpty()) item("empty") { NotePill("Nothing yet", "") }
            }
            PageComposer(vm, pg, a)
        }
    }
    if (pg.details) Details(vm, pg)
    zoom?.let { Zoom(vm, it) { zoom = null } }
}

@Composable
fun QuestionCard(vm: Seimei, pg: Page, ask: JSONObject) {
    val q = ask.optJSONArray("questions")?.optJSONObject(0)
    val canTap = ask.optBoolean("answerable") && pg.reach == null
    // Survives rotation; after a send the options unlock again in 10 s if the same question is still open.
    var picked by rememberSaveable(ask.str("tool_id")) { mutableStateOf(0) }
    var sentAt by rememberSaveable(ask.str("tool_id")) { mutableStateOf(0L) }
    LaunchedEffect(sentAt) {
        if (sentAt > 0) {
            delay(maxOf(0L, 10_000 - (System.currentTimeMillis() - sentAt)))
            picked = 0
            sentAt = 0
        }
    }
    val tk = LocalTokens.current
    // The warm card of a decision (final/ 09): the question glyph on a warn tile, the question, the options a segmented list
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(tk.warnContainer).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.Top) {
            Box(Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(tk.warn), contentAlignment = Alignment.Center) {
                Icon(Icons.question, null, Modifier.size(18.dp), tint = tk.onWarn)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(q?.str("question") ?: ask.str("text").ifEmpty { "A question" }, fontSize = 16.sp, lineHeight = 24.sp,
                    fontWeight = FontWeight.Medium, color = tk.onWarnContainer, modifier = Modifier.semantics { heading() })
            }
        }
        if (q != null && pg.reach == null) Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val opts = q.optJSONArray("options").objects()
            for ((i, o) in opts.withIndex()) {
                val n = i + 1
                val sh = segment(i, opts.size, 20.dp, 4.dp)
                Column(
                    Modifier.fillMaxWidth().heightIn(min = 56.dp).clip(sh)
                        .background(tk.warn.copy(alpha = .10f).compositeOver(tk.warnContainer))
                        .then(if (picked == n) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, sh) else Modifier)
                        .clickable(enabled = canTap && picked == 0) {
                            picked = n
                            vm.pageAnswer(n) { ok -> if (ok) sentAt = System.currentTimeMillis() else picked = 0 }
                        }
                        .padding(horizontal = 14.dp, vertical = 12.dp)
                        .alpha(if (canTap || picked == n) 1f else .7f),
                    verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(o.str("label"), fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = tk.onWarnContainer, modifier = Modifier.weight(1f))
                        if (picked == n && sentAt > 0) Text("Sent", fontSize = 12.sp, color = MaterialTheme.colorScheme.primary)
                    }
                    if (o.str("description").isNotEmpty()) Text(o.str("description"), fontSize = 12.sp, lineHeight = 16.sp, color = tk.onWarnContainer.copy(alpha = .82f))
                }
            }
        }
        if (q != null && !canTap) Text("Answer on the server", fontSize = 12.sp, color = tk.onWarnContainer.copy(alpha = .82f), modifier = Modifier.padding(horizontal = 4.dp))
    }
}

@Composable
fun PageComposer(vm: Seimei, pg: Page, a: Agent) {
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Seimei.MAX_FILES)) { vm.pageAttach(it) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.pageAttach(it) }
    var menu by remember { mutableStateOf(false) }
    val cs = MaterialTheme.colorScheme
    val why = blocked(pg)
    Column(Modifier.fillMaxWidth().onSizeChanged { vm.pageBarPx = it.height }.padding(start = 12.dp, end = 12.dp, bottom = 12.dp)) {
        val note = pg.note ?: why
        if (note != null) Text(note, fontSize = 13.sp, color = if (pg.note != null) cs.error else LocalExtra.current.text3,
            modifier = Modifier.padding(start = 4.dp, end = 4.dp, bottom = 8.dp))
        if (pg.files.isNotEmpty()) Row(Modifier.padding(bottom = 8.dp).horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (d in pg.files) Box {
                val preview = d.preview
                if (preview != null) Image(preview, d.name, Modifier.size(64.dp).clip(RoundedCornerShape(12.dp)).alpha(if (d.uploading) .55f else 1f),
                    contentScale = ContentScale.Crop)
                else Row(
                    Modifier.height(64.dp).widthIn(max = 200.dp).clip(RoundedCornerShape(12.dp)).background(cs.surfaceContainerHigh)
                        .padding(start = 12.dp, end = 30.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (d.uploading) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    else Icon(Icons.doc, null, Modifier.size(20.dp), tint = LocalExtra.current.text2)
                    Spacer(Modifier.width(8.dp))
                    Text(d.name, fontSize = 13.sp, color = LocalExtra.current.text2, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                // A 44dp touch area in the corner around the small visible circle.
                Box(Modifier.align(Alignment.TopEnd).size(44.dp).clickable { vm.pageDetach(d) }, contentAlignment = Alignment.TopEnd) {
                    Box(Modifier.padding(2.dp).size(22.dp).clip(CircleShape).background(Color.Black.copy(alpha = .6f)), contentAlignment = Alignment.Center) {
                        Icon(Icons.close, "Remove ${d.name}", Modifier.size(12.dp), tint = Color.White)
                    }
                }
            }
        }
        Row(verticalAlignment = Alignment.Bottom) {
            val name = if (a.name.length > 22) a.name.take(21).trim() + "…" else a.name
            val x = LocalTokens.current
            val asking = pg.ask != null || (pg.kind == "worker" && pg.row.str("state") == "asking")
            // final/ 09: the 52 pill of the chat's composer, the attach button inside it
            Row(Modifier.weight(1f).heightIn(min = 52.dp).clip(RoundedCornerShape(26.dp)).background(cs.surfaceContainerHigh),
                verticalAlignment = Alignment.Bottom) {
                Box(Modifier.padding(start = 4.dp, bottom = 2.dp)) {
                    IconButton({ menu = true }, Modifier.size(48.dp), enabled = why == null) {
                        Icon(Icons.attach, "Attach photos or files", Modifier.size(22.dp), tint = x.text3)
                    }
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
                val hint = if (why != null) "Can't reply from here right now" else if (asking) "Your answer" else "Message $name"
                BasicTextField(
                    value = pg.draft,
                    onValueChange = { pg.draft = it },
                    enabled = why == null,
                    maxLines = 6,
                    textStyle = TextStyle(fontSize = 15.sp, lineHeight = 22.sp, color = x.text),
                    cursorBrush = SolidColor(cs.primary),
                    modifier = Modifier.weight(1f).padding(start = 8.dp, end = 16.dp, top = 15.dp, bottom = 15.dp).semantics { contentDescription = hint },
                    decorationBox = { field ->
                        Box {
                            if (pg.draft.isEmpty()) Text(hint, fontSize = 15.sp, lineHeight = 22.sp, color = x.text3, maxLines = 1)
                            field()
                        }
                    },
                )
            }
            Spacer(Modifier.width(8.dp))
            val typing = pg.draft.isNotBlank() || pg.files.isNotEmpty()
            // Asking: the answer goes through Send; Stop is in the details sheet.
            if (pg.kind == "worker" && pg.row.str("state") in ACTIVE && pg.row.str("state") != "asking" && !typing) {
                // The square Stop of final/ 10: 16 corners on primary-container
                Box(Modifier.size(52.dp).clip(RoundedCornerShape(16.dp)).background(cs.primaryContainer)
                    .clickable(onClickLabel = "Stop") { vm.workerStop(pg.id) }.semantics { contentDescription = "Stop" },
                    contentAlignment = Alignment.Center) {
                    Box(Modifier.size(14.dp).clip(RoundedCornerShape(4.dp)).background(cs.onPrimaryContainer))
                }
            } else {
                FilledIconButton(vm::pageSend, Modifier.size(52.dp), enabled = typing && why == null) {
                    Icon(Icons.send, "Send", Modifier.size(20.dp))
                }
            }
        }
    }
}

@Composable
fun Fact(key: String, value: String, mono: Boolean = false) {
    if (value.isEmpty()) return
    Row {
        Text(key, fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3, modifier = Modifier.width(104.dp))
        Text(value, fontSize = if (mono) 13.sp else 14.sp, lineHeight = 20.sp, fontFamily = if (mono) FontFamily.Monospace else null)
    }
}

/** A tappable fact: model, effort and permissions of a live shikigami open the Change sheet. */
@Composable
fun TuneFact(key: String, value: String, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable(onClick = onClick).background(cs.surfaceContainerHigh)
        .padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(key, fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3, modifier = Modifier.width(104.dp))
        Text(value.ifEmpty { "default" }, fontSize = 14.sp, lineHeight = 20.sp, modifier = Modifier.weight(1f))
        Icon(Icons.chev, null, Modifier.size(16.dp), tint = LocalExtra.current.text3)
    }
}

val EFFORTS = listOf("low", "medium", "high", "xhigh", "max")
val MODELS = listOf("opus", "sonnet", "haiku")
val MODE_LABEL = mapOf("plan" to "Plan first", "default" to "Ask first", "acceptEdits" to "Allow edits", "auto" to "Auto",
    "dontAsk" to "Only what's allowed", "bypassPermissions" to "Bypass")
val MODE_NOTE = mapOf("acceptEdits" to "Commands still ask", "auto" to "Claude Code decides", "dontAsk" to "Never asks",
    "bypassPermissions" to "The guard still applies")

@Composable
fun Pills(values: List<String>, on: String, pick: (String) -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(cs.surfaceContainerHigh).padding(3.dp)) {
        for (v in values) Text(v, fontSize = 14.sp, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            color = if (v == on) cs.onSecondaryContainer else LocalExtra.current.text2,
            fontWeight = if (v == on) FontWeight.Medium else null,
            modifier = Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(if (v == on) cs.secondaryContainer else Color.Transparent)
                .clickable { pick(v) }.padding(vertical = 9.dp))
    }
}

/** One sheet for effort, model and permissions: only the user changes these here (seimei's tune tool does effort and model). */
@Composable
fun ChangeSheet(vm: Seimei, pg: Page, done: () -> Unit) {
    val r = pg.row
    val cs = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    val effortNow = r.str("effort_next").ifEmpty { r.str("effort") }.ifEmpty { r.str("effort_now") }
    var effort by remember { mutableStateOf(effortNow.ifEmpty { "high" }) }
    var model by remember { mutableStateOf(r.str("model")) }
    var mode by remember { mutableStateOf(r.str("mode")) }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    Text("Change ${r.str("name")}", fontSize = 22.sp, lineHeight = 26.sp)
    Text("Effort", fontSize = 13.sp, color = LocalExtra.current.text3)
    Pills(EFFORTS, effort) { effort = it }
    Text("Model", fontSize = 13.sp, color = LocalExtra.current.text3)
    Pills(MODELS, model) { model = it }
    Text("From its next turn. A new model rebuilds the cache once.",
        fontSize = 13.sp, lineHeight = 18.sp, color = LocalExtra.current.text3)
    Text("Permissions", fontSize = 13.sp, color = LocalExtra.current.text3)
    val shown = listOf("plan", "default", "acceptEdits", "bypassPermissions").let { if (r.str("mode") in it) it else it.take(3) + r.str("mode") + it.last() }
    Column {
        for (m in shown) Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { mode = m }.padding(vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(20.dp).border(2.dp, if (m == mode) cs.primary else LocalExtra.current.text3, CircleShape), contentAlignment = Alignment.Center) {
                if (m == mode) Box(Modifier.size(10.dp).clip(CircleShape).background(cs.primary))
            }
            Spacer(Modifier.width(14.dp))
            Column {
                Text(MODE_LABEL[m] ?: m, fontSize = 15.sp)
                Text(MODE_NOTE[m].orEmpty(), fontSize = 13.sp, color = LocalExtra.current.text3)
            }
        }
    }
    error?.let { Text(it, fontSize = 13.sp, color = cs.error) }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(done) { Text("Cancel") }
        Spacer(Modifier.width(8.dp))
        Button({
            val body = JSONObject()
            if (effort != effortNow) body.put("effort", effort)
            if (model != r.str("model")) body.put("model", model)
            if (mode != r.str("mode")) body.put("mode", mode)
            if (body.length() == 0) return@Button done()
            sending = true
            scope.launch {
                error = vm.workerTune(pg, body)
                sending = false
                if (error == null) done()
            }
        }, enabled = !sending) { Text("Apply") }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun Details(vm: Seimei, pg: Page) {
    val r = pg.row
    val cs = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    LaunchedEffect(pg) { vm.loadMetrics(pg) }
    ModalBottomSheet({ pg.details = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = cs.surfaceContainer) {
        var changing by remember { mutableStateOf(false) }
        Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 24.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (changing) return@Column ChangeSheet(vm, pg) { changing = false }
            Column {
                Text(r.str("name"), fontSize = 22.sp, lineHeight = 26.sp)
                Text(if (pg.kind == "worker") "Shikigami" else "Session ${r.str("kind")}".trim(), fontSize = 13.sp, color = LocalExtra.current.text3)
            }
            // A permission card it waits on.
            if (pg.kind == "worker" && r.str("card").isNotEmpty()) Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(cs.primaryContainer).padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.lock, null, Modifier.size(16.dp), tint = cs.onPrimaryContainer)
                Spacer(Modifier.width(8.dp))
                Text("Needs you: ${r.str("card").removePrefix("wants to ").substringBefore(':')}", fontSize = 14.sp, color = cs.onPrimaryContainer)
            }
            // Facts as a quiet key/value list; what can be done sits below as full-width tonal buttons.
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Fact("Folder", home(r.str("cwd")), mono = true)
                val effort = r.str("effort").ifEmpty { r.str("effort_now") }
                val effortText = if (r.str("effort_next").isNotEmpty()) "${effort.ifEmpty { "default" }} → ${r.str("effort_next")} at its next turn"
                    else if (effort.isEmpty()) "" else effort + if (pg.kind == "worker" && r.str("effort").isEmpty()) " · default" else ""
                // Model, effort and permissions of a live shikigami open the Change sheet.
                if (pg.kind == "worker" && r.str("state") !in listOf("released", "laptop")) {
                    TuneFact("Model", r.str("model")) { changing = true }
                    TuneFact("Effort", effortText) { changing = true }
                    TuneFact("Permissions", MODE_LABEL[r.str("mode")] ?: r.str("mode")) { changing = true }
                } else {
                    Fact("Model", r.str("model"))
                    Fact("Effort", effortText)
                }
                Fact("Compacts at", r.str("context_cap"))
                Fact("Subagents", r.str("subagents"))
                Fact("Context", r.context()?.let { "$it% used" } ?: "")
                if (pg.kind == "worker") Fact("Started", atTime(r.optDouble("created")).replace(Regex("^at "), "today at ").cap())
                else Fact("Last active", atTime(r.optDouble("active")).replace(Regex("^at "), "today at ").cap())
                // What its run took (metrics.py), once the server has priced it.
                if (pg.kind == "worker") pg.metrics?.let { m -> for ((k, v) in metricFacts(m)) Fact(k, v) }
            }
            val state = r.str("state")
            if (pg.kind == "worker" && state != "laptop" && state != "released" && r.optBoolean("started", true)) {
                // A running worker gets stopped first, so it takes a second tap.
                var armed by remember { mutableStateOf(false) }
                var sending by remember { mutableStateOf(false) }
                var error by remember { mutableStateOf<String?>(null) }
                FilledTonalButton({
                    if (state in ACTIVE && !armed) {
                        armed = true
                        return@FilledTonalButton
                    }
                    sending = true
                    scope.launch {
                        error = vm.workerLaptop(pg.id)
                        sending = false
                        armed = false
                    }
                }, Modifier.fillMaxWidth().heightIn(min = 48.dp), enabled = !sending) {
                    Icon(Icons.laptop, null, Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(if (armed) "Stop it and open in a terminal" else "Open in a terminal")
                }
                error?.let { Text("Couldn't open it: $it", fontSize = 13.sp, color = cs.error) }
            }
            if (pg.kind == "worker" && state in ACTIVE) {
                FilledTonalButton({
                    vm.workerStop(pg.id)
                    pg.details = false
                }, Modifier.fillMaxWidth().heightIn(min = 48.dp), colors = ButtonDefaults.filledTonalButtonColors(
                    containerColor = cs.error.copy(alpha = .16f).compositeOver(cs.surfaceContainer), contentColor = cs.error)) {
                    Box(Modifier.size(12.dp).clip(RoundedCornerShape(3.dp)).background(cs.error))
                    Spacer(Modifier.width(8.dp))
                    Text("Stop ${r.str("name")}", maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            val agent = if (pg.kind == "worker") fromWorker(r) else fromSession(r, vm.seen)
            if (agent.state == "done" || agent.state == "idle") FilledTonalButton({
                pg.details = false
                vm.setArchived(agent, !agent.archived)
            }, Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Icon(Icons.shelf, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text(if (agent.archived) "Unarchive" else "Archive")
            }
            if (pg.kind == "worker" && r.str("task").isNotEmpty()) TaskRow(r.str("task"), r.optJSONObject("now"))
            if (pg.kind == "session") Text(if (pg.via == "seimei") "seimei passes messages on with Claude Code's SendMessage: this terminal can't be typed into."
                else pg.reach ?: "Messages you send here are typed into this session's window on the server.",
                fontSize = 13.sp, color = LocalExtra.current.text3)
        }
    }
}

// Its board item now when it has one; the summon text, its first task, opens under it.
@Composable
fun TaskRow(text: String, now: JSONObject? = null) {
    var open by remember(text) { mutableStateOf(false) }
    val head = now?.let { "Now on ${it.str("id")}" } ?: "Task"
    val preview = now?.str("title") ?: text
    val full = if (now != null) "${now.str("title")}\n\nFirst task: $text" else text
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(cs.surfaceContainerHigh)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable { open = !open }.padding(start = 10.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(32.dp).clip(RoundedCornerShape(10.dp)).background(cs.secondaryContainer), contentAlignment = Alignment.Center) {
                Icon(Icons.list, null, Modifier.size(18.dp), tint = cs.onSecondaryContainer)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(head, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                if (!open) Text(preview, fontSize = 13.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Icon(Icons.chev, null, Modifier.size(20.dp).then(if (open) Modifier.rotate(90f) else Modifier), tint = LocalExtra.current.text3)
        }
        if (open) Text(full, fontSize = 14.sp, color = LocalExtra.current.text2,
            modifier = Modifier.heightIn(max = 240.dp).verticalScroll(rememberScrollState()).padding(start = 14.dp, end = 14.dp, bottom = 12.dp))
    }
}

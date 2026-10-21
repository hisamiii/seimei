package com.hisamiowo.seimei

import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.SpanStyle
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

// ---------- tones and words, as on the web page ----------

data class Tone(val bg: Color, val fg: Color, val dot: Color)

val TONE = mapOf(
    "run" to Tone(Color(0xFF3D3552), Color(0xFFECDCFF), Color(0xFFCBA6F7)),
    "ask" to Tone(Color(0xFF45393E), Color(0xFFFFDCC4), Color(0xFFFAB387)),
    "ok" to Tone(Color(0xFF364142), Color(0xFFC8F3C3), Color(0xFFA6E3A1)),
    "bad" to Tone(Color(0xFF45293A), Color(0xFFFFD0DC), Color(0xFFF38BA8)),
    "idle" to Tone(Color(0xFF313244), Color(0xFFCDD6F4), Color(0xFFA6ADC8)),
)

val PEACH = Color(0xFFFAB387)
val MAUVE = Color(0xFFCBA6F7)
val ASK_BG = Color(0xFF2A2633)

/** "at 10:28 PM" today, "on 18 Oct at 10:28 PM" another day. */
fun atTime(t: Double): String =
    if (dayOf(t) == LocalDate.now()) "at ${clock(t)}" else "on ${dayOf(t).format(DateTimeFormatter.ofPattern("d MMM"))} at ${clock(t)}"

fun stateText(w: JSONObject): String {
    val at = atTime(w.optDouble("updated", 0.0))
    val since = at.removePrefix("at ").removePrefix("on ")
    return when (w.str("state")) {
        "starting" -> "Starting"
        "running" -> "Working since $since"
        "asking" -> "Waiting for your answer"
        "done" -> "Finished $at"
        "failed" -> "Failed $at"
        "stopped" -> "Stopped $at"
        "interrupted" -> "Interrupted $at"
        "laptop" -> "Open on the laptop since $since"
        "released" -> "Your session since $since"
        else -> w.str("state")
    }
}

fun String.cap() = replaceFirstChar { it.uppercase() }

/** What a running shikigami is up to: its latest step. */
fun workerLine(w: JSONObject): String {
    val d = w.optJSONObject("doing") ?: return if (w.str("state") == "starting") "Starting" else "Working"
    if (d.str("text").isNotEmpty()) return d.str("text")
    val what = doingPhrase(d.str("name")).cap()
    return if (d.str("summary").isNotEmpty()) "$what: ${d.str("summary")}" else what
}

/** What a session is up to: its open call and how long it has run. */
fun sessionLine(d: JSONObject?): String {
    d ?: return "Working"
    val what = if (d.str("tool") == "Bash") "running ${d.str("what")}" else "${doingPhrase(d.str("tool"))}: ${d.str("what")}"
    val since = d.optDouble("since", 0.0)
    return what.cap() + if (since > 0) " · ${elapsed(since).substringBefore(' ')}" else ""
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

/** Catppuccin accents; a project keeps its color everywhere (the same hash as the web page). */
private val TINTS = listOf(0xFF89B4FA, 0xFFA6E3A1, 0xFF94E2D5, 0xFFCBA6F7, 0xFFF5C2E7, 0xFFB4BEFE, 0xFFF9E2AF, 0xFF74C7EC, 0xFFEBA0AC, 0xFFFAB387).map { Color(it) }

fun tint(project: String): Color {
    if (project.isEmpty()) return Color(0xFFA6ADC8)
    var h = 0L
    for (c in project) h = (h * 31 + c.code) and 0xFFFFFFFFL
    return TINTS[(h % TINTS.size).toInt()]
}

/** One row of the list and the page head, from a shikigami or one of the user's sessions. */
data class Agent(
    val kind: String, val id: String, val name: String, val project: String, val context: Int?, val state: String,
    val since: Double, val line: String, val fresh: Boolean = false, val bad: Boolean = false,
    /** A finished shikigami's end state (stopped, interrupted, ...), for its tile icon. */
    val end: String = "",
    /** Hidden from the list (#43); find: what the Archived search looks in. */
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
            "ask" -> w.str("question").ifEmpty { "Asks a question" }
            "work" -> workerLine(w)
            else -> w.str("summary").ifEmpty { stateText(w) }
        },
        bad = st == "failed", end = st, archived = w.optBoolean("archived"),
        find = listOf(w.str("name"), w.str("task"), w.str("summary")),
    )
}

fun fromSession(s: JSONObject, seen: JSONObject): Agent {
    val q = s.optBoolean("question")
    val asking = q || s.str("status") == "waiting"
    val state = if (asking) "ask" else if (s.str("status") == "busy") "work" else "idle"
    return Agent(
        "session", s.optLong("pid").toString(), s.str("name"), s.str("project"), s.context(), state, s.optDouble("since", 0.0),
        when {
            q -> s.str("ask").ifEmpty { "Asks a question" }
            asking -> s.str("waiting_text").ifEmpty { "Waiting on the laptop" }
            state == "work" -> sessionLine(s.optJSONObject("doing"))
            else -> s.str("line").ifEmpty { "Idle" }
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
            Text("Nothing running", fontWeight = FontWeight.Medium, fontSize = 16.sp)
            Spacer(Modifier.size(6.dp))
            Text("Your Claude sessions on the laptop and the shikigami seimei summons show up here.", color = LocalExtra.current.text3, fontSize = 14.sp)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        for ((title, color, list) in listOf(Triple("Needs you", PEACH, ask), Triple("Working", MAUVE, work), Triple("Idle", LocalExtraText3, idle))) {
            if (list.isEmpty()) continue
            item("h-$title") { GroupTitle(title, color) }
            items(list, key = { "${it.kind}-${it.id}" }) { a ->
                if (a.state == "idle") SwipeToArchive(a, vm) { AgentRow(a, now) { vm.open(a) } } else AgentRow(a, now) { vm.open(a) }
            }
        }
        if (done.isNotEmpty()) {
            val shown = if (showAll) done else done.take(15)
            item("h-done") { GroupTitle("Finished shikigami", LocalExtraText3) }
            items(shown.map(::fromWorker), key = { "done-${it.id}" }) { a -> SwipeToArchive(a, vm) { AgentRow(a, now) { vm.open(a) } } }
            if (done.size > shown.size) item("all") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    TextButton({ showAll = true }) { Text("Show all ${done.size}") }
                }
            }
        }
        if (archived.isNotEmpty()) item("archived") { ArchivedRow(archived.size) { vm.archOpen = true } }
    }
}

// ---------- the archive (#43): hidden from the list, never stopped or deleted ----------

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
        Box(Modifier.clip(RoundedCornerShape(18.dp)).background(cs.surface)) { content() }
    }
}

@Composable
fun ArchivedRow(n: Int, onClick: () -> Unit) {
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
            Text("$n shikigami and session${if (n == 1) "" else "s"}", fontSize = 13.5.sp, color = LocalExtra.current.text3)
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
                placeholder = { Text("Search names, tasks and reports") },
                leadingIcon = { Icon(Icons.search, null, Modifier.size(20.dp)) },
                trailingIcon = { if (vm.archQuery.isNotEmpty()) IconButton({ vm.archQuery = "" }) { Icon(Icons.close, "Clear the search", Modifier.size(18.dp)) } },
                shape = RoundedCornerShape(24.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ))
        }
        if (hits.isEmpty()) item("none") {
            Text(if (q.isEmpty()) "Nothing archived. Shikigami and idle sessions move here 12 hours after their last activity." else "Nothing archived matches.",
                Modifier.fillMaxWidth().padding(32.dp), color = LocalExtra.current.text3, fontSize = 14.sp, textAlign = TextAlign.Center)
        }
        items(hits, key = { "arch-${it.kind}-${it.id}" }) { a -> SwipeToArchive(a, vm) { AgentRow(a, now, q) { vm.open(a) } } }
    }
}

val LocalExtraText3 = Color(0xFFA6ADC8)

fun Seimei.open(a: Agent) = if (a.kind == "worker") openWorker(a.id) else openSession(a.id.toLong())

@Composable
fun GroupTitle(title: String, color: Color) =
    Text(title, color = color, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(start = 8.dp, top = 18.dp, bottom = 6.dp))

@Composable
fun AgentTile(a: Agent) {
    val color = when {
        a.state == "ask" -> PEACH
        a.bad -> MaterialTheme.colorScheme.error
        a.state == "done" -> LocalExtra.current.text3
        else -> tint(a.project)
    }
    val bg = if (a.state == "done") MaterialTheme.colorScheme.surfaceContainerHigh else color.copy(alpha = .18f)
    Box(Modifier.size(42.dp).clip(RoundedCornerShape(14.dp)).background(bg), contentAlignment = Alignment.Center) {
        when (a.state) {
            "work" -> CircularProgressIndicator(Modifier.size(20.dp), color = color, strokeWidth = 2.4.dp, trackColor = Color.Transparent)
            "ask" -> Icon(Icons.question, null, Modifier.size(20.dp), tint = color)
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
fun AgentRow(a: Agent, now: Long, query: String = "", onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clip(RoundedCornerShape(18.dp))
            .background(if (a.state == "ask") ASK_BG else Color.Transparent).clickable(onClick = onClick)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AgentTile(a)
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // Name and tag take the room left of the time, so the time lines up at the right edge.
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    Text(a.name, fontSize = 15.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        color = if (a.state == "done" || a.archived) LocalExtra.current.text2 else Color.Unspecified, modifier = Modifier.weight(1f, fill = false))
                    if (a.kind == "worker") {
                        Spacer(Modifier.width(8.dp))
                        Text("shikigami", fontSize = 11.sp, color = LocalExtra.current.text2, modifier = Modifier.clip(RoundedCornerShape(50))
                            .background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(horizontal = 7.dp, vertical = 1.dp))
                    }
                }
                Spacer(Modifier.width(8.dp))
                if (a.fresh) {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
                    Spacer(Modifier.width(6.dp))
                }
                Text(if (a.state == "done") whenText(a.since) else short(a.since).also { now }, fontSize = 12.5.sp, color = LocalExtra.current.text3)
            }
            Spacer(Modifier.height(3.dp))
            Row {
                if (a.project.isNotEmpty()) Text("${a.project} · ", fontSize = 13.5.sp, color = tint(a.project), fontWeight = FontWeight.Medium, maxLines = 1)
                val (line, at) = searchLine(a, query)
                val mark = MaterialTheme.colorScheme.primary.copy(alpha = .3f)
                Text(buildAnnotatedString {
                    append(line)
                    if (at >= 0) addStyle(SpanStyle(background = mark), at, at + query.length)
                }, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    color = when (a.state) { "ask" -> PEACH; "work" -> MaterialTheme.colorScheme.onSurface; else -> LocalExtra.current.text3 })
            }
        }
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
            .semantics { contentDescription = if (pct == null) "Session details" else "Context $n% used. Session details" },
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
    pg.reach?.let { return it }
    val ask = pg.ask
    if (ask != null && !ask.optBoolean("answerable")) return "Answer on the laptop."
    if (pg.row.str("status") == "waiting" && ask == null) return "${pg.row.str("waiting_text").ifEmpty { "Waiting on the laptop" }}. Answer it there."
    return null
}

@Composable
fun AgentPage(vm: Seimei, pg: Page) {
    BackHandler { if (pg.details) pg.details = false else vm.closePage() }
    val cs = MaterialTheme.colorScheme
    val a = if (pg.kind == "worker") fromWorker(pg.row) else fromSession(pg.row, vm.seen)
    var zoom by remember { mutableStateOf<String?>(null) }
    Surface(Modifier.fillMaxSize(), color = cs.surface) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(vm::closePage, Modifier.size(48.dp)) { Icon(Icons.back, "Back to agents", Modifier.size(22.dp)) }
                Column(Modifier.weight(1f)) {
                    Text(a.name, fontSize = 18.sp, lineHeight = 24.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        val dot = when (a.state) { "work" -> MAUVE; "ask" -> PEACH; else -> Color(0xFF6C7086) }
                        Box(Modifier.size(7.dp).clip(CircleShape).background(dot))
                        Spacer(Modifier.width(6.dp))
                        if (a.project.isNotEmpty()) Text("${a.project} · ", fontSize = 13.sp, color = tint(a.project), fontWeight = FontWeight.Medium)
                        val word = when (a.state) {
                            "ask" -> "Needs you"
                            "work" -> "Working"
                            "idle" -> "Idle"
                            else -> stateText(pg.row)
                        }
                        val ago = short(a.since)
                        Text(if (a.state == "done" || ago == "now") word else "$word $ago", fontSize = 13.sp,
                            color = if (a.state == "ask") PEACH else LocalExtra.current.text3, maxLines = 1)
                    }
                }
                // One tap each way between seimei and the user.
                val canHand = if (pg.kind == "worker") pg.row.optBoolean("started", true) && pg.row.str("state") !in setOf("released", "starting")
                    else pg.row.str("kind") in setOf("interactive", "background")
                if (canHand) FilledTonalButton({ vm.handoff(pg) }, Modifier.heightIn(min = 36.dp).padding(end = 4.dp)
                    .semantics { contentDescription = if (pg.kind == "worker") "Give ${a.name} to me: it goes on as my own session on the laptop"
                        else "Hand ${a.name} to seimei as a shikigami" },
                    contentPadding = PaddingValues(horizontal = 12.dp)) {
                    Text(if (pg.kind == "worker") "Give to me" else "Hand to seimei", fontSize = 13.sp, maxLines = 1)
                }
                ContextRing(a.context) { pg.details = true }
            }
            if (a.archived) Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 4.dp).clip(RoundedCornerShape(16.dp))
                    .background(cs.surfaceContainer).padding(start = 14.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.shelf, null, Modifier.size(20.dp), tint = LocalExtra.current.text3)
                Spacer(Modifier.width(12.dp))
                Text(if (pg.kind == "worker") "Archived. A message wakes it with its full context and puts it back in the list."
                    else "Archived: hidden from the list, still running. It comes back when it does something.",
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
                contentPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                pg.ask?.let { ask -> item("ask-${ask.str("tool_id")}") { QuestionCard(vm, pg, ask) } }
                if (pg.kind == "worker" && pg.row.str("state") == "asking" && pg.row.str("question").isNotEmpty()) item("wask") {
                    QuestionCard(vm, pg, JSONObject().put("text", pg.row.str("question")))
                }
                items(rows, key = { it.key }, contentType = { it::class }) { Row(it, vm, pg.live) { f -> zoom = f } }
                if (rows.isEmpty()) item("empty") { NotePill("Nothing here yet.", "") }
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
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(24.dp)).background(ASK_BG).border(1.5.dp, PEACH, RoundedCornerShape(24.dp)).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(34.dp).clip(RoundedCornerShape(11.dp)).background(PEACH.copy(alpha = .18f)), contentAlignment = Alignment.Center) {
                Icon(Icons.question, null, Modifier.size(18.dp), tint = PEACH)
            }
            Spacer(Modifier.width(10.dp))
            Column {
                Text(q?.str("question") ?: ask.str("text").ifEmpty { "A question" }, fontSize = 15.5.sp, fontWeight = FontWeight.Medium)
                if (q != null && q.str("header").isNotEmpty()) Text(q.str("header"), fontSize = 12.5.sp, color = LocalExtra.current.text3)
            }
        }
        if (q != null) {
            for ((i, o) in q.optJSONArray("options").objects().withIndex()) {
                val n = i + 1
                Column(
                    Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(16.dp))
                        .background(MaterialTheme.colorScheme.surfaceContainerHigh)
                        .then(if (picked == n) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(16.dp)) else Modifier)
                        .clickable(enabled = canTap && picked == 0) {
                            picked = n
                            vm.pageAnswer(n) { ok -> if (ok) sentAt = System.currentTimeMillis() else picked = 0 }
                        }
                        .padding(horizontal = 14.dp, vertical = 12.dp)
                        .alpha(if (canTap || picked == n) 1f else .7f),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(o.str("label"), fontSize = 14.5.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                        if (picked == n && sentAt > 0) Text("Sent", fontSize = 12.sp, color = MaterialTheme.colorScheme.primary)
                    }
                    if (o.str("description").isNotEmpty()) Text(o.str("description"), fontSize = 12.5.sp, lineHeight = 17.sp, color = LocalExtra.current.text3)
                }
            }
            val many = (ask.optJSONArray("questions")?.length() ?: 0) > 1
            Text(
                if (!ask.optBoolean("answerable")) "Answer ${if (many) "these" else "this one"} on the laptop: ${if (many) "several questions at once" else "pick any"} works there only."
                else "Something else? Answer in your own words below.",
                fontSize = 12.5.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(horizontal = 4.dp),
            )
        } else {
            Text("Answer below.", fontSize = 12.5.sp, color = LocalExtra.current.text3, modifier = Modifier.padding(horizontal = 4.dp))
        }
    }
}

@Composable
fun PageComposer(vm: Seimei, pg: Page, a: Agent) {
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Seimei.MAX_FILES)) { vm.pageAttach(it) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.pageAttach(it) }
    var menu by remember { mutableStateOf(false) }
    val cs = MaterialTheme.colorScheme
    val why = blocked(pg)
    Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 10.dp)) {
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
            TextField(
                value = pg.draft,
                onValueChange = { pg.draft = it },
                enabled = why == null,
                placeholder = { Text(if (why != null) "Can't reply from here right now" else if (pg.ask != null) "Your answer…" else if (pg.kind == "worker" && a.archived) "Message $name to wake it" else "Message $name", maxLines = 1) },
                leadingIcon = {
                    Box {
                        IconButton({ menu = true }, enabled = why == null) { Icon(Icons.attach, "Attach photos or files") }
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
                maxLines = 6,
                shape = RoundedCornerShape(28.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                    disabledContainerColor = cs.surfaceContainerHigh,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent, disabledIndicatorColor = Color.Transparent,
                ),
                modifier = Modifier.weight(1f).heightIn(min = 56.dp),
            )
            Spacer(Modifier.width(8.dp))
            val typing = pg.draft.isNotBlank() || pg.files.isNotEmpty()
            // Asking: the answer goes through Send; Stop is in the details sheet.
            if (pg.kind == "worker" && pg.row.str("state") in ACTIVE && pg.row.str("state") != "asking" && !typing) {
                FilledTonalIconButton({ vm.workerStop(pg.id) }, Modifier.size(56.dp)) {
                    Box(Modifier.size(14.dp).clip(RoundedCornerShape(3.dp)).background(cs.onSecondaryContainer).semantics { contentDescription = "Stop" })
                }
            } else {
                FilledIconButton(vm::pageSend, Modifier.size(56.dp), enabled = typing && why == null) {
                    Icon(Icons.send, "Send", Modifier.size(22.dp))
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
val MODE_LABEL = mapOf("plan" to "Plan first", "default" to "Ask", "acceptEdits" to "Allow edits", "auto" to "Auto",
    "dontAsk" to "Only what's allowed", "bypassPermissions" to "Bypass")
val MODE_NOTE = mapOf("plan" to "It plans, you OK the plan, then it works.", "default" to "What Claude Code would ask comes to you as a card.",
    "acceptEdits" to "File edits go through; commands still ask.", "auto" to "Claude Code decides what is safe; the rest asks.",
    "dontAsk" to "Only what your rules allow; nothing asks.", "bypassPermissions" to "Nothing asks. seimei's guard still applies.")

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
    Text("Takes effect at its next turn, without restarting the task. A new effort keeps the cache; a new model rebuilds it once.",
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
            // A permission card it waits on (#64).
            if (pg.kind == "worker" && r.str("card").isNotEmpty()) Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(cs.primaryContainer).padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.lock, null, Modifier.size(16.dp), tint = cs.onPrimaryContainer)
                Spacer(Modifier.width(8.dp))
                Text("Waits for your OK: ${r.str("card").removePrefix("wants to ").substringBefore(':')}", fontSize = 14.sp, color = cs.onPrimaryContainer)
            }
            // Facts as a quiet key/value list; what can be done sits below as full-width tonal buttons.
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Fact("Folder", home(r.str("cwd")), mono = true)
                val effort = r.str("effort").ifEmpty { r.str("effort_now") }
                val effortText = if (r.str("effort_next").isNotEmpty()) "${effort.ifEmpty { "default" }} → ${r.str("effort_next")} at its next turn"
                    else if (effort.isEmpty()) "" else effort + if (pg.kind == "worker" && r.str("effort").isEmpty()) " (default)" else ""
                // Model, effort and permissions of a live shikigami open the Change sheet (#64, #74).
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
                // What its run took (metrics.py), once the laptop has priced it.
                if (pg.kind == "worker") pg.metrics?.let { m -> for ((k, v) in metricFacts(m)) Fact(k, v) }
            }
            val state = r.str("state")
            if (pg.kind == "worker" && state != "laptop" && state != "released" && r.optBoolean("started")) {
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
                    Text(if (armed) "Stop it and open on laptop" else "On laptop")
                }
                error?.let { Text("Could not open it: $it", fontSize = 13.sp, color = cs.error) }
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
            if (pg.kind == "worker" && r.str("task").isNotEmpty()) TaskRow(r.str("task"))
            if (pg.kind == "session") Text(pg.reach ?: "Messages you send here are typed into this session's window on the laptop.",
                fontSize = 13.sp, color = LocalExtra.current.text3)
        }
    }
}

@Composable
fun TaskRow(text: String) {
    var open by remember(text) { mutableStateOf(false) }
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(cs.surfaceContainerHigh)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable { open = !open }.padding(start = 10.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(32.dp).clip(RoundedCornerShape(10.dp)).background(cs.secondaryContainer), contentAlignment = Alignment.Center) {
                Icon(Icons.list, null, Modifier.size(18.dp), tint = cs.onSecondaryContainer)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text("Task", fontSize = 14.sp, fontWeight = FontWeight.Medium)
                if (!open) Text(text, fontSize = 13.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Icon(Icons.chev, null, Modifier.size(20.dp).then(if (open) Modifier.rotate(90f) else Modifier), tint = LocalExtra.current.text3)
        }
        if (open) Text(text, fontSize = 14.sp, color = LocalExtra.current.text2,
            modifier = Modifier.heightIn(max = 240.dp).verticalScroll(rememberScrollState()).padding(start = 14.dp, end = 14.dp, bottom = 12.dp))
    }
}

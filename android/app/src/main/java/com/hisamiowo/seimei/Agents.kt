package com.hisamiowo.seimei

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
        bad = st == "failed", end = st,
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
    )
}

/** Working agents, and how many things wait on the user. */
fun agentCounts(vm: Seimei): Pair<Int, Int> {
    val all = vm.workers.map(::fromWorker) + vm.sessions.map { fromSession(it, vm.seen) }
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
    var showAll by remember { mutableStateOf(false) }
    val items = vm.workers.filter { it.str("state") in ACTIVE }.map(::fromWorker) + vm.sessions.map { fromSession(it, vm.seen) }
    fun by(st: String) = items.filter { it.state == st }.sortedByDescending { it.since }
    val ask = by("ask")
    val work = by("work")
    val idle = by("idle")
    val done = (vm.allWorkers ?: vm.workers).filter { it.str("state") !in ACTIVE }.sortedByDescending { it.optDouble("updated") }
    if (items.isEmpty() && done.isEmpty()) {
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
            items(list, key = { "${it.kind}-${it.id}" }) { a -> AgentRow(a, now) { vm.open(a) } }
        }
        if (done.isNotEmpty()) {
            val shown = if (showAll) done else done.take(15)
            item("h-done") { GroupTitle("Finished shikigami", LocalExtraText3) }
            items(shown.map(::fromWorker), key = { "done-${it.id}" }) { a -> AgentRow(a, now) { vm.open(a) } }
            if (done.size > shown.size) item("all") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    TextButton({ showAll = true }) { Text("Show all ${done.size}") }
                }
            }
        }
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
fun AgentRow(a: Agent, now: Long, onClick: () -> Unit) {
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
                        color = if (a.state == "done") LocalExtra.current.text2 else Color.Unspecified, modifier = Modifier.weight(1f, fill = false))
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
                Text(a.line, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
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
                ContextRing(a.context) { pg.details = true }
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
    var picked by remember(ask.str("tool_id")) { mutableStateOf(0) }
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
                            vm.pageAnswer(n) { ok -> if (!ok) picked = 0 }
                        }
                        .padding(horizontal = 14.dp, vertical = 12.dp)
                        .alpha(if (canTap || picked == n) 1f else .7f),
                ) {
                    Text(o.str("label"), fontSize = 14.5.sp, fontWeight = FontWeight.Medium)
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
                Box(Modifier.align(Alignment.TopEnd).padding(2.dp).size(22.dp).clip(CircleShape).background(Color.Black.copy(alpha = .6f))
                    .clickable { vm.pageDetach(d) }, contentAlignment = Alignment.Center) {
                    Icon(Icons.close, "Remove ${d.name}", Modifier.size(12.dp), tint = Color.White)
                }
            }
        }
        Row(verticalAlignment = Alignment.Bottom) {
            val name = if (a.name.length > 22) a.name.take(21).trim() + "…" else a.name
            TextField(
                value = pg.draft,
                onValueChange = { pg.draft = it },
                enabled = why == null,
                placeholder = { Text(if (why != null) "Can't reply from here right now" else if (pg.ask != null) "Your answer…" else "Message $name", maxLines = 1) },
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
            if (pg.kind == "worker" && pg.row.str("state") in ACTIVE && !typing) {
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
fun Fact(text: String, mono: Boolean = false) = Text(
    text, fontSize = if (mono) 12.sp else 13.sp, color = LocalExtra.current.text2, fontFamily = if (mono) FontFamily.Monospace else null,
    modifier = Modifier.heightIn(min = 30.dp).border(1.dp, Color(0xFF585B70), RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 6.dp),
)

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun Details(vm: Seimei, pg: Page) {
    val r = pg.row
    val cs = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    ModalBottomSheet({ pg.details = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = cs.surfaceContainer) {
        Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Column {
                Text(r.str("name"), fontSize = 22.sp, lineHeight = 26.sp)
                Text(if (pg.kind == "worker") "Shikigami" else "Session ${r.str("kind")}".trim(), fontSize = 13.sp, color = LocalExtra.current.text3)
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Fact(home(r.str("cwd")), mono = true)
                if (r.str("model").isNotEmpty()) Fact(r.str("model"))
                val effort = r.str("effort").ifEmpty { r.str("effort_now") }
                if (effort.isNotEmpty()) Fact("effort $effort" + if (pg.kind == "worker" && r.str("effort").isEmpty()) " (default)" else "")
                if (r.str("context_cap").isNotEmpty()) Fact("compacts at ${r.str("context_cap")}")
                if (r.str("subagents").isNotEmpty()) Fact("subagents on ${r.str("subagents")}")
                r.context()?.let { Fact("context $it% used") }
                Fact(if (pg.kind == "worker") "started ${atTime(r.optDouble("created"))}" else "last active ${atTime(r.optDouble("active"))}")
                val state = r.str("state")
                if (pg.kind == "worker" && state != "laptop" && r.optBoolean("started")) {
                    // A running worker gets stopped first, so it takes a second tap.
                    var armed by remember { mutableStateOf(false) }
                    var sending by remember { mutableStateOf(false) }
                    var error by remember { mutableStateOf<String?>(null) }
                    Row(
                        Modifier.heightIn(min = 30.dp).clip(RoundedCornerShape(8.dp)).background(cs.secondaryContainer)
                            .clickable(enabled = !sending) {
                                if (state in ACTIVE && !armed) {
                                    armed = true
                                    return@clickable
                                }
                                sending = true
                                scope.launch {
                                    error = vm.workerLaptop(pg.id)
                                    sending = false
                                    armed = false
                                }
                            }.padding(horizontal = 10.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.laptop, null, Modifier.size(16.dp), tint = cs.onSecondaryContainer)
                        Spacer(Modifier.width(6.dp))
                        Text(if (armed) "Stop it and open on laptop" else "On laptop", fontSize = 13.sp, color = cs.onSecondaryContainer)
                    }
                    error?.let { Text("Could not open it: $it", fontSize = 13.sp, color = cs.error) }
                }
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

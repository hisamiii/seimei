package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.time.Instant
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

val STATE_TONE = mapOf(
    "starting" to "run", "running" to "run", "asking" to "ask", "done" to "idle",
    "failed" to "bad", "stopped" to "idle", "interrupted" to "idle", "laptop" to "idle",
)

fun stateText(w: JSONObject): String {
    val at = clock(w.optDouble("updated", 0.0))
    return when (w.str("state")) {
        "starting" -> "Starting"
        "running" -> "Working since $at"
        "asking" -> "Waiting for your answer"
        "done" -> "Finished at $at"
        "failed" -> "Failed at $at"
        "stopped" -> "Stopped at $at"
        "interrupted" -> "Interrupted at $at"
        "laptop" -> "Open on the laptop since $at"
        else -> w.str("state")
    }
}

/** What a running shikigami is up to: its latest step. */
fun doing(w: JSONObject): String {
    val d = w.optJSONObject("doing") ?: return if (w.str("state") == "starting") "Starting" else "Working"
    if (d.str("text").isNotEmpty()) return d.str("text")
    val what = phrase(d.str("name"), 1)
    return if (d.str("summary").isNotEmpty()) "${what.replaceFirstChar { it.uppercase() }}: ${d.str("summary")}" else what
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

/** What one of the user's sessions needs: an open question first, then a finished turn not yet looked at. */
data class Look(val label: String, val tone: String, val rank: Int, val ask: Boolean = false, val fresh: Boolean = false)

fun question(s: JSONObject): String = (s.opt("question") as? String).orEmpty()

fun look(s: JSONObject, seen: JSONObject): Look {
    val status = s.str("status")
    return when {
        question(s).isNotEmpty() -> Look("question", "ask", 0, ask = true)
        status == "waiting" -> Look(s.str("waiting_for").ifEmpty { "waiting" }, "ask", 0, ask = true)
        status == "busy" -> Look("working", "run", 2)
        status == "idle" && s.optDouble("active", 0.0) > seen.optDouble(s.str("session_id"), 0.0) + 1 -> Look("new reply", "idle", 1, fresh = true)
        else -> Look(status, "idle", 3)
    }
}

/** Working shikigami, and how many things wait on the user. */
fun agentCounts(vm: Seimei): Pair<Int, Int> {
    val working = vm.workers.count { it.str("state") in ACTIVE }
    val asks = vm.workers.count { it.str("state") == "asking" } + vm.sessions.count { look(it, vm.seen).ask }
    return working to asks
}

fun letter(name: String) = name.trim().take(1).lowercase().ifEmpty { "?" }

// ---------- the Agents tab ----------

@Composable
fun AgentsTab(vm: Seimei) {
    // Relative times and the fading of old rows move on by themselves.
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(30_000)
            value = System.currentTimeMillis()
        }
    }
    var showAll by remember { mutableStateOf(false) }
    val live = vm.workers.filter { it.str("state") in ACTIVE }
        .sortedWith(compareBy<JSONObject> { if (it.str("state") == "asking") 0 else 1 }.thenByDescending { it.optDouble("updated") })
    val sessions = vm.sessions.sortedWith(compareBy<JSONObject> { look(it, vm.seen).rank }.thenByDescending { it.optDouble("active") })
    val done = (vm.allWorkers ?: vm.workers).filter { it.str("state") !in ACTIVE }.sortedByDescending { it.optDouble("updated") }
    if (live.isEmpty() && sessions.isEmpty() && done.isEmpty()) {
        Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            Text("No shikigami yet", fontWeight = FontWeight.Medium, fontSize = 16.sp)
            Spacer(Modifier.size(6.dp))
            Text("Ask seimei to summon one, and it shows up here while it works.", color = LocalExtra.current.text3, fontSize = 14.sp)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp)) {
        if (live.isNotEmpty()) {
            val asks = live.count { it.str("state") == "asking" }
            item("h-live") { Section("Shikigami", if (asks > 0) "${live.size} working · $asks needs you" else "${live.size} working") }
            item("live") {
                Group {
                    for (w in live) {
                        val asking = w.str("state") == "asking"
                        AgentRow(
                            letter(w.str("name")), if (asking) "ask" else "run", w.str("name"),
                            if (asking) "Asks: ${w.str("question").ifEmpty { "a question" }}" else doing(w),
                            onClick = { vm.openWorker(w.str("id")) },
                        ) { if (asking) AnswerPill() else Spinner() }
                    }
                }
            }
        }
        if (sessions.isNotEmpty()) {
            val asks = sessions.count { look(it, vm.seen).ask }
            item("h-sessions") { Section("Your sessions", if (asks > 0) "$asks needs you" else "") }
            item("sessions") {
                Group {
                    for (s in sessions) {
                        val l = look(s, vm.seen)
                        val sup = if (l.ask) (question(s).lineSequence().firstOrNull()?.let { "Asks: $it" } ?: "Waiting for ${l.label}")
                        else "${if (s.str("status") == "busy") "Working" else if (l.fresh) "New reply" else "Idle"} · ${home(s.str("cwd"))}"
                        AgentRow(letter(s.str("name")), if (l.ask) "ask" else "user", s.str("name"), sup,
                            onClick = { vm.openSession(s.optLong("pid")) }) {
                            when {
                                l.ask -> AnswerPill()
                                s.str("status") == "busy" -> Spinner()
                                else -> Trail(short(s.optDouble("since")), l.fresh)
                            }
                        }
                    }
                }
            }
        }
        if (done.isNotEmpty()) {
            val shown = if (showAll) done else done.take(15)
            item("h-done") { Section("Finished", "") }
            item("done") {
                Group {
                    for (w in shown) {
                        val age = now / 1000.0 - w.optDouble("updated")
                        val state = w.str("state")
                        AgentRow(
                            "", if (state == "failed") "donebad" else "done", w.str("name"), w.str("summary").ifEmpty { stateText(w) },
                            icon = when (state) { "failed" -> Icons.alert; "done" -> Icons.check; else -> Icons.dash },
                            fade = if (age < 3600) .8f else if (age < 86400) .6f else .45f,
                            onClick = { vm.openWorker(w.str("id")) },
                        ) { Trail(whenText(w.optDouble("updated")), false) }
                    }
                }
            }
            if (done.size > shown.size) item("all") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    TextButton({ showAll = true }) { Text("Show all ${done.size}") }
                }
            }
        }
    }
}

@Composable
fun Section(title: String, note: String) {
    Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 16.dp, bottom = 8.dp), verticalAlignment = Alignment.Bottom) {
        Text(title, color = MaterialTheme.colorScheme.primary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
        if (note.isNotEmpty()) Text(note, fontSize = 12.sp, color = LocalExtra.current.text3)
    }
}

@Composable
fun Group(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)), verticalArrangement = Arrangement.spacedBy(2.dp)) { content() }
}

@Composable
fun AgentRow(
    av: String, avTone: String, name: String, sup: String,
    icon: ImageVector? = null, fade: Float = 1f, onClick: () -> Unit, trail: @Composable () -> Unit,
) {
    val cs = MaterialTheme.colorScheme
    val (bg, fg) = when (avTone) {
        "ask" -> TONE["ask"]!!.bg to TONE["ask"]!!.fg
        "user" -> Color.Transparent to LocalExtra.current.text2
        "done" -> cs.surfaceContainerHigh to LocalExtra.current.text3
        "donebad" -> cs.surfaceContainerHigh to cs.error
        else -> cs.primaryContainer to cs.onPrimaryContainer
    }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 72.dp).background(cs.surfaceContainer).clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.alpha(fade).size(40.dp).clip(CircleShape).background(bg)
                .then(if (avTone == "user") Modifier.border(1.dp, cs.outline, CircleShape) else Modifier),
            contentAlignment = Alignment.Center,
        ) {
            if (icon != null) Icon(icon, null, Modifier.size(20.dp), tint = fg)
            else Text(av, color = fg, fontSize = 16.sp, fontWeight = FontWeight.Medium)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f).alpha(fade)) {
            Text(name, fontSize = 16.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(sup, fontSize = 14.sp, lineHeight = 19.sp, color = LocalExtra.current.text3, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(10.dp))
        Box(Modifier.alpha(fade)) { trail() }
    }
}

@Composable
fun Spinner() = CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.5.dp)

@Composable
fun AnswerPill() = Text(
    "Answer", fontSize = 13.sp, fontWeight = FontWeight.Medium, color = MaterialTheme.colorScheme.onSecondaryContainer,
    modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.secondaryContainer).padding(horizontal = 14.dp, vertical = 7.dp),
)

@Composable
fun Trail(text: String, fresh: Boolean) = Row(verticalAlignment = Alignment.CenterVertically) {
    if (fresh) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
        Spacer(Modifier.width(6.dp))
    }
    Text(text, fontSize = 12.sp, color = LocalExtra.current.text3)
}

// ---------- the sheets ----------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentSheet(vm: Seimei) {
    val sh = vm.sheet ?: return
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(vm::closeSheet, sheetState = state, containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxHeight(.92f).imePadding().padding(horizontal = 20.dp)) {
            when (sh) {
                is Sheet.Worker -> WorkerSheet(vm, sh)
                is Sheet.Session -> SessionSheet(vm, sh)
            }
        }
    }
}

@Composable
fun SheetHead(tone: String, title: String, sub: String) {
    val t = TONE[tone] ?: TONE["idle"]!!
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(t.bg), contentAlignment = Alignment.Center) {
            Box(Modifier.size(12.dp).clip(CircleShape).background(t.dot))
        }
        Spacer(Modifier.width(14.dp))
        Column {
            Text(title, fontSize = 22.sp, lineHeight = 26.sp)
            Text(sub, fontSize = 13.sp, color = LocalExtra.current.text3)
        }
    }
}

@Composable
fun Fact(text: String, mono: Boolean = false) = Text(
    text, fontSize = if (mono) 12.sp else 13.sp, color = LocalExtra.current.text2, fontFamily = if (mono) FontFamily.Monospace else null,
    modifier = Modifier.heightIn(min = 30.dp).border(1.dp, Color(0xFF585B70), RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 6.dp),
)

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ColumnScope.WorkerSheet(vm: Seimei, sh: Sheet.Worker) {
    val w = sh.info
    val id = w.str("id")
    val name = w.str("name")
    val state = w.str("state")
    val cs = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    var zoom by remember { mutableStateOf<String?>(null) }
    SheetHead(STATE_TONE[state] ?: "idle", name, stateText(w))
    Spacer(Modifier.size(12.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Fact(home(w.str("cwd")), mono = true)
        Fact(w.str("model"))
        Fact("started ${clock(w.optDouble("created"))}")
        if (state != "laptop" && w.optBoolean("started")) {
            // A running worker gets stopped first, so it takes a second tap.
            var armed by remember(id) { mutableStateOf(false) }
            var sending by remember(id) { mutableStateOf(false) }
            var error by remember(id) { mutableStateOf<String?>(null) }
            Row(
                Modifier.heightIn(min = 30.dp).clip(RoundedCornerShape(8.dp)).background(cs.secondaryContainer)
                    .clickable(enabled = !sending) {
                        if (state in ACTIVE && !armed) {
                            armed = true
                            return@clickable
                        }
                        sending = true
                        scope.launch {
                            error = vm.workerLaptop(id)
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
    Spacer(Modifier.size(12.dp))
    TaskRow(sh.task)
    val q = w.str("question")
    if (state == "asking" && q.isNotEmpty()) {
        Spacer(Modifier.size(12.dp))
        Text(q, fontSize = 14.sp, color = TONE["ask"]!!.fg,
            modifier = Modifier.fillMaxWidth().heightIn(max = 160.dp).clip(RoundedCornerShape(16.dp)).background(TONE["ask"]!!.bg)
                .verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 12.dp))
    }
    // The log, newest at the bottom; a reader near the end follows it.
    val list = rememberLazyListState()
    val rows = sh.items.asReversed()
    LaunchedEffect(sh.items.lastOrNull()?.key) { if (list.firstVisibleItemIndex <= 2) list.scrollToItem(0) }
    LazyColumn(
        Modifier.weight(1f).fillMaxWidth(), state = list, reverseLayout = true,
        contentPadding = PaddingValues(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        items(rows, key = { it.key }, contentType = { it::class }) { Row(it, vm) { f -> zoom = f } }
    }
    SheetComposer(if (state == "asking") "Answer $name" else "Message $name", state in ACTIVE, onStop = { vm.workerStop(id) }) { text, fail ->
        vm.workerSend(id, text, fail)
    }
    zoom?.let { Zoom(vm, it) { zoom = null } }
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
            Icon(Icons.chev, null, Modifier.size(20.dp).then(if (open) Modifier.rotate90() else Modifier), tint = LocalExtra.current.text3)
        }
        if (open) Text(text, fontSize = 14.sp, color = LocalExtra.current.text2,
            modifier = Modifier.heightIn(max = 240.dp).verticalScroll(rememberScrollState()).padding(start = 14.dp, end = 14.dp, bottom = 12.dp))
    }
}

@Composable
fun SheetComposer(hint: String, running: Boolean, onStop: () -> Unit, onSend: (String, () -> Unit) -> Unit) {
    var text by remember { mutableStateOf("") }
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 12.dp), verticalAlignment = Alignment.Bottom) {
        TextField(
            text, { text = it }, placeholder = { Text(hint) }, maxLines = 5, shape = RoundedCornerShape(28.dp),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
            ),
            modifier = Modifier.weight(1f).heightIn(min = 56.dp),
        )
        Spacer(Modifier.width(8.dp))
        FilledIconButton({
            val t = text.trim()
            if (t.isNotEmpty()) {
                text = ""
                onSend(t) { if (text.isEmpty()) text = t }
            }
        }, Modifier.size(56.dp), enabled = text.isNotBlank()) { Icon(Icons.send, "Send", Modifier.size(22.dp)) }
        if (running) {
            Spacer(Modifier.width(8.dp))
            FilledTonalIconButton(onStop, Modifier.size(56.dp)) {
                Box(Modifier.size(14.dp).clip(RoundedCornerShape(3.dp)).background(cs.onSecondaryContainer))
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ColumnScope.SessionSheet(vm: Seimei, sh: Sheet.Session) {
    val r = sh.data
    val s = r.getJSONObject("session")
    val l = if (s.str("session_id").isNotEmpty()) look(s, vm.seen) else Look(s.str("status"), "idle", 3)
    val label = if (l.label == "new reply") "idle" else l.label
    val msgs = buildList {
        for ((role, key) in listOf("user" to "user", "assistant" to "assistant")) {
            val arr = r.optJSONArray(key) ?: continue
            for (i in 0 until arr.length()) {
                val pair = arr.getJSONArray(i)
                add(Triple(pair.getString(0), role, pair.getString(1)))
            }
        }
    }.sortedBy { it.first }
    var zoom by remember { mutableStateOf<String?>(null) }
    SheetHead(l.tone, s.str("name"), "$label for ${short(s.optDouble("since")).replace("now", "a moment")}")
    Spacer(Modifier.size(12.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Fact(home(s.str("cwd")), mono = true)
        if (s.str("kind").isNotEmpty()) Fact(s.str("kind"))
        Fact("last active ${clock(s.optDouble("active"))}")
    }
    val list = rememberLazyListState()
    LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, reverseLayout = true,
        contentPadding = PaddingValues(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item("note") { Text("Read-only. Ask seimei to message this session.", fontSize = 13.sp, color = LocalExtra.current.text3) }
        val calls = r.optJSONArray("open_calls").objects()
        for ((i, c) in calls.withIndex().reversed()) item("call$i") {
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceContainerLow).padding(horizontal = 12.dp, vertical = 10.dp)) {
                Text("${c.str("tool")} since ${clock(isoTime(c.str("since")))}", fontSize = 13.sp, color = LocalExtra.current.text3)
                Text(c.str("what"), fontSize = 12.sp, fontFamily = FontFamily.Monospace, color = LocalExtra.current.text2, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
        }
        val questions = r.optJSONArray("questions").objects()
        for ((i, q) in questions.withIndex().reversed()) item("q$i") { TerminalQuestion(q) }
        if (msgs.isEmpty()) item("empty") { NotePill("No messages yet.", "") }
        items(msgs.asReversed().mapIndexed { i, m -> Item.Msg("m$i", m.second, m.third, emptyList(), isoTime(m.first)) }, key = { it.key }) {
            Row(it, vm) { f -> zoom = f }
        }
    }
    zoom?.let { Zoom(vm, it) { zoom = null } }
}

@Composable
fun TerminalQuestion(q: JSONObject) {
    val t = TONE["ask"]!!
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(t.bg).padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Waiting on a question in the terminal", color = t.fg, fontWeight = FontWeight.SemiBold, fontSize = 14.sp)
        for (item in q.optJSONArray("questions").objects()) {
            Text(item.str("question") + if (item.optBoolean("multiSelect")) " (pick any)" else "", fontSize = 14.sp)
            for ((i, o) in item.optJSONArray("options").objects().withIndex()) {
                Column(Modifier.padding(start = 8.dp)) {
                    Text("${i + 1}. ${o.str("label")}", fontSize = 14.sp)
                    if (o.str("description").isNotEmpty()) Text(o.str("description"), fontSize = 12.sp, color = LocalExtra.current.text3)
                }
            }
        }
    }
}

fun isoTime(s: String): Double = try {
    Instant.parse(s).toEpochMilli() / 1000.0
} catch (e: Exception) {
    0.0
}

package com.hisamiowo.seimei

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

// The Tasks lane: seimei's board by project, as on the web page (tasks.js). Each project is a section with a thin
// rule in its Agents-tab tint; done items fold into one "N done" line, paused projects stay shut until opened. The
// user ticks their own ("you") items; a task with a shikigami opens its page. Each owner's first open item is marked
// next; a task's steps (its shikigami's own todo list) show under it while it works, folded otherwise.

/** One step of a shikigami's own todo list: pending, in_progress or completed. */
data class Step(val title: String, val state: String)

data class Task(val id: String, val title: String, val owner: String, val state: String, val worker: String?,
                val pos: Int = 0, val steps: List<Step> = emptyList(), val next: Boolean = false) {
    val you get() = owner == "you"
    /** Steps show by default while it runs or asks. */
    val working get() = state == "running" || state == "asks"
}

/** How live work sorts within a project (then by its owner's queue). */
private val ORDER = mapOf("asks" to 0, "running" to 1, "review" to 2, "open" to 3, "paused" to 4)

/** A project as the board shows it: [label] says on the right how it stands, [live] runs first, [done] newest first. */
data class Section(val name: String, val color: String?, val paused: Boolean, val label: String, val live: List<Task>, val done: List<Task>)

/** The board's projects in the order they show: what asks first, then running work, paused last, otherwise as the
 * server keeps them. */
fun sections(board: JSONObject): List<Section> {
    val tasks = board.optJSONArray("tasks").objects()
    // Each owner's next item: their first open task in queue order.
    val nexts = tasks.sortedBy { it.optInt("pos") }.filter { it.str("state") == "open" }.distinctBy { it.str("owner") }.map { it.str("id") }.toSet()
    fun task(t: JSONObject) = Task(
        t.str("id"), t.str("title"), t.str("owner"), t.str("state"), t.str("worker").ifEmpty { null }, t.optInt("pos"),
        t.optJSONArray("steps").objects().map { Step(it.str("title"), it.str("state")) }, t.str("id") in nexts,
    )
    return board.optJSONArray("projects").objects().mapNotNull { p ->
        val name = p.str("name")
        val ts = tasks.filter { it.str("project") == name }
        if (ts.isEmpty()) return@mapNotNull null
        val live = ts.filter { it.str("state") != "done" }.map(::task).sortedWith(compareBy({ ORDER[it.state] ?: 3 }, { it.pos }))
        val done = ts.filter { it.str("state") == "done" }.sortedByDescending { it.optDouble("done_at", 0.0).takeIf { d -> !d.isNaN() } ?: 0.0 }.map(::task)
        val paused = p.optBoolean("paused")
        val (label, rank) = when {
            paused -> "paused" to 6
            live.any { it.state == "asks" } -> "asks" to 0
            live.any { it.state == "running" } -> "running" to 1
            live.any { it.state == "review" } -> "review" to 2
            live.isNotEmpty() && live.all { it.you } -> "you" to 4
            live.isNotEmpty() -> "open" to 3
            else -> "done" to 5
        }
        rank to Section(name, p.str("color").ifEmpty { null }, paused, label, live, done)
    }.withIndex().sortedWith(compareBy({ it.value.first }, { it.index })).map { it.value.second }
}

private fun colorOf(hex: String?): Color? = hex?.removePrefix("#")?.takeIf { it.length == 6 }?.toLongOrNull(16)?.let { Color(0xFF000000 or it) }

@Composable
fun TasksTab(vm: Seimei) {
    val b = vm.board
    val secs = remember(b) { b?.let(::sections).orEmpty() }
    // Done items shown in full, per project: for this visit only, like the web page.
    var doneShown by rememberSaveable { mutableStateOf(listOf<String>()) }
    // Tasks whose steps were opened or shut by hand (they show by default while the task runs or asks).
    var flipped by rememberSaveable { mutableStateOf(listOf<String>()) }
    Column(Modifier.fillMaxSize()) {
        LaneTabs(vm)
        if (secs.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(if (b == null) "Loading the board" else "Nothing on the board. seimei adds what it queues here.",
                    Modifier.padding(horizontal = 48.dp), fontSize = 14.sp, color = LocalExtra.current.text3, textAlign = TextAlign.Center)
            }
            return@Column
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 10.dp, bottom = 24.dp)) {
            items(secs, key = { it.name }) { s ->
                val open = if (vm.boardOpen.has(s.name)) vm.boardOpen.optBoolean(s.name) else !s.paused
                Project(vm, s, open, s.name in doneShown, first = s === secs.first(), flipped = flipped,
                    flip = { id -> flipped = if (id in flipped) flipped - id else flipped + id }) {
                    doneShown = if (s.name in doneShown) doneShown - s.name else doneShown + s.name
                }
            }
        }
    }
}

@Composable
private fun Project(vm: Seimei, s: Section, open: Boolean, showDone: Boolean, first: Boolean, flipped: List<String>,
                    flip: (String) -> Unit, fold: () -> Unit) {
    val x = LocalExtra.current
    val rule = colorOf(s.color) ?: tint(s.name)
    Column(
        Modifier.fillMaxWidth().padding(top = if (first) 2.dp else if (open) 14.dp else 2.dp)
            .drawBehind {
                val top = (if (open) 12 else 10).dp.toPx()
                val bottom = (if (open) 8 else 10).dp.toPx()
                drawRoundRect(rule.copy(alpha = if (open) .55f else .25f), Offset(0f, top), Size(2.dp.toPx(), size.height - top - bottom), CornerRadius(1.dp.toPx()))
            }
            .padding(start = 14.dp),
    ) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable { vm.toggleProject(s.name, !open) }
                .semantics(mergeDescendants = true) { role = Role.Button; stateDescription = if (open) "open" else "shut" }
                .padding(top = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.chev, null, Modifier.size(18.dp).rotate(if (open) 90f else 0f), tint = x.text3)
            Spacer(Modifier.width(10.dp))
            Text(s.name, fontSize = 15.sp, fontWeight = FontWeight.Medium, color = if (s.paused) x.text3 else MaterialTheme.colorScheme.onSurface,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Text(s.label, fontSize = 12.5.sp, color = if (s.label == "you" || s.label == "asks") x.warn.copy(alpha = .7f) else x.text3.copy(alpha = .8f))
        }
        if (!open) return@Column
        s.live.forEach {
            TaskLine(vm, it)
            Steps(it, it.working != (it.id in flipped)) { flip(it.id) }
        }
        if (s.done.isEmpty()) return@Column
        if (showDone) s.done.forEach { TaskLine(vm, it) }
        Row(Modifier.fillMaxWidth().heightIn(min = 34.dp).clickable(onClick = fold), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(32.dp)) { Glyph("done", Modifier.size(18.dp), alpha = .7f) }
            Text(if (showDone) "hide done" else "${s.done.size} done", fontSize = 13.sp, color = x.text3)
        }
    }
}

@Composable
private fun TaskLine(vm: Seimei, t: Task) {
    val x = LocalExtra.current
    val done = t.state == "done"
    Row(
        Modifier.fillMaxWidth().heightIn(min = if (done) 32.dp else 42.dp)
            .then(if (t.worker != null) Modifier.clickable { vm.openWorker(t.worker) } else Modifier),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // The user's own item: the circle ticks it off, the check takes it back.
        val g = Modifier.width(32.dp).heightIn(min = if (done) 32.dp else 42.dp)
        Box(
            if (t.you) g.clickable { vm.tick(t.id, !done) }.semantics {
                role = Role.Checkbox
                contentDescription = if (done) "Reopen ${t.title}" else "Tick off ${t.title}"
            } else g,
            contentAlignment = Alignment.CenterStart,
        ) { Glyph(t.state, Modifier.size(if (done) 15.dp else 18.dp)) }
        Text(
            buildAnnotatedString {
                if (t.id.startsWith("#")) withStyle(SpanStyle(color = x.text3, textDecoration = TextDecoration.None)) { append(t.id + "  ") }
                append(t.title)
            },
            Modifier.weight(1f),
            fontSize = if (done) 14.sp else 15.5.sp,
            fontWeight = if (t.working) FontWeight.Medium else FontWeight.Normal,
            color = when (t.state) {
                "done" -> x.text3.copy(alpha = .7f)
                "paused" -> x.text3
                else -> MaterialTheme.colorScheme.onSurface
            },
            textDecoration = if (done) TextDecoration.LineThrough else null,
            maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
        if (t.next) Text("next", Modifier.padding(start = 10.dp).background(MaterialTheme.colorScheme.primary.copy(alpha = .14f), RoundedCornerShape(6.dp))
            .padding(horizontal = 6.dp), fontSize = 11.5.sp, lineHeight = 18.sp, color = MaterialTheme.colorScheme.primary)
        Text(t.owner, Modifier.padding(start = if (t.next) 8.dp else 14.dp), fontSize = 13.sp,
            color = (if (t.you) x.warn else x.text3).copy(alpha = if (done) .45f else if (t.you) .7f else .75f))
    }
}

/** A task's steps: a line with the count that folds, then one small line per step while open. */
@Composable
private fun Steps(t: Task, open: Boolean, flip: () -> Unit) {
    if (t.steps.isEmpty()) return
    val x = LocalExtra.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 28.dp).clickable(onClick = flip)
            .semantics(mergeDescendants = true) { role = Role.Button; stateDescription = if (open) "open" else "shut" }
            .padding(start = 32.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.chev, null, Modifier.size(14.dp).rotate(if (open) 90f else 0f), tint = x.text3)
        Spacer(Modifier.width(6.dp))
        Text("${t.steps.count { it.state == "completed" }} of ${t.steps.size} steps", fontSize = 12.5.sp, color = x.text3)
    }
    if (!open) return
    t.steps.forEach { st ->
        val done = st.state == "completed"
        Row(Modifier.fillMaxWidth().heightIn(min = 26.dp).padding(start = 32.dp), verticalAlignment = Alignment.CenterVertically) {
            Glyph(STEP[st.state] ?: "open", Modifier.size(14.dp))
            Spacer(Modifier.width(10.dp))
            Text(st.title, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                color = when (st.state) { "completed" -> x.text3; "in_progress" -> MaterialTheme.colorScheme.onSurface; else -> x.text2 },
                textDecoration = if (done) TextDecoration.LineThrough else null)
        }
    }
}

private val STEP = mapOf("completed" to "done", "in_progress" to "running", "pending" to "open")

/** A task's state: an open circle, half full while running, a dot while it asks, a check inside once it waits for
 * review, paused bars, a plain check once done. */
@Composable
private fun Glyph(state: String, modifier: Modifier, alpha: Float = 1f) {
    val cs = MaterialTheme.colorScheme
    val c = when (state) {
        "running" -> cs.primary
        "asks" -> LocalExtra.current.warn
        "review" -> LocalExtra.current.ok
        "done" -> LocalExtra.current.ok.copy(alpha = .55f * alpha)
        else -> cs.outline
    }
    Canvas(modifier) {
        val u = size.width / 18f
        val line = Stroke(1.6f * u, cap = StrokeCap.Round)
        if (state == "done") {
            val p = Path().apply { moveTo(4f * u, 9.5f * u); lineTo(7.2f * u, 12.7f * u); lineTo(14f * u, 5.8f * u) }
            drawPath(p, c, style = Stroke(1.8f * u, cap = StrokeCap.Round, join = StrokeJoin.Round))
            return@Canvas
        }
        drawCircle(c, 6.5f * u, center, style = line)
        if (state == "running") drawArc(c, -90f, 180f, true, Offset(4.5f * u, 4.5f * u), Size(9f * u, 9f * u))
        if (state == "asks") drawCircle(c, 2.3f * u, center)
        if (state == "review") {
            val p = Path().apply { moveTo(6.3f * u, 9.2f * u); lineTo(8.2f * u, 11.1f * u); lineTo(11.7f * u, 7.4f * u) }
            drawPath(p, c, style = Stroke(1.6f * u, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
        if (state == "paused") {
            drawLine(c, Offset(7.3f * u, 6.5f * u), Offset(7.3f * u, 11.5f * u), 1.6f * u, StrokeCap.Round)
            drawLine(c, Offset(10.7f * u, 6.5f * u), Offset(10.7f * u, 11.5f * u), 1.6f * u, StrokeCap.Round)
        }
    }
}

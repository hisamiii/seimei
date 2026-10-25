package com.hisamiowo.seimei

import androidx.compose.runtime.snapshotFlow
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

// The Tasks lane, as on the web page (tasks.js): two views behind one connected button group. Now: what needs the
// user (a question, a job to look over, their own tasks), what runs and how far, and each shikigami's next. All
// tasks: one quiet row per project with a one-line summary; a project opens in place, its done items fold into
// "N done", paused projects wait behind one row. The user ticks their own tasks (with Undo); a task with a
// shikigami opens its page.

/** One step of a shikigami's plan for its task: pending, in_progress or completed. */
data class Step(val title: String, val state: String)

data class Task(val id: String, val title: String, val project: String, val owner: String, val state: String, val worker: String?,
                val pos: Int = 0, val steps: List<Step> = emptyList(), val doneAt: Double = 0.0) {
    val you get() = owner == "you"
    val mine get() = you && state == "open"
    val stepsDone get() = steps.count { it.state == "completed" }
    /** The step it's on, if its plan says. */
    val current get() = steps.firstOrNull { it.state == "in_progress" }?.title
}

/** The Now view: what needs the user, what runs, and each shikigami's next. */
data class Now(val asks: List<Task>, val review: List<Task>, val mine: List<Task>, val running: List<Task>, val next: List<Task>) {
    val needs get() = asks.size + review.size + mine.size
}

/** A project on All tasks: [live] in the order it shows, [done] newest first. */
data class Project(val name: String, val paused: Boolean, val live: List<Task>, val done: List<Task>)

data class Board(val tasks: List<Task>, val projects: List<Project>) {
    /** Live tasks of projects that aren't paused. */
    val active get() = projects.filter { !it.paused }.flatMap { p -> p.live.filter { it.state != "paused" } }
    val nexts get() = nextOf(active).map { it.id }.toSet()
}

private val ORDER = mapOf("asks" to 0, "review" to 1, "running" to 2)

/** Each shikigami's next: its first open task in queue order. */
fun nextOf(ts: List<Task>) = ts.sortedBy { it.pos }.filter { it.state == "open" && !it.you }.distinctBy { it.owner }

fun board(b: JSONObject): Board {
    val tasks = b.optJSONArray("tasks").objects().map { t ->
        Task(t.str("id"), t.str("title"), t.str("project"), t.str("owner"), t.str("state"), t.str("worker").ifEmpty { null }, t.optInt("pos"),
            t.optJSONArray("steps").objects().map { Step(it.str("title"), it.str("state")) },
            t.optDouble("done_at", 0.0).takeIf { !it.isNaN() } ?: 0.0)
    }
    fun rank(ts: List<Task>) = when {
        ts.isEmpty() -> 4
        ts.any { it.state == "asks" } -> 0
        ts.any { it.state == "review" } -> 1
        ts.any { it.state == "running" } -> 2
        else -> 3
    }
    val projects = b.optJSONArray("projects").objects().mapNotNull { p ->
        val name = p.str("name")
        val ts = tasks.filter { it.project == name }
        if (ts.isEmpty()) return@mapNotNull null
        Project(name, p.optBoolean("paused"),
            ts.filter { it.state != "done" }.sortedWith(compareBy({ ORDER[it.state] ?: 3 }, { it.pos })),
            ts.filter { it.state == "done" }.sortedByDescending { it.doneAt })
    }.withIndex().sortedWith(compareBy({ rank(it.value.live) }, { it.index })).map { it.value }
    return Board(tasks, projects)
}

fun now(b: Board): Now {
    val ts = b.active.sortedBy { it.pos }
    return Now(ts.filter { it.state == "asks" }, ts.filter { it.state == "review" }, ts.filter { it.mine },
        ts.filter { it.state == "running" }, nextOf(ts))
}

/** A project's summary, in words a person would say: (text, tone) with tone "warn" for what needs the user, "run"
 * for what runs. */
fun summary(p: Project): List<Pair<String, String>> {
    val ts = p.live
    fun n(f: (Task) -> Boolean) = ts.count(f)
    return buildList {
        n { it.state == "asks" }.takeIf { it > 0 }?.let { add((if (it == 1) "1 question" else "$it questions") to "warn") }
        n { it.state == "review" }.takeIf { it > 0 }?.let { add("$it review" to "warn") }
        n { it.mine }.takeIf { it > 0 }?.let { add("$it for you" to "warn") }
        n { it.state == "running" }.takeIf { it > 0 }?.let { add("$it running" to "run") }
        n { it.state == "open" && !it.you }.takeIf { it > 0 }?.let { add("$it open" to "") }
    }
}

@Composable
fun TasksTab(vm: Seimei) {
    val raw = vm.board
    val b = remember(raw) { raw?.let(::board) }
    var view by rememberSaveable { mutableStateOf("now") }
    // Done items shown in full, per project, and the paused projects: for this visit only, like the web page
    var doneShown by rememberSaveable { mutableStateOf(listOf<String>()) }
    var pausedShown by rememberSaveable { mutableStateOf(false) }
    var moreMine by rememberSaveable { mutableStateOf(false) }
    Column(Modifier.fillMaxSize()) {
        if (b == null || b.tasks.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(if (b == null) "Loading the board…" else "No tasks",
                    Modifier.padding(horizontal = 48.dp), fontSize = 14.sp, color = LocalExtra.current.text3, textAlign = TextAlign.Center)
            }
            return@Column
        }
        val list = rememberLazyListState()
        LaunchedEffect(list) { snapshotFlow { list.canScrollBackward }.collect { vm.lifted = it } }
        LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 24.dp)) {
            item(key = "view") { ViewSwitch(view) { view = it } }
            if (view == "now") nowItems(vm, now(b), moreMine) { moreMine = true }
            else allItems(vm, b, doneShown, pausedShown,
                fold = { doneShown = if (it in doneShown) doneShown - it else doneShown + it },
                paused = { pausedShown = !pausedShown })
        }
    }
}

/** Now / All tasks: a connected button group, 40 tall in a 48 target, 2dp apart, the selected one a full pill. */
@Composable
private fun ViewSwitch(view: String, pick: (String) -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        listOf("now" to "Now", "all" to "All tasks").forEachIndexed { i, (k, label) ->
            val on = k == view
            val shape = when {
                on -> CircleShape
                i == 0 -> RoundedCornerShape(20.dp, 8.dp, 8.dp, 20.dp)
                else -> RoundedCornerShape(8.dp, 20.dp, 20.dp, 8.dp)
            }
            Box(
                Modifier.weight(1f).height(48.dp).clickable { pick(k) }.semantics { role = Role.Tab; selected = on },
                contentAlignment = Alignment.Center,
            ) {
                Box(Modifier.fillMaxWidth().height(40.dp).clip(shape).background(if (on) cs.primary else cs.primaryContainer), contentAlignment = Alignment.Center) {
                    Text(label, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = if (on) cs.onPrimary else cs.onPrimaryContainer)
                }
            }
        }
    }
}

@Composable
private fun TaskGroupTitle(title: String, n: Int?, color: Color) {
    Text(buildAnnotatedString {
        append(title)
        if (n != null) withStyle(SpanStyle(fontWeight = FontWeight.Normal, color = color.copy(alpha = .75f))) { append("  $n") }
    }, color = color, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(start = 16.dp, top = 20.dp, bottom = 8.dp).semantics { heading() })
}

private fun LazyListScope.group(key: String, rows: List<Pair<String, @Composable (Shape) -> Unit>>, outer: Dp = 20.dp, inner: Dp = 4.dp) {
    itemsIndexed(rows, key = { _, r -> "$key:${r.first}" }) { i, (_, row) ->
        if (i > 0) Spacer(Modifier.height(2.dp))
        row(segment(i, rows.size, outer, inner))
    }
}

private fun LazyListScope.nowItems(vm: Seimei, n: Now, moreMine: Boolean, more: () -> Unit) {
    if (n.needs > 0) {
        item(key = "needs") { TaskGroupTitle("Needs you", n.needs, LocalTokens.current.warn) }
        val mine = if (moreMine) n.mine else n.mine.take(1)
        val rows = buildList<Pair<String, @Composable (Shape) -> Unit>> {
            n.asks.forEach { t -> add(t.id to { s -> TaskRow(vm, t, s, "ask", listOf(t.owner, "question"), warm = true) }) }
            n.review.forEach { t -> add(t.id to { s -> TaskRow(vm, t, s, "review", listOf(t.owner, "review"), warm = true) }) }
            mine.forEach { t -> add(t.id to { s -> TaskRow(vm, t, s, "you", listOf("your task"), warm = true) }) }
            if (n.mine.size > mine.size) add("more" to { s -> More("${n.mine.size - mine.size} more of yours", s, more) })
        }
        // Needs you, the hero: its own corner set against the routine groups
        group("n", rows, 28.dp, 8.dp)
    } else item(key = "clear") { AllClear(vm) }
    if (n.running.isNotEmpty()) {
        item(key = "running") { TaskGroupTitle("Running", n.running.size, MaterialTheme.colorScheme.primary) }
        group("r", n.running.mapIndexed { i, t -> t.id to { s: Shape -> TaskRow(vm, t, s, "run", listOfNotNull(t.owner, t.current), bar = true, emph = i == 0) } })
    }
    if (n.next.isNotEmpty()) {
        item(key = "next") { TaskGroupTitle("Up next", null, LocalExtra.current.text2) }
        group("x", n.next.map { t -> t.id to { s: Shape -> TaskRow(vm, t, s, "open", listOf(t.owner)) } })
    }
}

private fun LazyListScope.allItems(vm: Seimei, b: Board, doneShown: List<String>, pausedShown: Boolean, fold: (String) -> Unit, paused: () -> Unit) {
    val nexts = b.nexts
    fun rows(ps: List<Project>) = ps.flatMap { p ->
        val open = vm.boardOpen.optBoolean(p.name)
        buildList<Pair<String, @Composable (Shape) -> Unit>> {
            add("p:${p.name}" to { s -> ProjectRow(p, open, s) { vm.toggleProject(p.name, !open) } })
            if (!open) return@buildList
            fun own(t: Task) = if (t.owner == t.project || t.you) emptyList() else listOf(t.owner)
            p.live.forEach { t ->
                add(t.id to { s ->
                    when {
                        t.state == "asks" -> TaskRow(vm, t, s, "ask", own(t) + "question", inset = true, warm = true, ref = true)
                        t.state == "review" -> TaskRow(vm, t, s, "review", own(t) + "review", inset = true, ref = true)
                        t.state == "running" -> TaskRow(vm, t, s, "run", own(t) + (t.current ?: "running"), inset = true, bar = true, ref = true)
                        t.you -> TaskRow(vm, t, s, "you", listOf(if (t.state == "paused") "paused" else "your task"), inset = true)
                        else -> TaskRow(vm, t, s, "open", own(t) + (if (t.id in nexts) "next" else if (t.state == "paused") "paused" else "open"),
                            inset = true, ref = true)
                    }
                })
            }
            if (p.done.isEmpty()) return@buildList
            val shown = p.name in doneShown
            if (shown) p.done.forEach { t -> add(t.id to { s -> TaskRow(vm, t, s, "done", own(t) + "done", inset = true, ref = true) }) }
            add("f:${p.name}" to { s -> Fold(if (shown) "Hide done" else "${p.done.size} done", s) { fold(p.name) } })
        }
    }
    group("b", rows(b.projects.filter { !it.paused }))
    val held = b.projects.filter { it.paused }
    if (held.isEmpty()) return
    item(key = "paused") {
        val n = held.size
        Column(Modifier.padding(top = 12.dp)) {
            ProjectRow(Project(if (n == 1) "1 paused project" else "$n paused projects", true, emptyList(), emptyList()), pausedShown,
                if (pausedShown) segment(0, 2, 20.dp, 4.dp) else RoundedCornerShape(20.dp), sub = held.joinToString(", ") { it.name }, onClick = paused)
        }
    }
    if (pausedShown) {
        item(key = "paused-gap") { Spacer(Modifier.height(2.dp)) }
        val rs = rows(held)
        itemsIndexed(rs, key = { _, r -> "h:${r.first}" }) { i, (_, row) ->
            if (i > 0) Spacer(Modifier.height(2.dp))
            row(if (i == rs.size - 1) RoundedCornerShape(4.dp, 4.dp, 20.dp, 20.dp) else RoundedCornerShape(4.dp))
        }
    }
}

/** A row: optional tile, title, supporting line, optional bar. Rows inside an open project are set in by their text. */
@Composable
private fun TaskRow(vm: Seimei, t: Task, shape: Shape, kind: String, parts: List<String>, warm: Boolean = false, inset: Boolean = false,
                    bar: Boolean = false, ref: Boolean = false, emph: Boolean = false) {
    val tk = LocalTokens.current
    val x = LocalExtra.current
    val cs = MaterialTheme.colorScheme
    val bg = when {
        warm -> tk.warnContainer
        inset -> cs.surfaceContainerHigh
        else -> cs.surfaceContainer
    }
    val sub = if (warm) tk.onWarnContainer.copy(alpha = .72f) else x.text3
    Row(
        Modifier.fillMaxWidth().heightIn(min = if (inset) 56.dp else 64.dp).clip(shape).background(bg)
            .then(if (t.worker != null) Modifier.clickable { vm.openWorker(t.worker) } else Modifier)
            .padding(start = if (inset) 32.dp else 16.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val tile = !inset && (kind == "ask" || kind == "review")
        if (tile) {
            val ask = kind == "ask"
            ShapeTile(t.owner, 40.dp, if (ask) tk.warn else cs.primary) {
                Icon(if (ask) Icons.question else Icons.review, null, Modifier.size(20.dp), tint = if (ask) tk.onWarn else cs.onPrimary)
            }
            Spacer(Modifier.width(16.dp))
        } else if (t.you) {
            Tick(vm, t, if (inset) Modifier.padding(end = 8.dp) else Modifier.padding(end = 16.dp))
        }
        Column(Modifier.weight(1f)) {
            // The screen's one emphasized title: the running task
            Text(t.title, fontSize = 16.sp, lineHeight = 24.sp, fontWeight = if (emph) FontWeight.Bold else FontWeight.Medium,
                color = when { warm -> tk.onWarnContainer; kind == "done" -> x.text3; else -> cs.onSurface },
                maxLines = if (warm || kind == "ask" || kind == "review") 2 else 1, overflow = TextOverflow.Ellipsis)
            Text(buildAnnotatedString {
                val all = parts + (if (ref && t.id.startsWith("#")) listOf(t.id) else emptyList())
                all.forEachIndexed { i, p ->
                    if (i > 0) append(" · ")
                    if (p == "next" && i == all.indexOf("next")) withStyle(SpanStyle(color = cs.primary)) { append(p) } else append(p)
                }
            }, fontSize = 14.sp, lineHeight = 20.sp, color = sub, maxLines = if (warm) 2 else 1, overflow = TextOverflow.Ellipsis)
            if (bar) StepBar(t)
        }
    }
}

/** The user's own task: an empty circle that ticks it off, a check that takes it back. */
@Composable
private fun Tick(vm: Seimei, t: Task, modifier: Modifier) {
    val done = t.state == "done"
    val tk = LocalTokens.current
    Box(
        modifier.size(48.dp).clip(CircleShape).clickable { vm.tick(t.id, !done) }.semantics {
            role = Role.Checkbox
            toggleableState = if (done) ToggleableState.On else ToggleableState.Off
            contentDescription = if (done) "Reopen ${t.title}" else "Tick off ${t.title}"
        },
        contentAlignment = Alignment.Center,
    ) {
        if (done) Icon(Icons.check, null, Modifier.size(20.dp), tint = tk.ok)
        else Box(Modifier.size(24.dp).drawBehind {
            drawCircle(tk.warn, size.minDimension / 2 - 1.dp.toPx(), style = androidx.compose.ui.graphics.drawscope.Stroke(2.dp.toPx()))
        })
    }
}

/** Step progress: the active part, a 4dp gap, the track, and the count in words. */
@Composable
private fun StepBar(t: Task) {
    val cs = MaterialTheme.colorScheme
    val track = LocalTokens.current.containerHighest
    val f = if (t.steps.isEmpty()) 0f else t.stepsDone.toFloat() / t.steps.size
    Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.weight(1f).height(4.dp).drawBehind {
            val h = size.height
            val r = CornerRadius(h / 2)
            val w = size.width * f
            val gap = if (f > 0f) 4.dp.toPx() else 0f
            if (w > 0f) drawRoundRect(cs.primary, Offset.Zero, Size(w, h), r)
            if (size.width - w - gap > 0f) drawRoundRect(track, Offset(w + gap, 0f), Size(size.width - w - gap, h), r)
        })
        if (t.steps.isNotEmpty()) {
            Spacer(Modifier.width(12.dp))
            Text("${t.stepsDone} of ${t.steps.size}", fontSize = 12.sp, lineHeight = 16.sp, color = LocalExtra.current.text3)
        }
    }
}

@Composable
private fun ProjectRow(p: Project, open: Boolean, shape: Shape, sub: String? = null, onClick: () -> Unit) {
    val tk = LocalTokens.current
    val x = LocalExtra.current
    val cs = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clip(shape).background(if (open && sub == null) cs.surfaceContainerHigh else cs.surfaceContainer)
            .clickable(onClick = onClick)
            .semantics(mergeDescendants = true) { role = Role.Button; stateDescription = if (open) "Open" else "Shut" }
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(p.name, fontSize = 16.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold,
                color = if (sub != null) x.text2 else cs.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(buildAnnotatedString {
                if (sub != null) append(sub)
                else if (p.live.isEmpty()) append(if (p.done.size == 1) "1 done" else "${p.done.size} done")
                else summary(p).forEachIndexed { i, (s, tone) ->
                    if (i > 0) append(" · ")
                    when (tone) {
                        "warn" -> withStyle(SpanStyle(color = tk.warn, fontWeight = FontWeight.Medium)) { append(s) }
                        "run" -> withStyle(SpanStyle(color = cs.primary)) { append(s) }
                        else -> append(s)
                    }
                }
            }, fontSize = 14.sp, lineHeight = 20.sp, color = x.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Icon(if (open) Icons.expand else Icons.chev, null, Modifier.size(20.dp), tint = x.text3)
    }
}

@Composable
private fun More(text: String, shape: Shape, onClick: () -> Unit) {
    val tk = LocalTokens.current
    Box(Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(shape).background(tk.warnContainer).clickable(onClick = onClick).padding(horizontal = 16.dp),
        contentAlignment = Alignment.CenterStart) {
        Text(text, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = tk.onWarnContainer)
    }
}

@Composable
private fun Fold(text: String, shape: Shape, onClick: () -> Unit) {
    val x = LocalExtra.current
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(shape).background(MaterialTheme.colorScheme.surfaceContainerHigh)
        .clickable(onClick = onClick).padding(start = 32.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.check, null, Modifier.size(18.dp), tint = LocalTokens.current.ok)
        Spacer(Modifier.width(16.dp))
        Text(text, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = x.text3)
    }
}

/** In Needs you's place when nothing waits: one row at a needs row's height, on the done tone. */
@Composable
private fun AllClear(vm: Seimei) {
    val tk = LocalTokens.current
    val reduced = reducedMotion()
    val since = remember(vm.board) { vm.board?.let(::board)?.tasks?.maxOfOrNull { it.doneAt }?.takeIf { it > 0 } ?: (System.currentTimeMillis() / 1000.0) }
    Row(Modifier.fillMaxWidth().padding(top = 16.dp).heightIn(min = 68.dp).clip(RoundedCornerShape(28.dp)).background(tk.okContainer)
        .padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        // The clover settles into the very sunny shape once, on a short spring
        val settle = remember { Animatable(if (reduced) 1f else 0f) }
        LaunchedEffect(Unit) { settle.animateTo(1f, tween(410, easing = SpringEasing)) }
        Box(Modifier.size(40.dp).clip(PolyShape(SHAPES.getValue("clover4"), SHAPES.getValue("very-sunny"), settle.value)).background(tk.ok),
            contentAlignment = Alignment.Center) {
            Icon(Icons.check, null, Modifier.size(24.dp), tint = tk.okContainer)
        }
        Spacer(Modifier.width(16.dp))
        Column {
            Text("All clear", fontSize = 16.sp, lineHeight = 24.sp, fontWeight = FontWeight.Bold, color = tk.onOkContainer)
            Text("Since ${clock(since)}", fontSize = 14.sp, lineHeight = 20.sp, color = tk.onOkContainer)
        }
    }
}

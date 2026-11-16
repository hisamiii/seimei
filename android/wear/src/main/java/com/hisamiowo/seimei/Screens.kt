package com.hisamiowo.seimei

import com.hisamiowo.seimei.wear.BuildConfig
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.wear.compose.material3.Icon
import androidx.wear.compose.material3.ScrollIndicator
import com.hisamiowo.seimei.shared.R
import kotlinx.coroutines.delay
import org.json.JSONObject
import java.time.LocalDate
import java.time.format.DateTimeFormatter

/** A list screen: rows from the top, room for the clock above and the round edge below. */
@Composable
fun Screen(state: LazyListState = rememberLazyListState(), top: Dp = 34.dp, content: LazyListScope.() -> Unit) {
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            Modifier.fillMaxSize().fadeEdges(top, 56.dp).crown(state), state = state,
            contentPadding = PaddingValues(start = 0.dp, end = 0.dp, top = top, bottom = 60.dp),
            horizontalAlignment = Alignment.CenterHorizontally, content = content,
        )
        ScrollIndicator(state, Modifier.align(Alignment.CenterEnd))
    }
}

/** Relative times move on by themselves. */
@Composable
fun ticking(): Long = produceState(System.currentTimeMillis()) {
    while (true) {
        delay(30_000)
        value = System.currentTimeMillis()
    }
}.value

private fun ago(a: Agent) = if (a.state == "done") whenText(a.since) else short(a.since)

// ---------- the drawer ----------

/** The drawer: the phone's sections in its order, as one list down the round screen (mockup v6). */
@Composable
fun Drawer(vm: Model) {
    val t = LocalT.current
    ticking()
    @Composable
    fun Head(label: String, to: Screen) = CircleRow(inset = 6.dp) {
        Row(Modifier.height(16.dp).clip(RoundedCornerShape(8.dp)).clickable { vm.go(to) }.padding(start = 8.dp, end = 6.dp),
            verticalAlignment = Alignment.CenterVertically) {
            T(label, Modifier.weight(1f), LABEL, t.text2, FontWeight.SemiBold, 1)
            Ico(Icons.chev, 14.dp, t.text3)
        }
    }

    @Composable
    fun Rule() = Box(Modifier.height(6.6.dp).fillMaxWidth(), contentAlignment = Alignment.Center) {
        Box(Modifier.padding(horizontal = 46.dp).fillMaxWidth().height(1.dp).background(t.containerHigh))
    }

    @Composable
    fun Time(s: String) = T(s, size = SMALL0, color = t.text3, lines = 1)

    Screen(top = 34.dp) {
        item("seimei") {
            ListRow("seimei", bg = t.containerHigh, lead = { Glyph(18.dp) }) { vm.go(Screen.Home) }
        }
        item("r1") { Rule() }
        item("h-threads") { Head("Side threads", Screen.Threads) }
        items(vm.threads.take(3), key = { "t" + it.str("id") }) { th ->
            ListRow(th.str("name"), lead = { Ico(Icons.thread, 16.dp) }, trail = if (th.optInt("unread") > 0) ({ Dot() }) else null) {
                vm.go(Screen.Agent("thread", th.str("id")))
            }
        }
        if (vm.threadsArchived > 0) item("arch") { ListRow("Archived", lead = { Ico(Icons.shelf, 16.dp) }) { vm.go(Screen.Threads) } }
        item("r2") { Rule() }
        item("h-agents") { Head("Agents", Screen.Agents) }
        items(vm.recentAgents(4), key = { "a" + it.kind + it.id }) { a ->
            ListRow(a.name, lead = { Tile(a, 16.dp) }, trail = { Time(ago(a)) }) { vm.go(Screen.Agent(a.kind, a.id)) }
        }
        item("r3") { Rule() }
        item("h-watches") { Head("Watches", Screen.Watches) }
        items(vm.watches.take(3), key = { "w" + it.str("id") }) { w ->
            ListRow(w.str("title"), lead = { Ico(Icons.watch[w.str("group")] ?: Icons.watch.getValue("ping"), 16.dp) },
                trail = if (w.str("group") == "time") ({ Time(due(w.optDouble("at"))) }) else null) { vm.go(Screen.Watches) }
        }
        item("r4") { Rule() }
        item("files") { ListRow("Files", lead = { Ico(Icons.files, 16.dp) }) { vm.go(Screen.Files) } }
        item("r5") { Rule() }
        item("usage") {
            val w = vm.usage?.optJSONObject("windows")?.optJSONObject("five_hour")
            ListRow("Usage", lead = { UsageRing(w?.optDouble("utilization", 0.0) ?: 0.0, 16.dp) },
                trail = w?.let { { Time("${Math.round(it.optDouble("utilization") * 100)}%") } }) { vm.go(Screen.Usage) }
        }
        item("settings") { ListRow("Settings", lead = { Ico(Icons.tune, 16.dp) }) { vm.go(Screen.Settings) } }
    }
}

/** seimei's mark: the star in a primary-container circle. */
@Composable
fun Glyph(size: Dp) {
    val t = LocalT.current
    Box(Modifier.size(size).clip(CircleShape).background(t.primaryContainer), contentAlignment = Alignment.Center) {
        Icon(painterResource(R.drawable.ic_seimei_monochrome), null, Modifier.requiredSize(size * 1.2f), tint = t.onPrimaryContainer)
    }
}

/** A timer's time: its clock today, the weekday and clock after. */
fun due(t: Double): String {
    if (t <= 0 || t.isNaN()) return ""
    if (dayOf(t) == LocalDate.now()) return clock(t)
    return dayOf(t).dayOfWeek.getDisplayName(java.time.format.TextStyle.SHORT, java.util.Locale.getDefault()) + " " + clock(t)
}

@Composable
fun UsageRing(util: Double, size: Dp) {
    val t = LocalT.current
    Canvas(Modifier.size(size)) {
        val w = this.size.minDimension * 3f / 24f
        val inset = this.size.minDimension * 3.5f / 24f
        val s = Size(this.size.width - 2 * inset, this.size.height - 2 * inset)
        drawArc(t.containerHighest, 0f, 360f, false, Offset(inset, inset), s, style = Stroke(w))
        drawArc(if (util >= .9) t.error else t.text2, -90f, 360f * util.toFloat().coerceIn(0f, 1f), false, Offset(inset, inset), s,
            style = Stroke(w, cap = StrokeCap.Round))
    }
}

// ---------- agents ----------

/** Agents by state, as the phone's tab groups them (mockup v5): needs you, working, idle. */
@Composable
fun Agents(vm: Model) {
    val t = LocalT.current
    ticking()
    val all = vm.agents()
    fun by(st: String) = all.filter { it.state == st }.sortedByDescending { it.since }
    val groups = listOf(Triple("Needs you", t.warn, by("ask")), Triple("Working", t.primary, by("work")), Triple("Idle", t.text3, by("idle")))
    Screen(top = 35.dp) {
        if (all.isEmpty()) item("none") { T("No agents", Modifier.padding(top = 60.dp), BODY, t.text3) }
        for ((title, color, list) in groups) {
            if (list.isEmpty()) continue
            item("h$title") { Label(title, color) }
            itemsIndexed(list, key = { _, a -> a.kind + a.id }) { i, a -> AgentRow(a, segment(i, list.size)) { vm.open(a) } }
            item("g$title") { Gap(3.dp) }
        }
    }
}

@Composable
fun AgentRow(a: Agent, shape: androidx.compose.ui.graphics.Shape, onClick: () -> Unit) {
    val t = LocalT.current
    val ask = a.state == "ask"
    CircleRow(Modifier.padding(bottom = 1.dp), inset = if (ask) 0.dp else 3.dp) {
        Row(
            Modifier.height(if (ask) 33.dp else 26.dp).clip(shape).background(if (ask) t.warnContainer else t.container).clickable(onClick = onClick)
                .padding(start = 8.dp, end = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Tile(a, 20.dp)
            if (ask) Column(Modifier.weight(1f)) {
                T(a.name, size = BODY, color = t.onWarnContainer, weight = FontWeight.Bold, lines = 1)
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    T(a.line, Modifier.weight(1f), SMALL, t.onWarnContainer, lines = 1)
                    T(ago(a), size = SMALL0, color = t.onWarnContainer.copy(alpha = .8f), lines = 1)
                }
            } else {
                T(a.name, Modifier.weight(1f), BODY, t.text, FontWeight.Medium, 1)
                T(ago(a), size = SMALL0, color = t.text3, lines = 1)
            }
        }
    }
}

// ---------- one agent ----------

@Composable
fun AgentPage(vm: Model, pg: Page) {
    val t = LocalT.current
    val now = ticking()
    val a = when (pg.kind) {
        "worker" -> fromWorker(pg.row)
        "thread" -> fromThread(pg.row)
        else -> fromSession(pg.row, vm.seen)
    }
    val list = rememberLazyListState()
    val lift = rememberLift(true)
    val runs = when (pg.kind) {
        "thread" -> pg.row.str("state") in setOf("running", "starting")
        "worker" -> pg.row.str("state") in ACTIVE && pg.row.str("state") != "asking"
        else -> false
    }
    // Like the chat: newest at the bottom, and a reader near the end follows it; a sent message or a new question brings it down.
    val follow = rememberFollow(list, Triple(pg.items.lastOrNull()?.key, pg.ask?.str("tool_id"), pg.row.str("question")), pg.sent.size,
        force = pg.ask != null)
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().padding(bottom = PILL_BOTTOM), horizontalAlignment = Alignment.CenterHorizontally) {
          Box(Modifier.weight(1f)) {
            Log(list, lift, room = 30.dp) {
                pg.note?.let { n -> item("note") { T(n, size = SMALL, color = t.warn) } }
                val q = pg.row.str("question")
                if (pg.kind == "worker" && pg.row.str("state") == "asking" && q.isNotEmpty()) item("wask") {
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.warnContainer).padding(8.dp)) { Md(q, t.onWarnContainer) }
                }
                pg.ask?.let { ask -> item("ask") { Question(vm, ask) } }
                items(pg.sent.reversed(), key = { "p" + it.cid }) { Bubble(it.text, dim = true) }
                items(pg.items.asReversed(), key = { it.key }, contentType = { it::class }) { ItemRow(it, vm, "p") }
            }
            JumpDown(list, follow)
          }
            Gap(6.dp)
            Composer(pg.draft, { pg.draft = it }, "Message ${a.name.ifEmpty { "agent" }}", pg.chatKey, vm, onSend = vm::pageSend,
                stop = if (runs) ({ vm.workerStop(pg) }) else null)
        }
        PageHeader(lift, a, now)
        ScrollIndicator(list, Modifier.align(Alignment.CenterEnd))
    }
}

/** The page's head, one line: the name, then its state and age in the state's colour. */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.PageHeader(lift: Lift, a: Agent, now: Long) {
    val t = LocalT.current
    val f = lift.fraction()
    if (f <= 0f) return
    val word = when (a.state) {
        "ask" -> "Needs you"
        "work" -> "Working"
        "done" -> "Finished"
        else -> "Idle"
    }
    val ago = short(a.since).also { now }
    Box(Modifier.align(Alignment.TopCenter).fillMaxWidth().height(LOG_TOP + 30.dp)
        .background(androidx.compose.ui.graphics.Brush.verticalGradient(0f to t.surface, .8f to t.surface, 1f to t.surface.copy(alpha = 0f))))
    Row(Modifier.align(Alignment.TopCenter).padding(top = LOG_TOP, start = 30.dp, end = 30.dp).offset(y = (-12).dp * (1 - f)),
        verticalAlignment = Alignment.CenterVertically) {
        T(a.name, Modifier.weight(1f, fill = false), BODY_H, t.text, FontWeight.Medium, 1)
        T("  " + if (a.state == "done" || ago == "now") word else "$word · $ago", size = LABEL, lines = 1,
            color = when (a.state) { "ask" -> t.warn; "work" -> t.primary; else -> t.text3 })
    }
}

/** A session's open question: its options as buttons, the first tap answers. */
@Composable
private fun Question(vm: Model, ask: JSONObject) {
    val t = LocalT.current
    val q = ask.optJSONArray("questions")?.optJSONObject(0)
    var picked by rememberSaveable(ask.str("tool_id")) { mutableStateOf(0) }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp, 12.dp, 3.dp, 3.dp)).background(t.warnContainer).padding(8.dp)) {
            Md(q?.str("question") ?: ask.str("text").ifEmpty { "A question" }, t.onWarnContainer)
        }
        val opts = q?.optJSONArray("options").objects()
        for ((i, o) in opts.withIndex()) {
            val n = i + 1
            Box(Modifier.fillMaxWidth().clip(segment(i + 1, opts.size + 1)).background(if (picked == n) t.primaryContainer else t.container)
                .clickable(enabled = picked == 0 && ask.optBoolean("answerable", true)) {
                    picked = n
                    vm.pageAnswer(n) { ok -> if (!ok) picked = 0 }
                }.padding(8.dp)) {
                T(o.str("label"), size = BODY, color = t.text, lines = 3)
            }
        }
    }
}

// ---------- usage ----------

/** Usage (mockup v6): the 5-hour and weekly windows as two rings, then today's sessions, then the API prices. */
@Composable
fun Usage(vm: Model) {
    val t = LocalT.current
    ticking()
    val u = vm.usage
    val today = u?.optJSONObject("today")?.optJSONObject("sessions")
    Screen(top = 34.dp) {
        item("rings") {
            Row(Modifier.padding(top = 5.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                for ((key, label) in WINDOWS) {
                    val w = u?.optJSONObject("windows")?.optJSONObject(key) ?: continue
                    Gauge(w.optDouble("utilization", 0.0), label, w.optDouble("resets_at", 0.0))
                }
            }
        }
        if (today != null && today.length() > 0) {
            item("h-today") { Label("Today in seimei", t.text2, Modifier.padding(top = 4.dp)) }
            val rows = today.keys().asSequence().map { it to today.getJSONObject(it) }.sortedByDescending { it.second.optLong("output") }.toList()
            itemsIndexed(rows, key = { _, r -> "s" + r.first }) { i, (name, s) ->
                val turns = s.optInt("turns")
                UsageRow(name.removePrefix("shikigami "), "${tokens(s.optLong("output"))} out · ${tokens(s.optLong("input") + s.optLong("cache_read") + s.optLong("cache_write"))} in",
                    segment(i, rows.size)) { T("$turns ${if (turns == 1) "turn" else "turns"}", size = SMALL0, color = t.text3, lines = 1) }
            }
        }
        val cost = vm.cost
        if (cost != null) {
            item("h-cost") { Label("At API prices", t.text2, Modifier.padding(top = 6.dp)) }
            val since = cost.optDouble("week_start").let { java.time.Instant.ofEpochMilli((it * 1000).toLong()).atZone(java.time.ZoneId.systemDefault())
                .format(DateTimeFormatter.ofPattern("EEE")) + " " + clock(it) }
            val rows = listOf(Triple("day", "Today", ""), Triple("week", "This week", "Since $since"), Triple("all", "All time", ""))
                .filter { cost.optJSONObject(it.first) != null }
            itemsIndexed(rows, key = { _, r -> "c" + r.first }) { i, (key, label, sub) ->
                val p = cost.getJSONObject(key)
                UsageRow(label, listOf(sub, "%,d API calls".format(java.util.Locale.US, p.optLong("messages"))).filter { it.isNotEmpty() }.joinToString(" · "),
                    segment(i, rows.size)) { T(usd(p.optDouble("usd")), size = BODY1, weight = FontWeight.SemiBold, lines = 1) }
            }
        }
    }
}

@Composable
private fun UsageRow(title: String, sub: String, shape: androidx.compose.ui.graphics.Shape, trail: @Composable () -> Unit) {
    val t = LocalT.current
    CircleRow(Modifier.padding(bottom = 1.4.dp)) {
        Row(Modifier.height(32.dp).clip(shape).background(t.container).padding(horizontal = 7.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(5.dp)) {
            Column(Modifier.weight(1f)) {
                T(title, size = BODY, weight = FontWeight.Medium, lines = 1, lineHeight = 15.sp)
                T(sub, size = LABEL, color = t.text3, lines = 1, lineHeight = 12.sp)
            }
            trail()
        }
    }
}

/** One window as a ring open at the bottom: the percent inside, the window's name and the time to its reset under it. */
@Composable
private fun Gauge(util: Double, label: String, resets: Double) {
    val t = LocalT.current
    val tone = if (util >= .9) t.error else t.text2
    Box(Modifier.size(70.dp, 88.dp)) {
        Canvas(Modifier.size(70.dp).align(Alignment.TopCenter)) {
            val w = 4.7.dp.toPx()
            val r = 27.3.dp.toPx()
            val tl = Offset(center.x - r, center.y - r)
            val s = Size(2 * r, 2 * r)
            val fill = 240f * util.toFloat().coerceIn(0f, 1f)
            // Clockwise from 8 o'clock to 4 o'clock: 240 degrees, the gap at the bottom.
            if (fill < 233f) drawArc(t.containerHighest, 150f + fill + 7f, 240f - fill - 7f, false, tl, s, style = Stroke(w, cap = StrokeCap.Round))
            if (fill > 0f) drawArc(tone, 150f, fill, false, tl, s, style = Stroke(w, cap = StrokeCap.Round))
        }
        T("${Math.round(util * 100)}%", Modifier.align(Alignment.TopCenter).padding(top = 22.dp), 18.8.sp, tone, FontWeight.SemiBold, 1)
        Column(Modifier.align(Alignment.TopCenter).padding(top = 51.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            T(label, size = SMALL, weight = FontWeight.SemiBold, lines = 1)
            if (resets > 0) T(resetText(resets).substringAfter(", "), size = LABEL, color = t.text2, lines = 1, align = TextAlign.Center)
        }
    }
}

// ---------- the drawer's other screens ----------

@Composable
fun Threads(vm: Model) {
    val t = LocalT.current
    val all = vm.allThreads ?: vm.threads
    val (archived, open) = all.partition { it.optBoolean("archived") }
    Screen {
        item("h") { Label("Side threads", t.text2) }
        if (all.isEmpty()) item("none") { T("No side threads", Modifier.padding(top = 30.dp), BODY, t.text3) }
        items(open, key = { "o" + it.str("id") }) { th -> ThreadRow(vm, th) }
        if (archived.isNotEmpty()) item("ha") { Label("Archived", t.text3) }
        items(archived, key = { "a" + it.str("id") }) { th -> ThreadRow(vm, th) }
    }
}

@Composable
private fun ThreadRow(vm: Model, th: JSONObject) =
    ListRow(th.str("name"), lead = { Ico(Icons.thread, 16.dp) }, trail = if (th.optInt("unread") > 0) ({ Dot() }) else null) {
        vm.push(Screen.Agent("thread", th.str("id")))
    }

@Composable
fun Watches(vm: Model) {
    val t = LocalT.current
    ticking()
    val groups = listOf("time" to "Timers", "exits" to "When it exits", "reboot" to "After reboot", "ping" to "Pings")
    Screen {
        if (vm.watches.isEmpty()) item("none") { T("No watches", Modifier.padding(top = 60.dp), BODY, t.text3) }
        for ((g, title) in groups) {
            val rows = vm.watches.filter { it.str("group") == g }
            if (rows.isEmpty()) continue
            item("h$g") { Label(title, t.text3) }
            items(rows, key = { "w" + it.str("id") }) { w ->
                ListRow(w.str("title"), lead = { Ico(Icons.watch[g] ?: Icons.watch.getValue("ping"), 16.dp) },
                    trail = if (g == "time") ({ T(due(w.optDouble("at")), size = SMALL0, color = t.text3, lines = 1) }) else null)
            }
        }
    }
}

@Composable
fun Files(vm: Model) {
    val t = LocalT.current
    val items = vm.gallery
    Screen {
        if (items != null && items.isEmpty()) item("none") { T("No files yet", Modifier.padding(top = 60.dp), BODY, t.text3) }
        for (f in items.orEmpty().take(40)) item("f" + f.str("file")) {
            Box(Modifier.width(LOG_W).padding(vertical = 3.dp)) {
                if (f.str("type") == "image") Picture(f.str("file"), "", vm)
                else Chip(f.str("name"), Icons.doc)
            }
        }
    }
}

@Composable
fun SettingsScreen(vm: Model) {
    val t = LocalT.current
    Screen {
        item("h") { Label("Theme", t.text2) }
        items(THEMES.filter { it != "material-you" }, key = { it }) { name ->
            ListRow(name.replace('-', ' ').cap(), lead = { if (vm.theme == name) Ico(Icons.check, 16.dp, t.primary) }) { vm.pickTheme(name) }
        }
        item("server") {
            T(vm.prefs.server.substringAfter("://").trimEnd('/'), Modifier.padding(top = 10.dp, start = 30.dp, end = 30.dp), SMALL, t.text3, lines = 2,
                align = TextAlign.Center)
        }
        item("version") { T(BuildConfig.VERSION_NAME, size = LABEL, color = t.text3, lines = 1) }
    }
}

// ---------- pairing ----------

/** Pairing: the server's address (when the build has none) and a code from `seimei pair`. */
@Composable
fun PairScreen(vm: Model) {
    val t = LocalT.current
    var server by rememberSaveable { mutableStateOf(vm.prefs.server) }
    var code by rememberSaveable { mutableStateOf("") }
    var editServer by rememberSaveable { mutableStateOf(vm.prefs.server.isEmpty()) }
    Screen(top = 30.dp) {
        item("mark") { Glyph(30.dp) }
        item("g1") { Gap(8.dp) }
        if (editServer) item("server") { Field(server, { server = it }, "Server address", KeyboardType.Uri) }
        else item("server") {
            T(server.substringAfter("://").trimEnd('/'), Modifier.clickable { editServer = true }.padding(4.dp), SMALL, t.text3, lines = 1)
        }
        item("g2") { Gap(4.dp) }
        item("code") { Field(code, { code = it }, "Pairing code", KeyboardType.Password) }
        item("g3") { Gap(8.dp) }
        item("go") {
            Pill(if (vm.pairBusy) "Pairing…" else "Pair", t.primary, t.onPrimary, Modifier.width(110.dp)) { vm.pair(server, code) }
        }
        vm.pairError?.let { e -> item("err") { T(e, Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp), SMALL, t.error, align = TextAlign.Center) } }
    }
}

@Composable
private fun Field(text: String, onChange: (String) -> Unit, hint: String, type: KeyboardType) {
    val t = LocalT.current
    // No suggestions: the code mixes letters and digits, and the keyboard keeps its digit key in reach.
    BasicTextField(
        text, onChange, Modifier.width(150.dp), singleLine = true,
        keyboardOptions = KeyboardOptions(KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = type, imeAction = ImeAction.Done),
        textStyle = TextStyle(color = t.text, fontSize = BODY, fontFamily = Sans, textAlign = TextAlign.Center), cursorBrush = SolidColor(t.primary),
        decorationBox = { field ->
            Box(Modifier.fillMaxWidth().height(32.dp).clip(RoundedCornerShape(16.dp)).background(t.containerHigh).padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center) {
                if (text.isEmpty()) T(hint, size = SMALL1, color = t.text3, lines = 1)
                field()
            }
        },
    )
}

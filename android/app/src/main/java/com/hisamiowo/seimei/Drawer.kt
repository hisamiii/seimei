package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.NavigationDrawerItemDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.material3.DrawerDefaults
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.paneTitle
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.math.abs
import org.json.JSONObject
import java.time.LocalDate

// The navigation drawer (M3 modal look, its own swipe): seimei, the recent side threads, agents and watches, files; usage and settings at its foot.
// And the drawer's quieter pages: watches, files, settings.

/** How many recent rows each section shows; its header opens the rest. */
private const val RECENT_THREADS = 3
private const val RECENT_AGENTS = 4
private const val RECENT_WATCHES = 3

/** Whether the menu icon wears its dot: a thread or an agent has news, or seimei does while its chat is out of view. */
fun drawerNews(vm: Seimei): Boolean =
    vm.threads.any { it.optInt("unread") > 0 } || agentCounts(vm).second > 0 || (!vm.chatInView && vm.unread > 0)

/** The agents the user last heard from: what asks and works first, then the newest. */
private fun recentAgents(vm: Seimei): List<Agent> {
    val items = (vm.workers.filter { it.str("state") != "released" }.map(::fromWorker) + vm.sessions.map { fromSession(it, vm.seen) })
        .filter { !it.archived }
    val rank = mapOf("ask" to 0, "work" to 1)
    return items.sortedWith(compareBy<Agent> { rank[it.state] ?: 2 }.thenByDescending { it.since }).take(RECENT_AGENTS)
}

@Composable
fun DrawerSheet(vm: Seimei, pick: (() -> Unit) -> Unit) {
    val cs = MaterialTheme.colorScheme
    val x = LocalExtra.current
    val colors = NavigationDrawerItemDefaults.colors(selectedContainerColor = cs.secondaryContainer, unselectedContainerColor = cs.surfaceContainerLow)
    val tab = if (vm.page == null) vm.tab else ""

    @Composable
    fun Item(label: String, icon: @Composable () -> Unit, on: Boolean = false, trail: (@Composable () -> Unit)? = null, run: () -> Unit) =
        NavigationDrawerItem(
            label = { Text(label, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            selected = on, onClick = { pick(run) }, icon = icon, badge = trail, colors = colors,
            modifier = Modifier.padding(horizontal = 12.dp).height(52.dp),
        )

    @Composable
    fun Ico(v: ImageVector) = Icon(v, null, Modifier.size(22.dp))

    @Composable
    fun Head(label: String, run: () -> Unit) = Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp).heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp)).clickable { pick(run) }
            .padding(start = 16.dp, end = 12.dp).semantics { heading() },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.weight(1f), fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = x.text2)
        Icon(Icons.chev, null, Modifier.size(18.dp), tint = x.text3)
    }

    @Composable
    fun Dot() = Box(Modifier.size(8.dp).clip(CircleShape).background(cs.primary))

    @Composable
    fun Rule() = HorizontalDivider(Modifier.padding(horizontal = 28.dp, vertical = 8.dp), color = cs.outlineVariant)

    ModalDrawerSheet(drawerContainerColor = cs.surfaceContainerLow, drawerShape = RoundedCornerShape(topEnd = 16.dp, bottomEnd = 16.dp)) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(top = 12.dp)) {
            val n = vm.unread
            Item("seimei", { Box(Modifier.size(24.dp).clip(CircleShape).background(cs.primaryContainer), contentAlignment = Alignment.Center) { Glyph(16.dp) } },
                on = tab == "chat", trail = if (n > 0) ({ Text(if (n > 99) "99+" else "$n", fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = cs.primary) }) else null) {
                vm.show("chat")
            }
            Rule()
            Head("Side threads") { vm.show("threads") }
            for (t in vm.threads.take(RECENT_THREADS)) {
                Item(t.str("name"), { Ico(Icons.thread) }, trail = if (t.optInt("unread") > 0) ({ Dot() }) else null) { vm.openThread(t.str("id")) }
            }
            if (vm.threadsArchived > 0) Item("Archived", { Ico(Icons.shelf) }) {
                vm.show("threads")
                vm.threadArchOpen = true
                vm.loadThreads()
            }
            Rule()
            Head("Agents") { vm.show("agents") }
            for (a in recentAgents(vm)) {
                Item(a.name, { MiniTile(a) }, trail = { Text(if (a.state == "done") whenText(a.since) else short(a.since), fontSize = 12.sp, color = x.text3) }) {
                    vm.open(a)
                }
            }
            Rule()
            Head("Watches") { vm.show("watches") }
            for (w in vm.watches.take(RECENT_WATCHES)) {
                Item(w.str("title"), { Ico(Icons.watch[w.str("group")] ?: Icons.watch.getValue("ping")) },
                    trail = if (w.str("group") == "time") ({ Text(due(w.optDouble("at")), fontSize = 12.sp, color = x.text3) }) else null) { vm.show("watches") }
            }
            Rule()
            Item("Files", { Ico(Icons.files) }, on = tab == "files") { vm.show("files") }
        }
        Column(Modifier.navigationBarsPadding().padding(bottom = 12.dp)) {
            HorizontalDivider(Modifier.padding(horizontal = 28.dp, vertical = 8.dp), color = cs.outlineVariant)
            val w = vm.usage?.optJSONObject("windows")?.optJSONObject("five_hour")
            Item("Usage", { UsageRing(vm) }, on = tab == "usage",
                trail = w?.let { { Text("${Math.round(it.optDouble("utilization") * 100)}%", fontSize = 12.sp, color = x.text3) } }) { vm.show("usage") }
            Item("Settings", { Ico(Icons.tune) }, on = tab == "settings") { vm.show("settings") }
        }
    }
}

/** An agent's tile at drawer size: its state's shape and tone, no icon. */
@Composable
private fun MiniTile(a: Agent) {
    val t = LocalTokens.current
    val cs = MaterialTheme.colorScheme
    val bg = when (a.state) {
        "ask" -> t.warn
        "work" -> cs.primary
        "idle" -> t.containerHighest
        else -> cs.surfaceContainerHigh
    }
    if (a.state == "done") Box(Modifier.size(24.dp).clip(CircleShape).background(bg))
    else ShapeTile(a.name, 24.dp, bg, a.state == "work") {}
}

// ---------- watches (crates/core/src/watches.rs rows) ----------

/** A timer's time: its clock today, the weekday and clock after. */
fun due(t: Double): String {
    if (t <= 0 || t.isNaN()) return ""
    if (dayOf(t) == LocalDate.now()) return clock(t)
    return dayOf(t).dayOfWeek.getDisplayName(java.time.format.TextStyle.SHORT, java.util.Locale.getDefault()) + " " + clock(t)
}

/** "in 28m" for a time to come. */
private fun ahead(t: Double): String {
    val s = t - System.currentTimeMillis() / 1000.0
    if (s <= 0) return "due"
    return "in " + when {
        s < 3600 -> "${maxOf(1, Math.round(s / 60))}m"
        s < 86400 -> "${Math.round(s / 3600)}h"
        else -> "${Math.round(s / 86400)}d"
    }
}

private val WATCH_GROUPS = listOf("time" to "Timers", "exits" to "When it exits", "reboot" to "After reboot", "ping" to "Pings")

@Composable
fun WatchesTab(vm: Seimei) {
    var menu by remember { mutableStateOf<JSONObject?>(null) }
    val scroll = rememberScrollState()
    LaunchedEffect(scroll) { snapshotFlow { scroll.value > 0 }.collect { vm.lifted = it } }
    if (vm.watches.isEmpty()) return Empty("No watches", "Timers and what seimei waits for show here")
    Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        for ((g, title) in WATCH_GROUPS) {
            val rows = vm.watches.filter { it.str("group") == g }
            if (rows.isEmpty()) continue
            GroupTitle(title, LocalExtraText3)
            for ((i, w) in rows.withIndex()) WatchRow(w, i, rows.size) { menu = w }
        }
    }
    menu?.let { w ->
        Sheet({ menu = null }, w.str("title")) {
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clip(RoundedCornerShape(16.dp)).clickable {
                menu = null
                vm.removeWatch(w.str("id"))
            }.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.trash, null, Modifier.size(22.dp), tint = LocalExtra.current.text2)
                Spacer(Modifier.width(16.dp))
                Column {
                    Text("Remove", fontSize = 16.sp)
                    Text("seimei hears that you removed it", fontSize = 13.sp, color = LocalExtra.current.text3)
                }
            }
        }
    }
}

@Composable
private fun WatchRow(w: JSONObject, i: Int, n: Int, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val x = LocalExtra.current
    val g = w.str("group")
    val at = w.optDouble("at", 0.0).let { if (it.isNaN()) 0.0 else it }
    val running = w.optBoolean("running")
    Row(
        Modifier.fillMaxWidth().padding(bottom = 2.dp).heightIn(min = 64.dp).clip(segment(i, n)).background(cs.surfaceContainer).clickable(onClick = onClick)
            .padding(start = 10.dp, end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (running) ShapeTile("watch-" + w.str("id"), 44.dp, cs.primary, true) {}
        else Box(Modifier.size(44.dp).clip(CircleShape).background(cs.surfaceContainerHigh), contentAlignment = Alignment.Center) {
            Icon(Icons.watch[g] ?: Icons.watch.getValue("ping"), null, Modifier.size(20.dp), tint = x.text2)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(w.str("title"), Modifier.weight(1f), fontSize = 16.sp, lineHeight = 24.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                // A timer shows when it rings; a command and a reboot note how long ago they were set; a ping has no time of its own
                val time = if (g == "time") due(at) else if (at > 0 && g != "ping") short(at) else ""
                if (time.isNotEmpty()) Text(time, Modifier.padding(start = 8.dp), fontSize = 12.sp, color = x.text3)
            }
            val sub = if (g == "time") listOf(w.str("sub"), if (at > 0) ahead(at) else "", w.str("every").let { if (it.isEmpty()) "" else "every $it" })
                .filter { it.isNotEmpty() }.joinToString(" · ") else w.str("sub")
            if (sub.isNotEmpty()) Text(sub, fontSize = 14.sp, lineHeight = 20.sp, color = x.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun Empty(title: String, line: String) {
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, fontWeight = FontWeight.Medium, fontSize = 16.sp)
        Text(line, Modifier.padding(top = 6.dp), fontSize = 14.sp, color = LocalExtra.current.text3, textAlign = TextAlign.Center)
    }
}

// ---------- files (api/gallery) ----------

@Composable
fun FilesTab(vm: Seimei) {
    var zoom by remember { mutableStateOf<String?>(null) }
    val scroll = rememberScrollState()
    LaunchedEffect(scroll) { snapshotFlow { scroll.value > 0 }.collect { vm.lifted = it } }
    val items = vm.gallery ?: return
    if (items.isEmpty()) return Empty("No files yet", "Pictures and files from the chat show here")
    Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(start = 16.dp, end = 16.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        // Each day: its pictures in a grid, then its files
        for ((_, day) in items.groupBy { dayOf(it.optDouble("t")) }) {
            Text(dayLabel(day.first().optDouble("t")), Modifier.padding(top = 16.dp, bottom = 4.dp), fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
                color = LocalExtra.current.text2)
            val (images, files) = day.partition { it.str("type") == "image" }
            for (row in images.chunked(3)) Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (it in row) {
                    val name = it.str("file")
                    Box(Modifier.weight(1f).aspectRatio(1f).clip(RoundedCornerShape(8.dp)).clickable { zoom = name }) {
                        Picture(vm, name, Modifier.fillMaxSize(), crop = true)
                    }
                }
                // The last row keeps its tiles at a third
                repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
            }
            for (f in files) {
                Column {
                    FileChip(vm, FileRef(f.str("file"), f.str("name"), f.optLong("bytes"), f.str("kind")), Modifier.fillMaxWidth(), expired = expired(f.optDouble("t")))
                    Text(f.str("from"), Modifier.padding(start = 12.dp, top = 2.dp), fontSize = 12.sp, color = LocalExtra.current.text3)
                }
            }
        }
    }
    zoom?.let { Zoom(vm, it) { zoom = null } }
}

// ---------- settings: what the header's menu had ----------

@Composable
fun SettingsTab(vm: Seimei) {
    var themes by remember { mutableStateOf(false) }
    if (themes) ThemeSheet(vm) { themes = false }
    var audit by remember { mutableStateOf(false) }
    if (audit) ActivitySheet(vm) { audit = false }
    var notifications by remember { mutableStateOf(false) }
    if (notifications) NotificationsSheet(vm) { notifications = false }
    val x = LocalTokens.current
    val cs = MaterialTheme.colorScheme
    val rows = buildList<Triple<String, ImageVector, () -> Unit>> {
        val r = vm.release
        add(Triple(if (r != null) "Update to ${r.version}" else "About seimei", Icons.info) { vm.about = true; vm.checkUpdate() })
        if (Delivery.available) add(Triple("Notifications", Icons.bell) { vm.pushTest = null; notifications = true })
        add(Triple("Theme", Icons.palette) { themes = true })
        add(Triple("Activity log", Icons.shield) { audit = true })
        add(Triple("Log out", Icons.logout) { vm.logOut() })
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 24.dp)) {
        for ((i, row) in rows.withIndex()) {
            val (label, icon, run) = row
            val update = i == 0 && vm.release != null
            Row(Modifier.fillMaxWidth().padding(bottom = 2.dp).heightIn(min = 56.dp).clip(segment(i, rows.size)).background(cs.surfaceContainer)
                .clickable(onClick = run).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(icon, null, Modifier.size(22.dp), tint = if (update) cs.primary else x.text3)
                Spacer(Modifier.width(16.dp))
                Text(label, Modifier.weight(1f), fontSize = 16.sp, color = if (update) cs.primary else x.text)
                if (update) Box(Modifier.size(8.dp).clip(CircleShape).background(cs.primary))
            }
        }
    }
}

// ---------- the drawer's host and its swipe ----------

/** Where the drawer is: 0 shut, 1 open, in between while it follows a finger or settles. */
@Stable
class SideDrawer(private val scope: CoroutineScope) {
    var progress by mutableFloatStateOf(0f)
        private set
    /** Open, or on its way there. */
    var isOpen by mutableStateOf(false)
        private set
    private var settling: Job? = null
    /** Children under the finger that take a sideways swipe of their own (see ownSwipe). */
    internal val takers = ArrayList<Taker>()

    fun open() = settle(true)
    fun close() = settle(false)

    /** Goes to open or shut; velocity is in drawer widths per second. */
    fun settle(open: Boolean, velocity: Float = 0f) {
        isOpen = open
        settling?.cancel()
        settling = scope.launch {
            animate(progress, if (open) 1f else 0f, velocity, spring(dampingRatio = 1f, stiffness = Spring.StiffnessMediumLow)) { v, _ -> progress = v }
        }
    }

    /** The drawer's width, from its host: a finger x in pixels maps to progress. */
    internal var widthPx = 1f

    /** Puts the drawer's edge under a finger at x. */
    fun follow(x: Float) = drag(x / widthPx)

    /** Follows a finger: no animation, held between shut and open. */
    fun drag(to: Float) {
        settling?.cancel()
        progress = to.coerceIn(0f, 1f)
    }
}

@Composable
fun rememberSideDrawer(): SideDrawer {
    val scope = rememberCoroutineScope()
    return remember { SideDrawer(scope) }
}

internal class Taker(val takes: (rightward: Boolean) -> Boolean) {
    var at: LayoutCoordinates? = null
}

private val LocalSideDrawer = staticCompositionLocalOf<SideDrawer?> { null }

/** Marks a child with a sideways swipe of its own: a swipe that starts on it stays its own when takes(rightward) says so. */
@Composable
fun Modifier.ownSwipe(takes: (rightward: Boolean) -> Boolean): Modifier {
    val drawer = LocalSideDrawer.current ?: return this
    val now by rememberUpdatedState(takes)
    val taker = remember { Taker { now(it) } }
    DisposableEffect(drawer, taker) {
        drawer.takers += taker
        onDispose { drawer.takers -= taker }
    }
    return onGloballyPositioned { taker.at = it }
}

/** A sideways scroll that keeps a swipe only while it can still scroll that way: at its edge the swipe opens the drawer. */
@Composable
fun Modifier.sideScroll(state: ScrollState): Modifier =
    ownSwipe { right -> if (right) state.canScrollBackward else state.canScrollForward }.horizontalScroll(state)

/** The drawer over content (M3 modal look): a sideways swipe anywhere opens it and closes it, and the drawer follows the finger. */
@Composable
fun SwipeDrawer(state: SideDrawer, sheet: @Composable () -> Unit, content: @Composable () -> Unit) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        // M3's modal drawer: at most 360 wide, and 56 of the screen stays in view beside it
        val width = minOf(360.dp, maxWidth - 56.dp)
        val px = with(LocalDensity.current) { width.toPx() }
        state.widthPx = px
        var here by remember { mutableStateOf<LayoutCoordinates?>(null) }
        Box(Modifier.fillMaxSize().onGloballyPositioned { here = it }.pointerInput(state, px) { swipes(state, px) { here } }) {
            CompositionLocalProvider(LocalSideDrawer provides state) { content() }
            val p = state.progress
            if (p > 0f) {
                val scrim = DrawerDefaults.scrimColor
                Box(Modifier.fillMaxSize().graphicsLayer { alpha = state.progress }.background(scrim)
                    .pointerInput(state) { detectTapGestures { state.close() } }
                    .semantics { contentDescription = "Close the menu"; onClick { state.close(); true } })
                Box(Modifier.width(width).fillMaxHeight().graphicsLayer { translationX = (state.progress - 1f) * px }
                    .semantics { paneTitle = "Menu" }) { sheet() }
            }
        }
    }
}

/** The drawer's swipe over the whole screen, decided at the touch slop before any child can claim the touch. */
private suspend fun PointerInputScope.swipes(state: SideDrawer, px: Float, here: () -> LayoutCoordinates?) {
    val fling = 400.dp.toPx()
    awaitEachGesture {
        // The initial pass sees the touch before any child, so a list or a row's own drag never claims it first.
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val opening = !state.isOpen
        val tracker = VelocityTracker()
        tracker.addPosition(down.uptimeMillis, down.position)
        var moved = Offset.Zero
        while (true) {
            val ev = awaitPointerEvent(PointerEventPass.Initial)
            val ch = ev.changes.firstOrNull { it.id == down.id }
            if (ev.changes.size > 1 || ch == null || !ch.pressed || ch.isConsumed) return@awaitEachGesture
            moved += ch.positionChange()
            tracker.addPosition(ch.uptimeMillis, ch.position)
            if (moved.getDistance() < viewConfiguration.touchSlop) continue
            // Past the slop the first clear direction decides: mostly sideways and toward the drawer's side is the drawer's.
            val right = moved.x > 0
            if (abs(moved.x) <= abs(moved.y) || right != opening) return@awaitEachGesture
            // A held finger is a long press (a message's menu, picking text), not a swipe.
            if (ch.uptimeMillis - down.uptimeMillis > viewConfiguration.longPressTimeoutMillis) return@awaitEachGesture
            val at = here()?.takeIf { it.isAttached }?.localToWindow(down.position)
            if (opening && at != null && state.takers.any { t -> t.at?.takeIf { it.isAttached }?.boundsInWindow()?.contains(at) == true && t.takes(right) }) {
                return@awaitEachGesture
            }
            ch.consume()
            break
        }
        var p = state.progress
        while (true) {
            val ev = awaitPointerEvent(PointerEventPass.Initial)
            val ch = ev.changes.firstOrNull { it.id == down.id } ?: break
            tracker.addPosition(ch.uptimeMillis, ch.position)
            if (!ch.pressed) {
                ch.consume()
                break
            }
            p += ch.positionChange().x / px
            ch.consume()
            state.drag(p)
        }
        // A flick decides by its direction; a slow swipe by how far it went (a third of the way opens or closes it).
        val v = tracker.calculateVelocity().x
        val open = when {
            v > fling -> true
            v < -fling -> false
            opening -> state.progress > 1f / 3
            else -> state.progress > 2f / 3
        }
        state.settle(open, v / px)
    }
}

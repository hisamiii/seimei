package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
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
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

// Side threads: their list and archive, and the sheets of a thread's page (details, its model, send to seimei, make it a shikigami).
// The page itself is the agent page's (Agents.kt, kind "thread").

@Composable
fun ThreadsTab(vm: Seimei) {
    // Relative times move on by themselves.
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(30_000)
            value = System.currentTimeMillis()
        }
    }
    if (vm.threadArchOpen) return ThreadArchive(vm, now)
    val open = vm.threads.map(::fromThread)
    val work = open.filter { it.state == "work" }
    val idle = open.filter { it.state != "work" }
    if (open.isEmpty() && vm.threadsArchived == 0) {
        Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            Text("No side threads yet", fontWeight = FontWeight.Medium, fontSize = 16.sp)
            Text("Start one with the pen above, or hold a message of seimei's", Modifier.padding(top = 6.dp), fontSize = 14.sp,
                color = LocalExtra.current.text3, textAlign = TextAlign.Center)
        }
        return
    }
    val list = rememberLazyListState()
    LaunchedEffect(list) { snapshotFlow { list.canScrollBackward }.collect { vm.lifted = it } }
    LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        for ((title, color, rows) in listOf(Triple("Working", 0, work), Triple("Idle", 1, idle))) {
            if (rows.isEmpty()) continue
            item("h-$title") { GroupTitle(title, if (color == 0) MAUVE else LocalExtraText3) }
            itemsIndexed(rows, key = { _, it -> "t-${it.id}" }) { i, a ->
                val row = @Composable { AgentRow(a, now, shape = segment(i, rows.size)) { vm.open(a) } }
                if (a.state == "idle") SwipeToArchive(a, vm, row) else row()
            }
        }
        if (vm.threadsArchived > 0) item("archived") {
            ArchivedRow(vm.threadsArchived, "${vm.threadsArchived} thread${if (vm.threadsArchived == 1) "" else "s"}") {
                vm.threadArchOpen = true
                vm.loadThreads()
            }
        }
    }
}

@Composable
private fun ThreadArchive(vm: Seimei, now: Long) {
    val close = {
        vm.threadArchOpen = false
        vm.threadArchQuery = ""
    }
    val gesture = predictiveBack(onBack = close)
    val cs = MaterialTheme.colorScheme
    val all = (vm.allThreads ?: emptyList()).filter { it.optBoolean("archived") }.map(::fromThread)
    val q = vm.threadArchQuery.trim().lowercase()
    val hits = if (q.isEmpty()) all else all.filter { a -> a.find.any { it.lowercase().contains(q) } }
    LazyColumn(Modifier.fillMaxSize().then(gesture), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp)) {
        item("ahead") {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(close, Modifier.size(48.dp)) { Icon(Icons.back, "Back to side threads", Modifier.size(22.dp)) }
                Text("Archived", Modifier.weight(1f), fontSize = 18.sp, fontWeight = FontWeight.Medium)
                Text(if (q.isEmpty()) "${all.size}" else "${hits.size} of ${all.size}", fontSize = 13.sp, color = LocalExtra.current.text3,
                    modifier = Modifier.padding(end = 8.dp))
            }
        }
        item("search") {
            TextField(vm.threadArchQuery, { vm.threadArchQuery = it }, Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 8.dp), singleLine = true,
                placeholder = { Text("Search") },
                leadingIcon = { Icon(Icons.search, null, Modifier.size(20.dp)) },
                trailingIcon = { if (vm.threadArchQuery.isNotEmpty()) IconButton({ vm.threadArchQuery = "" }) { Icon(Icons.close, "Clear the search", Modifier.size(18.dp)) } },
                shape = RoundedCornerShape(24.dp), colors = fieldColors())
        }
        if (hits.isEmpty()) item("none") {
            Text(if (q.isEmpty()) "Nothing archived. Quiet ones move here after a day." else "No matches",
                Modifier.fillMaxWidth().padding(32.dp), color = LocalExtra.current.text3, fontSize = 14.sp, textAlign = TextAlign.Center)
        }
        itemsIndexed(hits, key = { _, it -> "ta-${it.id}" }) { _, a -> SwipeToArchive(a, vm) { AgentRow(a, now, q) { vm.open(a) } } }
    }
}

@Composable
fun fieldColors() = MaterialTheme.colorScheme.let { cs ->
    TextFieldDefaults.colors(
        focusedContainerColor = cs.surfaceContainerHigh, unfocusedContainerColor = cs.surfaceContainerHigh,
        disabledContainerColor = cs.surfaceContainerHigh,
        focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent, disabledIndicatorColor = Color.Transparent,
    )
}

/** Cancel and the filled action at the end of a sheet. */
@Composable
private fun SheetButtons(ok: String, enabled: Boolean, cancel: () -> Unit, run: () -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(cancel) { Text("Cancel") }
        Spacer(Modifier.width(8.dp))
        Button(run, enabled = enabled) { Text(ok) }
    }
}

@Composable
private fun Label(text: String) = Text(text, fontSize = 13.sp, color = LocalExtra.current.text3)

/** Send to seimei: the thread writes a short summary, the user edits it, and it goes into the chat as their message. */
@Composable
fun SendSheet(vm: Seimei, pg: Page, done: () -> Unit) {
    var text by remember { mutableStateOf("") }
    var ready by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(pg.id) {
        vm.threadSummary(pg.id).onSuccess { text = it }.onFailure { error = "${it.message}. Write it yourself." }
        ready = true
    }
    Sheet(done, "Send to seimei") {
        TextField(text, { text = it }, Modifier.fillMaxWidth().heightIn(min = 140.dp), enabled = ready,
            placeholder = { Text(if (ready) "What seimei should know" else "Writing a summary…") },
            textStyle = TextStyle(fontSize = 15.sp, lineHeight = 22.sp), shape = RoundedCornerShape(16.dp), colors = fieldColors(),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences))
        error?.let { Text(it, fontSize = 13.sp, color = MaterialTheme.colorScheme.error) }
        SheetButtons("Send", ready && !busy && text.isNotBlank(), done) {
            busy = true
            scope.launch {
                error = vm.threadPost(pg.id, text.trim())
                busy = false
                if (error == null) done()
            }
        }
    }
}

/** Details: its model (tunable), context and start; then Make this a shikigami and Archive. */
@Composable
fun ThreadDetails(vm: Seimei, pg: Page) {
    var view by remember { mutableStateOf("details") }
    val r = pg.row
    val close = { pg.details = false }
    when (view) {
        "model" -> ModelSheet(vm, pg) { view = "details" }
        "shikigami" -> ShikigamiSheet(vm, pg, back = { view = "details" }, done = close)
        else -> Sheet(close, r.str("name")) {
            Text("Side thread", Modifier.padding(top = 0.dp), fontSize = 13.sp, color = LocalExtra.current.text3)
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (r.str("shikigami").isNotEmpty()) Fact("Model", r.str("model"))
                else TuneFact("Model", r.str("model")) { view = "model" }
                Fact("Context", r.context()?.let { "$it% used" } ?: "")
                Fact("Started", atTime(r.optDouble("created")).replace(Regex("^at "), "today at ").cap())
            }
            if (r.str("shikigami").isEmpty()) FilledTonalButton({ view = "shikigami" }, Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Icon(Icons.agents, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Make this a shikigami")
            }
            val a = fromThread(r)
            FilledTonalButton({
                close()
                vm.setArchived(a, !a.archived)
            }, Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Icon(Icons.shelf, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text(if (a.archived) "Unarchive" else "Archive")
            }
        }
    }
}

@Composable
private fun ModelSheet(vm: Seimei, pg: Page, back: () -> Unit) {
    var model by remember { mutableStateOf(pg.row.str("model").ifEmpty { "opus" }) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    Sheet(back, "Change ${pg.row.str("name")}") {
        Label("Model")
        Pills(MODELS, model) { model = it }
        Text("From its next message. A new model rebuilds the cache once.", fontSize = 13.sp, lineHeight = 18.sp, color = LocalExtra.current.text3)
        error?.let { Text(it, fontSize = 13.sp, color = MaterialTheme.colorScheme.error) }
        SheetButtons("Apply", !busy, back) {
            if (model == pg.row.str("model")) return@SheetButtons back()
            busy = true
            scope.launch {
                error = vm.threadTune(pg.id, model)
                busy = false
                if (error == null) back()
            }
        }
    }
}

/** Make this a shikigami: a summon with the thread's text as context; the thread is read-only after. */
@Composable
private fun ShikigamiSheet(vm: Seimei, pg: Page, back: () -> Unit, done: () -> Unit) {
    LaunchedEffect(Unit) { if (vm.lastCwd.isEmpty()) vm.loadThreads() }
    var name by remember { mutableStateOf(pg.row.str("name")) }
    var folder by remember { mutableStateOf<String?>(null) }
    var model by remember { mutableStateOf(pg.row.str("model").ifEmpty { "opus" }) }
    var effort by remember { mutableStateOf("high") }
    var task by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val cwd = folder ?: home(vm.lastCwd).ifEmpty { "~" }
    Sheet(back, "Make this a shikigami") {
        @Composable
        fun ColumnScope.field(label: String, value: String, set: (String) -> Unit, mono: Boolean = false, hint: String = "", lines: Int = 1) {
            Label(label)
            TextField(value, set, Modifier.fillMaxWidth(), singleLine = lines == 1, minLines = lines,
                placeholder = if (hint.isEmpty()) null else ({ Text(hint) }),
                textStyle = TextStyle(fontSize = if (mono) 14.sp else 15.sp, fontFamily = if (mono) FontFamily.Monospace else null),
                shape = RoundedCornerShape(16.dp), colors = fieldColors())
        }
        field("Name", name, { name = it })
        field("Folder", cwd, { folder = it }, mono = true)
        Label("Model")
        Pills(MODELS, model) { model = it }
        Label("Effort")
        Pills(EFFORTS, effort) { effort = it }
        field("Task", task, { task = it }, hint = "What it should do", lines = 3)
        error?.let { Text(it, fontSize = 13.sp, color = MaterialTheme.colorScheme.error) }
        SheetButtons("Summon", !busy && task.isNotBlank(), back) {
            busy = true
            val body = JSONObject().put("name", name.trim()).put("cwd", cwd.trim()).put("model", model).put("effort", effort).put("task", task.trim())
            scope.launch {
                error = vm.threadShikigami(pg.id, body)
                busy = false
                if (error == null) done()
            }
        }
    }
}

/** A tonal pill under a message: "Ask on the side", "Reply in chat". */
@Composable
fun TonalPill(text: String, onClick: () -> Unit) {
    val t = LocalTokens.current
    Text(text, Modifier.padding(top = 11.dp).heightIn(min = 48.dp).clip(androidx.compose.foundation.shape.CircleShape).background(t.tonal)
        .clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp),
        fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = t.onTonal)
}

package com.hisamiowo.seimei

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.Log
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.io.IOException

/** The watch's one login, shared by the app and the notification actions: the key signs a fresh challenge whenever the token is gone. */
object Session {
    @Volatile
    var api: Api? = null
    private val lock = Mutex()

    /** Thrown when the server no longer knows this watch. */
    class Unpaired : IOException("unpaired")

    suspend fun login(ctx: Context, fresh: Boolean = false): Api = lock.withLock {
        val prefs = Prefs(ctx)
        api?.takeIf { !fresh && it.token != null && it.base == prefs.server.trimEnd('/') + "/" }?.let { return it }
        val device = prefs.device ?: throw Unpaired()
        val a = Api(prefs.server)
        val nonce = try {
            a.post("api/devices/challenge", JSONObject().put("device", device)).getString("nonce")
        } catch (e: HttpError) {
            if (e.code == 401) throw Unpaired()
            throw e
        }
        val sig = withContext(Dispatchers.Default) { WatchKey.sign(nonce) }
        a.token = a.post("api/devices/login", JSONObject().put("device", device).put("nonce", nonce).put("signature", sig)).getString("token")
        api = a
        a
    }
}

/** Where the watch is: the chat pages, or one of the screens laid over them (each swipes back to the one under it). */
sealed interface Screen {
    data object Home : Screen
    data object Drawer : Screen
    data object Agents : Screen
    data object Threads : Screen
    data object Watches : Screen
    data object Files : Screen
    data object Usage : Screen
    data object Settings : Screen
    data class Agent(val kind: String, val id: String) : Screen
}

/** A message shown before the server echoes it back; a failed one waits for a retry tap. */
class Pending(val cid: String, val text: String) {
    var failed by mutableStateOf(false)
}

/** One agent's page: a shikigami ("worker"), one of the user's sessions ("session", by pid) or a side thread ("thread"). */
class Page(val kind: String, val id: String) {
    var row by mutableStateOf(JSONObject())
    var items by mutableStateOf<List<Item>>(emptyList())
    /** The open question of a session (an AskUserQuestion input, plus tool_id and answerable). */
    var ask by mutableStateOf<JSONObject?>(null)
    var note by mutableStateOf<String?>(null)
    var draft by mutableStateOf("")
    val sent = mutableStateListOf<Pending>()
    val chatKey get() = (when (kind) { "worker" -> "w:"; "thread" -> "t:"; else -> "s:" }) + id
    private var chat = Chat()
    internal var raw: List<JSONObject> = emptyList()
    internal var next: Long? = null
    internal var last = 0L
    internal var poll: Job? = null
    internal val poke = Channel<Unit>(Channel.CONFLATED)

    fun take(evs: List<JSONObject>, reset: Boolean) {
        val fresh = if (reset) evs else Store.fresh(raw, evs)
        if (reset) chat = Chat()
        raw = (if (reset) fresh else raw + fresh).takeLast(Store.PAGE_KEEP)
        fresh.forEach(chat::add)
        for (e in fresh) if (e.str("type") == "user" && sent.isNotEmpty()) sent.remove(sent.firstOrNull { it.text == e.str("text") } ?: sent.first())
        for (e in fresh) if (e.opt("id") is Number) last = maxOf(last, e.getLong("id"))
        items = chat.items.toList()
    }
}

enum class Phase { Pair, Ready }

class Model(app: Application) : AndroidViewModel(app), Dictation.Host {
    val prefs = Prefs(app)
    private val store = Store(app.noBackupFilesDir)
    override var api = Api(prefs.server)
        private set
    override val scope get() = viewModelScope

    var phase by mutableStateOf(if (prefs.device != null && WatchKey.exists()) Phase.Ready else Phase.Pair)
        private set
    var pairBusy by mutableStateOf(false)
        private set
    var pairError by mutableStateOf<String?>(null)
    var theme by mutableStateOf(prefs.theme)
        private set

    var stack by mutableStateOf(listOf<Screen>(Screen.Home))
        private set
    /** Which page of the home pager shows: 0 the chat, 1 the trajectory. */
    var homePage by mutableStateOf(0)

    var online by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set
    var live by mutableStateOf("")
        private set
    var liveLane by mutableStateOf("c")
        private set
    var items by mutableStateOf<List<Item>>(emptyList())
        private set
    var trail by mutableStateOf<List<Item>>(emptyList())
        private set
    var workers by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var sessions by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var threads by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var threadsArchived by mutableStateOf(0)
        private set
    var allThreads by mutableStateOf<List<JSONObject>?>(null)
        private set
    var usage by mutableStateOf<JSONObject?>(null)
        private set
    var cost by mutableStateOf<JSONObject?>(null)
        private set
    var watches by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var gallery by mutableStateOf<List<JSONObject>?>(null)
        private set
    var page by mutableStateOf<Page?>(null)
        private set
    var seen by mutableStateOf(try { JSONObject(prefs.seen) } catch (e: JSONException) { JSONObject() })
        private set
    var draft by mutableStateOf(prefs.draft)
        private set
    val pending = mutableStateListOf<Pending>()
    /** Counts the user's sends from the chat: the chat jumps to its end on each. */
    var sent by mutableStateOf(0)
        private set
    /** A short line for something that went wrong, shown once. */
    var snack by mutableStateOf<String?>(null)
    var retry: (() -> Unit)? = null
    val dictation = Dictation(this)
    var asrOn by mutableStateOf(false)
        private set
    /** The server takes voice calls. */
    var voiceOn by mutableStateOf(false)
        private set
    /** The assist gesture opened the app: the chat's mic starts as soon as the chat shows. */
    var dictateNow by mutableStateOf(false)

    private var events = ArrayList<JSONObject>()
    private var chat = Chat()
    private var trailChat = Chat(lane = "t")
    private var lastId = 0L
    private var stream: Job? = null
    private var foreground = false
    private var restored = false
    private var chatSave: Job? = null
    private var costFetched = 0L

    override fun tell(text: String, retry: (() -> Unit)?) {
        this.retry = retry
        snack = text
    }

    // ---------- pairing ----------

    /** Pairs with a code from `seimei pair` (or the web page's Settings): the watch keeps only its own key. */
    fun pair(typedServer: String, code: String) {
        if (pairBusy) return
        val server = when (val a = address(typedServer.ifEmpty { prefs.server })) {
            is Address.Bad -> return run { pairError = a.why }
            is Address.Ok -> a.url
        }
        pairBusy = true
        pairError = null
        viewModelScope.launch {
            val a = Api(server)
            try {
                val pub = withContext(Dispatchers.Default) { WatchKey.create() }
                val ctx = getApplication<Application>()
                val name = Settings.Global.getString(ctx.contentResolver, Settings.Global.DEVICE_NAME) ?: android.os.Build.MODEL
                val r = a.post("api/devices/claim", JSONObject().put("code", code.trim().uppercase()).put("name", name).put("public_key", pub))
                prefs.server = server
                prefs.device = r.getString("device")
                store.clear()
                Session.api = null
                phase = Phase.Ready
                if (foreground) connect()
            } catch (e: HttpError) {
                WatchKey.delete()
                pairError = if (e.code == 401) "Wrong or expired code" else "The server said: ${e.message}"
            } catch (e: JSONException) {
                WatchKey.delete()
                pairError = "The server's answer made no sense"
            } catch (e: IOException) {
                WatchKey.delete()
                pairError = "Couldn't reach ${server.substringAfter("://").trimEnd('/')}"
            } finally {
                pairBusy = false
            }
        }
    }

    /** The server no longer knows this watch: back to pairing, with nothing of the old pairing kept. */
    private fun unpair() {
        stream?.cancel()
        api.closeStream()
        Session.api = null
        prefs.device = null
        WatchKey.delete()
        store.clear()
        events.clear()
        lastId = 0
        rebuild()
        stack = listOf(Screen.Home)
        phase = Phase.Pair
    }

    fun pickTheme(name: String) {
        theme = name
        prefs.theme = name
    }

    // ---------- foreground and the stream ----------

    // The route to the server: the phone's Bluetooth proxy, Wi-Fi or the watch's own LTE, as Wear OS picks the default network.
    private val cm = app.getSystemService(ConnectivityManager::class.java)
    private var network: Network? = null
    private var routed = false
    private val routes = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(n: Network) {
            viewModelScope.launch { reroute(n) }
        }

        override fun onLost(n: Network) {
            viewModelScope.launch { if (n == network) reroute(null) }
        }
    }

    init {
        cm.registerDefaultNetworkCallback(routes)
    }

    override fun onCleared() {
        cm.unregisterNetworkCallback(routes)
    }

    /**
     * A new route, or none: the stream over the old one would hang until its read timeout, so it goes now.
     * On a new route the stream opens again at once, without the backoff's wait.
     */
    private fun reroute(n: Network?) {
        val first = !routed
        routed = true
        if (n == network) return
        network = n
        Log.i("seimei", "route " + (n?.let { transport(cm.getNetworkCapabilities(it)) } ?: "none"))
        if (first) return
        api.dropConnections()
        if (n != null && foreground && phase == Phase.Ready) connect()
    }

    fun onStart() {
        foreground = true
        Notify.visible = true
        Notify.clear(getApplication())
        if (phase == Phase.Ready) connect()
    }

    fun onStop() {
        foreground = false
        Notify.visible = false
        saveChat(now = true)
        stream?.cancel()
        stream = null
        api.closeStream()
        online = false
    }

    private fun connect() {
        stream?.cancel()
        online = false
        stream = viewModelScope.launch {
            var wait = 1000L
            var fresh = false
            while (isActive) {
                val ch = Channel<JSONObject>(Channel.UNLIMITED)
                val reader = launch { for (ev in ch) handle(ev) }
                try {
                    api = Session.login(getApplication(), fresh)
                    fresh = false
                    restore()
                    if (events.isEmpty()) loadHistory() else catchUp()
                    api.stream(lastId, onOpen = { ch.trySend(OPEN) }) { ch.trySend(it) }
                } catch (e: Session.Unpaired) {
                    ch.close()
                    snack = "This watch was unpaired on the server"
                    return@launch unpair()
                } catch (e: HttpError) {
                    // A token the server dropped (a restart, a lockdown): sign a new challenge.
                    if (e.code == 401) fresh = true
                } catch (e: IOException) {
                } catch (e: JSONException) {
                }
                ch.close()
                reader.join()
                if (!isActive) break
                if (online) wait = 1000L
                online = false
                delay(wait)
                wait = (wait * 2).coerceAtMost(10_000L)
            }
        }
    }

    private suspend fun loadHistory() {
        val r = api.get("api/history?limit=200")
        val evs = r.getJSONArray("events")
        events = ArrayList(List(evs.length()) { evs.getJSONObject(it) })
        lastId = r.getLong("last")
        r.optJSONObject("state")?.let { setState(it) }
        workers = r.optJSONArray("workers").objects()
        r.optJSONObject("threads")?.let(::takeThreads)
        val echoed = events.map { it.str("cid") }.toSet()
        pending.removeAll { it.cid in echoed }
        rebuild()
        saveChat()
    }

    private suspend fun restore() {
        if (restored) return
        restored = true
        val c = withContext(Dispatchers.IO) { store.read(Store.CHAT) } ?: return
        if (events.isNotEmpty()) return
        val evs = c.optJSONArray("events").objects()
        if (evs.isEmpty()) return
        events = ArrayList(evs)
        lastId = events.last().optLong("id")
        rebuild()
    }

    private suspend fun catchUp() {
        val r = api.get("api/history?limit=0")
        val last = r.getLong("last")
        if (last < lastId || last - lastId > Store.CHAT_KEEP) return loadHistory()
        r.optJSONObject("state")?.let { setState(it) }
        workers = r.optJSONArray("workers").objects()
        r.optJSONObject("threads")?.let(::takeThreads)
    }

    private fun saveChat(now: Boolean = false) {
        if (events.isEmpty() || prefs.device == null) return
        chatSave?.cancel()
        chatSave = viewModelScope.launch {
            if (!now) delay(1500)
            val data = JSONObject().put("events", Store.tail(events, Store.CHAT_KEEP)).toString()
            withContext(Dispatchers.IO + NonCancellable) { store.write(Store.CHAT, data) }
        }
    }

    private fun rebuild() {
        val asks = Asks()
        chat = Chat(asks = asks)
        trailChat = Chat(lane = "t", asks = asks)
        events.forEach(::route)
        publish()
    }

    private fun route(ev: JSONObject) {
        if (ev.inTraj()) {
            trailChat.add(ev)
            chat.fold(ev)
        } else chat.add(ev)
    }

    private fun publish() {
        items = chat.items.toList()
        trail = trailChat.items.toList()
    }

    private fun setState(ev: JSONObject) {
        busy = ev.optBoolean("busy")
        if (!busy) live = ""
    }

    private fun handle(ev: JSONObject) {
        if (ev === OPEN) {
            online = true
            presence()
            refreshPage()
            checkAsr()
            checkVoice()
            registerPush()
            return
        }
        val type = ev.str("type")
        if (ev.has("id")) {
            val id = ev.getLong("id")
            if (id <= lastId) return
            lastId = id
            if (type == "assistant") live = ""
            if (type == "user" && ev.str("cid").isNotEmpty()) pending.removeAll { it.cid == ev.str("cid") }
            events.add(ev)
            route(ev)
            publish()
            if (type == "approval_done") Notify.settled(getApplication(), ev.str("aid"))
            saveChat()
            return
        }
        when (type) {
            "state" -> setState(ev)
            "delta" -> {
                if (live.isEmpty()) liveLane = if (ev.str("lane") == "t") "t" else "c"
                live += ev.str("text")
            }
            "workers" -> {
                workers = ev.optJSONArray("list").objects()
                page?.takeIf { it.kind == "worker" }?.let { pg -> workers.firstOrNull { it.str("id") == pg.id }?.let { pg.row = pg.row.merged(it) } }
            }
            "sessions" -> takeSessions(ev.optJSONArray("list").objects())
            "w" -> workerEvent(ev)
            "threads" -> takeThreads(ev)
            "s" -> page?.takeIf { it.kind == "session" && it.id == ev.optLong("pid").toString() }?.poke?.trySend(Unit)
            "usage" -> usage = ev
        }
    }

    private fun takeThreads(l: JSONObject) {
        threads = l.optJSONArray("list").objects()
        threadsArchived = l.optInt("archived")
        page?.takeIf { it.kind == "thread" }?.let { pg -> threads.firstOrNull { it.str("id") == pg.id }?.let { pg.row = pg.row.merged(it) } }
    }

    private fun takeSessions(list: List<JSONObject>) {
        var fresh = false
        val s = JSONObject(seen.toString())
        for (x in list) {
            val id = x.str("session_id")
            if (id.isNotEmpty() && !s.has(id)) {
                s.put(id, if (x.str("status") == "busy") 0.0 else x.optDouble("active", 0.0))
                fresh = true
            }
        }
        if (fresh) saveSeen(s)
        sessions = list
        page?.takeIf { it.kind == "session" }?.let { pg -> list.firstOrNull { it.optLong("pid").toString() == pg.id }?.let { pg.row = pg.row.merged(it) } }
    }

    private fun saveSeen(s: JSONObject) {
        seen = s
        prefs.seen = s.toString()
    }

    /** The agents, as the phone's lists show them: shikigami that run or ask, and the user's sessions. */
    fun agents(): List<Agent> =
        (workers.filter { it.str("state") in ACTIVE }.map(::fromWorker) + sessions.map { fromSession(it, seen) }).filter { !it.archived }

    /** The agents the drawer names: what asks and works first, then the newest, finished shikigami too. */
    fun recentAgents(n: Int): List<Agent> {
        val all = (workers.filter { it.str("state") != "released" }.map(::fromWorker) + sessions.map { fromSession(it, seen) }).filter { !it.archived }
        val rank = mapOf("ask" to 0, "work" to 1)
        return all.sortedWith(compareBy<Agent> { rank[it.state] ?: 2 }.thenByDescending { it.since }).take(n)
    }

    // ---------- navigation ----------

    fun push(s: Screen) {
        stack = stack + s
        when (s) {
            Screen.Threads -> loadThreads()
            Screen.Watches, Screen.Drawer -> loadWatches()
            Screen.Files -> loadGallery()
            Screen.Usage -> refreshCost()
            is Screen.Agent -> openPage(s.kind, s.id)
            else -> {}
        }
        presence()
    }

    /** Opens [s] from the drawer: the drawer goes, so back leads to the chat. */
    fun go(s: Screen) {
        stack = stack.filter { it == Screen.Home }
        if (s != Screen.Home) push(s) else presence()
    }

    fun back() {
        if (stack.size <= 1) return
        val top = stack.last()
        stack = stack.dropLast(1)
        if (top is Screen.Agent) closePage()
        presence()
    }

    fun open(a: Agent) = push(Screen.Agent(a.kind, a.id))

    /** Tells the server which view is on screen: a finished turn notifies everywhere but on the chat. */
    private fun presence() {
        val view = when (val top = stack.last()) {
            Screen.Home -> if (homePage == 0) "chat" else "trajectory"
            is Screen.Agent -> "page"
            else -> top.toString().lowercase()
        }
        Notify.onChat = view == "chat"
        if (!foreground || !online) return
        viewModelScope.launch {
            try {
                api.post("api/presence", JSONObject().put("view", view))
            } catch (e: IOException) {
            } catch (e: JSONException) {
            }
        }
    }

    fun pageShown(n: Int) {
        if (homePage == n) return
        homePage = n
        presence()
    }

    // ---------- agent pages ----------

    private fun openPage(kind: String, id: String) {
        page?.poll?.cancel()
        val pg = Page(kind, id)
        pg.row = when (kind) {
            "worker" -> workers.firstOrNull { it.str("id") == id }
            "thread" -> threads.firstOrNull { it.str("id") == id }
            else -> sessions.firstOrNull { it.optLong("pid").toString() == id }
        } ?: JSONObject()
        page = pg
        viewModelScope.launch {
            withContext(Dispatchers.IO) { store.read(Store.pageKey(kind, id)) }?.let { k ->
                if (pg.raw.isEmpty()) {
                    pg.take(k.optJSONArray("events").objects(), reset = true)
                    pg.last = k.optLong("last")
                    if (k.has("next")) pg.next = k.getLong("next")
                }
            }
            try {
                val r = api.get(when {
                    kind != "session" -> "api/${kind}s/$id" + if (pg.last > 0) "?after=${pg.last}" else ""
                    pg.next != null -> "api/sessions/$id?from=${pg.next}"
                    else -> "api/sessions/$id"
                })
                if (page !== pg) return@launch
                pg.take(r.getJSONArray("events").objects(), reset = pg.raw.isEmpty() || r.optBoolean("reset"))
                when (kind) {
                    "worker" -> pg.row = pg.row.merged(r.getJSONObject("worker"))
                    "thread" -> pg.row = pg.row.merged(r.getJSONObject("thread"))
                    else -> {
                        takeSession(pg, r)
                        pg.poll = viewModelScope.launch { pollSession(pg) }
                    }
                }
                savePage(pg)
            } catch (e: HttpError) {
                pg.note = if (e.code == 404) "Gone from the server" else "The server said no (${e.code})"
            } catch (e: IOException) {
                pg.note = "Couldn't reach the server"
            }
        }
    }

    private fun takeSession(pg: Page, r: JSONObject) {
        pg.next = r.optLong("next")
        pg.row = pg.row.merged(r.getJSONObject("session"))
        pg.ask = r.optJSONObject("ask")
        val id = pg.row.str("session_id")
        if (id.isNotEmpty()) saveSeen(JSONObject(seen.toString()).put(id, pg.row.optDouble("active", 0.0)))
    }

    private suspend fun pollSession(pg: Page) {
        while (true) {
            withTimeoutOrNull(if (foreground) 2000L else 10_000L) { pg.poke.receive() }
            if (!foreground) continue
            try {
                val r = api.get("api/sessions/${pg.id}?from=${pg.next ?: 0}")
                pg.take(r.getJSONArray("events").objects(), r.optBoolean("reset"))
                takeSession(pg, r)
                savePage(pg)
            } catch (e: IOException) {
            }
        }
    }

    private fun refreshPage() {
        page?.takeIf { it.kind == "session" }?.poke?.trySend(Unit)
        val pg = page?.takeIf { it.kind != "session" } ?: return
        viewModelScope.launch {
            try {
                val r = api.get("api/${pg.kind}s/${pg.id}?after=${pg.last}")
                if (page !== pg) return@launch
                pg.take(r.getJSONArray("events").objects(), reset = false)
                pg.row = pg.row.merged(r.getJSONObject(pg.kind))
                savePage(pg)
            } catch (e: IOException) {
            }
        }
    }

    private fun workerEvent(ev: JSONObject) {
        val pg = page?.takeIf { it.kind != "session" && it.id == ev.str("worker") } ?: return
        val e = ev.optJSONObject("ev") ?: return
        if (e.has("id") && e.getLong("id") <= pg.last) return
        pg.take(listOf(e), reset = false)
        savePage(pg)
    }

    private fun savePage(pg: Page) {
        val data = JSONObject().put("events", JSONArray(pg.raw)).put("last", pg.last).put("row", pg.row)
            .apply { pg.next?.let { put("next", it) } }.toString()
        viewModelScope.launch(Dispatchers.IO + NonCancellable) { store.write(Store.pageKey(pg.kind, pg.id), data) }
    }

    private fun closePage() {
        page?.poll?.cancel()
        page = null
    }

    fun pageSend() {
        val pg = page ?: return
        val text = pg.draft.trim()
        if (text.isEmpty()) return
        dictation.sent(pg.chatKey, text)
        pg.draft = ""
        val ask = pg.ask
        val shown = if (pg.kind == "thread" || (pg.kind == "session" && ask?.optBoolean("answerable") != true))
            Pending(java.util.UUID.randomUUID().toString().take(12), text).also { pg.sent.add(it) } else null
        viewModelScope.launch {
            try {
                val body = JSONObject().put("text", text).put("files", JSONArray())
                when {
                    pg.kind == "worker" -> api.post("api/workers/${pg.id}/send", body)
                    pg.kind == "thread" -> api.post("api/threads/${pg.id}/send", body)
                    ask != null && ask.optBoolean("answerable") -> api.post("api/sessions/${pg.id}/answer", body.put("tool_id", ask.str("tool_id")))
                    else -> api.post("api/sessions/${pg.id}/send", body)
                }
                pg.note = null
            } catch (e: IOException) {
                shown?.let { pg.sent.remove(it) }
                if (pg.draft.isEmpty()) pg.draft = text
                pg.note = errorOf(e)
            }
        }
    }

    /** Presses option n (from 1) of the session's open question. */
    fun pageAnswer(n: Int, onDone: (Boolean) -> Unit) {
        val pg = page ?: return
        val ask = pg.ask ?: return
        viewModelScope.launch {
            try {
                api.post("api/sessions/${pg.id}/answer", JSONObject().put("tool_id", ask.str("tool_id")).put("option", n))
                pg.note = null
                onDone(true)
            } catch (e: IOException) {
                pg.note = errorOf(e)
                onDone(false)
            }
        }
    }

    fun workerStop(pg: Page) {
        viewModelScope.launch {
            try {
                api.post("api/${pg.kind}s/${pg.id}/stop")
            } catch (e: IOException) {
                snack = errorOf(e)
            }
        }
    }

    // ---------- the drawer's lists ----------

    fun loadThreads() {
        viewModelScope.launch {
            try {
                allThreads = api.get("api/threads").optJSONArray("threads").objects()
            } catch (e: IOException) {
            }
        }
    }

    private fun loadWatches() {
        viewModelScope.launch {
            try {
                watches = api.get("api/watches").optJSONArray("watches").objects()
            } catch (e: IOException) {
            }
        }
    }

    private fun loadGallery() {
        viewModelScope.launch {
            try {
                gallery = api.get("api/gallery").optJSONArray("items").objects()
            } catch (e: IOException) {
            }
        }
    }

    private fun refreshCost() {
        val now = System.currentTimeMillis()
        if (now - costFetched < 15_000) return
        costFetched = now
        viewModelScope.launch {
            try {
                val r = api.get("api/cost")
                if (r.has("all")) cost = r
            } catch (e: IOException) {
            }
        }
    }

    // ---------- the chat ----------

    fun edit(text: String) {
        draft = text
        prefs.draft = text
    }

    private fun checkAsr() {
        viewModelScope.launch {
            asrOn = try {
                api.get("api/asr").optBoolean("enabled")
            } catch (e: IOException) {
                asrOn
            } catch (e: JSONException) {
                false
            }
        }
    }

    private fun checkVoice() {
        viewModelScope.launch {
            voiceOn = try {
                api.get("api/voice").optBoolean("enabled")
            } catch (e: IOException) {
                voiceOn
            } catch (e: JSONException) {
                false
            }
        }
    }

    /** Starts a voice call (docs/voice.md), which goes on in the chat; at a cold start it logs in first. The caller has the microphone's permission. */
    fun startCall() {
        go(Screen.Home)
        homePage = 0
        if (CallService.call != null) return
        viewModelScope.launch {
            val a = try {
                Session.login(getApplication())
            } catch (e: IOException) {
                return@launch tell("Couldn't reach the server", ::startCall)
            } catch (e: JSONException) {
                return@launch tell("Couldn't reach the server", ::startCall)
            }
            CallService.start(getApplication(), a)
        }
    }

    /** What the server needs besides the audio (docs/asr.md). */
    fun asrHint(chat: String, text: String): JSONObject =
        JSONObject().put("client", "watch").put("chat", chat).put("screen", if (page != null) "agents" else "chat").put("draft", text.takeLast(2000))

    fun send() {
        val text = draft.trim()
        if (text.isEmpty()) return
        if (!online) {
            snack = "Offline. Your message stays here."
            return
        }
        dictation.sent("chat", text)
        edit("")
        deliver(Pending(java.util.UUID.randomUUID().toString().take(12), text))
    }

    fun retrySend(p: Pending) {
        pending.remove(p)
        deliver(Pending(p.cid + "r", p.text))
    }

    private fun deliver(p: Pending) {
        pending.add(p)
        sent++
        viewModelScope.launch {
            try {
                api.post("api/send", JSONObject().put("text", p.text).put("cid", p.cid).put("images", JSONArray()).put("files", JSONArray()))
            } catch (e: IOException) {
                if (p in pending) p.failed = true
            }
        }
    }

    /** Answers a card; a permission card names its option ("once" or "deny"). */
    fun decide(a: Item.Approval, approve: Boolean, onFail: () -> Unit) {
        val option = if (a.options.isEmpty()) null else if (approve) "once" else "deny"
        viewModelScope.launch {
            try {
                api.post("api/approvals/${a.aid}", JSONObject().put("decision", if (approve) "approve" else "deny").apply { option?.let { put("option", it) } })
            } catch (e: IOException) {
                if (e is HttpError && e.code == 409) return@launch
                onFail()
                snack = "Couldn't send your answer"
            }
        }
    }

    // ---------- notifications ----------

    private fun registerPush() {
        viewModelScope.launch {
            try {
                Notify.register(getApplication(), api, prefs)
            } catch (e: IOException) {
            }
        }
    }

    private fun errorOf(e: IOException): String = (e as? HttpError)?.let {
        try { JSONObject(it.body).str("error") } catch (x: JSONException) { "" }
    }.orEmpty().ifEmpty { if (e is HttpError) "The server said no (${e.code})" else "The server didn't answer" }

    companion object {
        private val OPEN = JSONObject()

        fun JSONObject.merged(o: JSONObject): JSONObject {
            val r = JSONObject(toString())
            o.keys().forEach { r.put(it, o.get(it)) }
            return r
        }
    }
}

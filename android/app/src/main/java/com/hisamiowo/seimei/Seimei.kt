package com.hisamiowo.seimei

import androidx.compose.runtime.mutableStateListOf
import android.app.Activity
import android.app.Application
import android.content.ActivityNotFoundException
import android.content.ContentValues
import android.content.Intent
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.util.LruCache
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.util.Base64

enum class Phase { Pair, Locked, Ready }

/** Text and files another app handed over through the share sheet. */
data class Shared(val text: String, val uris: List<Uri>)

sealed interface Attachment {
    class Photo(val data: String, val preview: ImageBitmap) : Attachment

    /** Any other file: it goes up to the laptop as soon as it is picked, and the message names it by id. */
    class Doc(val name: String, val preview: ImageBitmap? = null) : Attachment {
        var id by mutableStateOf<String?>(null)
        var uploading by mutableStateOf(true)
        var job: Job? = null
    }
}

/** A message shown before the laptop echoes it back (QA U1); a failed one waits for a retry tap. */
class Pending(val cid: String, val text: String, val sending: List<Attachment>, val reply: Pair<Long, String>? = null) {
    var failed by mutableStateOf(false)
}

/** The full-screen page of one agent: a shikigami ("worker", by id) or one of the user's sessions ("session", by pid). */
class Page(val kind: String, val id: String) {
    var row by mutableStateOf(JSONObject())
    var items by mutableStateOf<List<Item>>(emptyList())
    /** The open question (an AskUserQuestion input, plus tool_id and answerable). */
    var ask by mutableStateOf<JSONObject?>(null)
    /** The call that runs now: tool_id, tool, what, since, and its last output lines. */
    var live by mutableStateOf<JSONObject?>(null)
    /** Why the app cannot type into this session, or null. */
    var reach by mutableStateOf<String?>(null)
    var note by mutableStateOf<String?>(null)
    var draft by mutableStateOf("")
    var files by mutableStateOf<List<Attachment.Doc>>(emptyList())
    var details by mutableStateOf(false)
    /** A shikigami's run so far (metrics.py), as of opening the page. */
    var metrics by mutableStateOf<JSONObject?>(null)
    internal val chat = Chat()
    internal var next: Long? = null
    internal var last = 0L
    internal var poll: Job? = null
}

class Seimei(app: Application) : AndroidViewModel(app) {
    val prefs = Prefs(app)
    var api = Api(prefs.server)
        private set

    var phase by mutableStateOf(if (prefs.device != null && DeviceKey.exists()) Phase.Locked else Phase.Pair)
        private set
    var authBusy by mutableStateOf(false)
        private set
    var authError by mutableStateOf<String?>(null)

    var dynamicColor by mutableStateOf(prefs.dynamicColor)
        private set

    var online by mutableStateOf(false)
        private set
    var awaySince by mutableStateOf<Long?>(null)
        private set
    var connected by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set
    var items by mutableStateOf<List<Item>>(emptyList())
        private set
    /** The trajectory: what happened on its own (see Chat.fold); the chat shows a line for each run of it. */
    var trail by mutableStateOf<List<Item>>(emptyList())
        private set
    /** Which of the two the chat tab shows: "c" (chat) or "t" (trajectory). */
    var lane by mutableStateOf("c")
        private set
    /** Trajectory events the user has not seen yet. */
    var unseen by mutableStateOf(0)
        private set
    /** The message of seimei's that the next send answers ("Reply in chat"): its event id and text. */
    var replyTo by mutableStateOf<Pair<Long, String>?>(null)
        private set
    /** A row to scroll to and flash, by key, in the lane on screen. */
    var goto by mutableStateOf<String?>(null)
    var live by mutableStateOf("")
        private set
    /** The lane of the turn streaming into live. */
    var liveLane by mutableStateOf("c")
        private set
    var waiting by mutableStateOf(0)
        private set
    var hasOlder by mutableStateOf(false)
        private set
    var loadingOlder by mutableStateOf(false)
        private set
    var workers by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var sessions by mutableStateOf<List<JSONObject>>(emptyList())
        private set
    var usage by mutableStateOf<JSONObject?>(null)
        private set
    var cost by mutableStateOf<JSONObject?>(null)
    /** Each shikigami's run (api/metrics), newest first. */
    var metrics by mutableStateOf<List<JSONObject>?>(null)
        private set
    /** Every shikigami, finished ones from days ago too; fetched while the Agents tab shows. */
    var allWorkers by mutableStateOf<List<JSONObject>?>(null)
        private set
    var page by mutableStateOf<Page?>(null)
        private set
    var seen by mutableStateOf(JSONObject(prefs.seen))
        private set
    var visible by mutableStateOf(false)
        private set
    var tab by mutableStateOf("chat")
        private set

    /** Counts the user's sends from the chat: the chat jumps to the end on each. */
    var sent by mutableStateOf(0)
        private set
    val pending = mutableStateListOf<Pending>()
    /** A short note for a tap that could not do anything; the screen shows it once. */
    var snack by mutableStateOf<String?>(null)
    /** What the snack's Undo does, set together with snack. */
    var undo: (() -> Unit)? = null
    /** The Archived list (#43) is open, and what it is searched for. */
    var archOpen by mutableStateOf(false)
    var archQuery by mutableStateOf("")

    /** An event the chat should scroll to (a tapped "reply ready"), once it has loaded. */
    var focus by mutableStateOf<Long?>(null)
    var unread by mutableStateOf(false)

    var draft by mutableStateOf(prefs.draft)
        private set
    var attachments by mutableStateOf<List<Attachment>>(emptyList())
        private set

    private var events = ArrayList<JSONObject>()
    private var chat = Chat()
    private var trailChat = Chat(lane = "t")
    private var lastTraj = 0L
    private var lastId = 0L
    private var firstId = 0L
    private var stream: Job? = null
    private var foreground = false
    private var backgroundSince = 0L
    private val images = LruCache<String, ImageBitmap>(48)
    private var allFetched = 0L
    private var costFetched = 0L

    // ---------- pairing and login ----------

    fun pair(activity: Activity, server: String, code: String, name: String) {
        if (authBusy) return
        authBusy = true
        authError = null
        viewModelScope.launch {
            try {
                val fp = canFingerprint(activity)
                if (!fp && !BuildConfig.TEST_KEY) {
                    authError = "Set up a fingerprint on this phone first (Settings, Security)."
                    return@launch
                }
                val url = server.trim().let { if ("://" in it) it else "https://$it" }
                if (url.toHttpUrlOrNull() == null) {
                    authError = "That server address is not valid."
                    return@launch
                }
                api = Api(url)
                val pub = withContext(Dispatchers.Default) { DeviceKey.create(fp) }
                val r = api.post("api/devices/claim", JSONObject().put("code", code).put("name", name.trim()).put("public_key", pub))
                prefs.server = url
                prefs.keyAuth = fp
                prefs.device = r.getString("device")
                phase = Phase.Locked
                authBusy = false
                unlock(activity)
            } catch (e: HttpError) {
                DeviceKey.delete()
                authError = if (e.code == 401) "That code is wrong or has expired. Make a new one on the laptop." else "The laptop said: ${e.message}"
            } catch (e: IOException) {
                DeviceKey.delete()
                authError = "Could not reach $server."
            } finally {
                authBusy = false
            }
        }
    }

    fun unlock(activity: Activity) {
        if (authBusy || phase != Phase.Locked) return
        authBusy = true
        authError = null
        viewModelScope.launch {
            try {
                val device = prefs.device ?: return@launch unpaired(null)
                val nonce = try {
                    api.post("api/devices/challenge", JSONObject().put("device", device)).getString("nonce")
                } catch (e: HttpError) {
                    if (e.code == 401) return@launch unpaired("This phone was unpaired on the laptop. Pair it again with a new code.")
                    throw e
                }
                val signer = try {
                    DeviceKey.signer()
                } catch (e: KeyPermanentlyInvalidatedException) {
                    return@launch unpaired("A fingerprint was added or removed on this phone, so it needs pairing again.")
                }
                val ready = if (prefs.keyAuth) {
                    when (val u = fingerprint(activity, signer)) {
                        is Unlock.Ok -> u.signer
                        Unlock.Cancelled -> return@launch
                        is Unlock.Failed -> {
                            authError = u.why
                            return@launch
                        }
                    }
                } else signer
                val sig = DeviceKey.sign(ready, nonce)
                val r = api.post("api/devices/login", JSONObject().put("device", device).put("nonce", nonce).put("signature", sig))
                api.token = r.getString("token")
                phase = Phase.Ready
                start()
                registerPush()
                checkUpdate()
                pendingShare?.let { pendingShare = null; applyShare(it) }
            } catch (e: HttpError) {
                authError = "Login failed (${e.code})."
            } catch (e: IOException) {
                authError = "Could not reach the laptop. Is it on?"
            } finally {
                authBusy = false
            }
        }
    }

    /** Hands this phone's Firebase token to the laptop, which then may push to it. */
    private fun registerPush() {
        viewModelScope.launch {
            val token = Push.token() ?: prefs.pushToken ?: return@launch
            prefs.pushToken = token
            try {
                api.post("api/devices/push", JSONObject().put("token", token))
            } catch (e: IOException) {
            }
        }
    }

    private fun unpaired(why: String?) {
        DeviceKey.delete()
        prefs.device = null
        api.token = null
        phase = Phase.Pair
        authError = why
    }

    private fun lock() {
        stream?.cancel()
        api.closeStream()
        api.token = null
        online = false
        phase = Phase.Locked
    }

    fun setDynamic(on: Boolean) {
        prefs.dynamicColor = on
        dynamicColor = on
    }

    // ---------- foreground and the stream ----------

    fun onStart() {
        foreground = true
        visible = true
        Push.visible = true
        // Anything pushed while away is on screen now.
        Push.clear(getApplication<Application>())
        installAllowed = Updater.canInstall(getApplication())
        if (phase != Phase.Ready) return
        if (System.currentTimeMillis() - lastCheck > 60_000) checkUpdate()
        // Back after a while away: ask for the fingerprint again.
        if (backgroundSince > 0 && System.currentTimeMillis() - backgroundSince > LOCK_AFTER) return lock()
        connect()
    }

    fun onStop() {
        foreground = false
        visible = false
        Push.visible = false
        backgroundSince = System.currentTimeMillis()
        stream?.cancel()
        stream = null
        api.closeStream()
        online = false
    }

    private fun start() {
        events = ArrayList()
        lastId = 0
        if (foreground) connect()
    }

    private fun connect() {
        stream?.cancel()
        val began = System.currentTimeMillis()
        awaySince = null
        online = false
        stream = viewModelScope.launch {
            // Quiet reconnect: only call the laptop away if it does not answer within the grace period.
            val grace = launch {
                delay(GRACE)
                if (!online && awaySince == null) awaySince = began
            }
            var wait = 1000L
            while (isActive) {
                val ch = Channel<JSONObject>(Channel.UNLIMITED)
                val reader = launch { for (ev in ch) handle(ev) }
                try {
                    if (events.isEmpty()) loadHistory()
                    api.stream(lastId, onOpen = { ch.trySend(OPEN) }) { ch.trySend(it) }
                } catch (e: HttpError) {
                    if (e.code == 401) {
                        ch.close()
                        return@launch lock()
                    }
                } catch (e: IOException) {
                } catch (e: JSONException) {
                }
                ch.close()
                reader.join()
                if (!isActive) break
                if (online) {
                    awaySince = System.currentTimeMillis()
                    wait = 1000L
                }
                online = false
                grace.cancel()
                if (awaySince == null) awaySince = began
                delay(wait)
                wait = (wait * 2).coerceAtMost(10_000L)
            }
        }
    }

    private suspend fun loadHistory() {
        val r = api.get("api/history?limit=200")
        val evs = r.getJSONArray("events")
        events = ArrayList(List(evs.length()) { evs.getJSONObject(it) })
        firstId = events.firstOrNull()?.optLong("id") ?: 0
        hasOlder = firstId > 1
        lastId = r.getLong("last")
        r.optJSONObject("state")?.let { setState(it) }
        takeWorkers(r.optJSONArray("workers").objects())
        // A send that got through while the connection was down is in the history now.
        val echoed = events.map { it.str("cid") }.toSet()
        pending.removeAll { it.cid in echoed }
        rebuild()
        // The first visit starts with nothing unseen; later ones count what came while away.
        val worth = events.filter { it.worth() }
        lastTraj = maxOf(lastTraj, worth.lastOrNull()?.optLong("id") ?: 0)
        if (prefs.trajSeen == 0L) prefs.trajSeen = lastTraj
        unseen = worth.count { it.optLong("id") > prefs.trajSeen }
    }

    fun loadOlder() {
        if (loadingOlder || !hasOlder) return
        loadingOlder = true
        viewModelScope.launch {
            try {
                val evs = api.get("api/history?before=$firstId&limit=200").getJSONArray("events")
                if (evs.length() > 0) {
                    events.addAll(0, List(evs.length()) { evs.getJSONObject(it) })
                    firstId = events[0].optLong("id")
                    rebuild()
                }
                hasOlder = evs.length() > 0 && firstId > 1
            } catch (e: IOException) {
            } finally {
                loadingOlder = false
            }
        }
    }

    private fun rebuild() {
        val asks = Asks()
        chat = Chat(asks = asks)
        trailChat = Chat(lane = "t", asks = asks)
        events.forEach(::route)
        publish()
    }

    /** One event, drawn where it belongs: the trajectory gets it whole, the chat a fold line. */
    private fun route(ev: JSONObject) {
        if (ev.inTraj()) {
            trailChat.add(ev)
            chat.fold(ev)
        } else chat.add(ev)
    }

    private fun publish() {
        items = chat.items.toList()
        trail = trailChat.items.toList()
        waiting = chat.waiting
    }

    // ---------- chat | trajectory ----------

    fun showLane(l: String, key: String? = null) {
        lane = l
        if (l == "t") markSeen()
        goto = key
    }

    private fun markSeen() {
        prefs.trajSeen = maxOf(prefs.trajSeen, lastTraj)
        unseen = 0
    }

    private fun count(ev: JSONObject) {
        if (!ev.worth()) return
        val id = ev.optLong("id")
        lastTraj = maxOf(lastTraj, id)
        if (id <= prefs.trajSeen) return
        if (lane == "t" && tab == "chat" && foreground && page == null) markSeen() else unseen++
    }

    /** "Reply in chat" on a message of seimei's in the trajectory: the next send answers it. */
    fun replyIn(id: Long) {
        replyTo = id to (trailChat.said[id] ?: "")
        showLane("c")
    }

    fun cancelReply() {
        replyTo = null
    }

    private fun setState(ev: JSONObject) {
        connected = ev.optBoolean("connected")
        busy = ev.optBoolean("busy")
        if (!busy) live = ""
    }

    private fun handle(ev: JSONObject) {
        if (ev === OPEN) {
            online = true
            awaySince = null
            presence()
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
            count(ev)
            // The chat dot is for the conversation; the trajectory keeps its own count.
            if (tab != "chat" && !ev.inTraj() && type in setOf("assistant", "approval", "worker", "image", "file")) unread = true
            return
        }
        when (type) {
            "state" -> setState(ev)
            "delta" -> {
                if (live.isEmpty()) liveLane = if (ev.str("lane") == "t") "t" else "c"
                live += ev.str("text")
            }
            "workers" -> takeWorkers(ev.optJSONArray("list").objects())
            "sessions" -> takeSessions(ev.optJSONArray("list").objects())
            "w" -> workerEvent(ev)
            "usage" -> usage = ev
        }
    }

    // ---------- tabs ----------

    fun show(t: String) {
        tab = t
        when (t) {
            "chat" -> {
                unread = false
                if (lane == "t") markSeen()
            }
            "agents" -> refreshAll(force = true)
            "usage" -> refreshCost()
        }
        presence()
    }

    /** Tells the laptop which view is on screen: a finished turn notifies everywhere but on the chat. */
    private fun presence() {
        val view = if (page != null) "page" else tab
        Push.onChat = view == "chat"
        if (view == "chat") Push.cancel(getApplication(), "reply")
        if (!foreground || !online) return
        viewModelScope.launch {
            try {
                api.post("api/presence", JSONObject().put("view", view))
            } catch (e: IOException) {
            } catch (e: JSONException) {
            }
        }
    }

    // ---------- shikigami and sessions ----------

    private fun takeWorkers(list: List<JSONObject>) {
        workers = list
        page?.takeIf { it.kind == "worker" }?.let { pg ->
            list.firstOrNull { it.str("id") == pg.id }?.let { w -> pg.row = pg.row.merged(w) }
        }
        if (tab == "agents") refreshAll()
    }

    private fun refreshAll(force: Boolean = false) {
        val now = System.currentTimeMillis()
        if (!force && now - allFetched < 2000) return
        allFetched = now
        viewModelScope.launch {
            try {
                allWorkers = api.get("api/workers").getJSONArray("workers").objects()
            } catch (e: IOException) {
            }
        }
    }

    private fun takeSessions(list: List<JSONObject>) {
        // A session seen for the first time starts read; only turns that end after that get marked.
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
        page?.takeIf { it.kind == "session" }?.let { pg ->
            list.firstOrNull { it.optLong("pid").toString() == pg.id }?.let { pg.row = pg.row.merged(it) }
        }
    }

    private fun saveSeen(s: JSONObject) {
        seen = s
        prefs.seen = s.toString()
    }

    private fun JSONObject.merged(o: JSONObject): JSONObject {
        val r = JSONObject(toString())
        o.keys().forEach { r.put(it, o.get(it)) }
        return r
    }

    fun openWorker(id: String) = openPage("worker", id)

    fun openSession(pid: Long) = openPage("session", pid.toString())

    private fun openPage(kind: String, id: String) {
        viewModelScope.launch {
            try {
                val r = api.get(if (kind == "worker") "api/workers/$id" else "api/sessions/$id")
                page?.poll?.cancel()
                val pg = Page(kind, id)
                val evs = r.getJSONArray("events").objects()
                evs.forEach(pg.chat::add)
                pg.items = pg.chat.items.toList()
                if (kind == "worker") {
                    pg.last = evs.lastOrNull()?.optLong("id") ?: 0
                    pg.row = r.getJSONObject("worker").merged(workers.firstOrNull { it.str("id") == id } ?: JSONObject())
                    pg.metrics = r.optJSONObject("metrics")
                } else {
                    takeSession(pg, r)
                    pg.poll = viewModelScope.launch { pollSession(pg) }
                }
                page = pg
                presence()
            } catch (e: IOException) {
                snack = "Could not reach the laptop."
            }
        }
    }

    private fun takeSession(pg: Page, r: JSONObject) {
        pg.next = r.optLong("next")
        pg.row = pg.row.merged(r.getJSONObject("session"))
        pg.reach = r.str("reach").ifEmpty { null }
        pg.ask = r.optJSONObject("ask")
        pg.live = r.optJSONObject("doing")?.let { d -> JSONObject(d.toString()).put("output", r.optJSONObject("live")?.optJSONArray("output") ?: JSONArray()) }
        val s = pg.row
        val id = s.str("session_id")
        if (id.isNotEmpty()) saveSeen(JSONObject(seen.toString()).put(id, s.optDouble("active", 0.0)))
    }

    /** A session's page follows its transcript from where the last read ended. */
    private suspend fun pollSession(pg: Page) {
        while (true) {
            delay(if (foreground) 2000 else 10_000)
            if (!foreground) continue
            try {
                val r = api.get("api/sessions/${pg.id}?from=${pg.next ?: 0}")
                if (pg.next != null && r.optLong("next") < pg.next!!) {
                    // The transcript was replaced (compacted or continued): load it again.
                    openPage("session", pg.id)
                    return
                }
                r.getJSONArray("events").objects().forEach(pg.chat::add)
                pg.items = pg.chat.items.toList()
                takeSession(pg, r)
            } catch (e: HttpError) {
                if (e.code == 404) pg.reach = "The session has ended."
            } catch (e: IOException) {
            }
        }
    }

    private fun workerEvent(ev: JSONObject) {
        val pg = page?.takeIf { it.kind == "worker" && it.id == ev.str("worker") } ?: return
        val e = ev.optJSONObject("ev") ?: return
        // The page loaded the log a moment ago; skip what it already holds.
        if (e.has("id") && e.getLong("id") <= pg.last) return
        if (e.has("id")) pg.last = e.getLong("id")
        pg.chat.add(e)
        pg.items = pg.chat.items.toList()
    }

    fun closePage() {
        page?.poll?.cancel()
        page = null
        presence()
    }

    private fun errorOf(e: IOException): String = (e as? HttpError)?.let {
        try { JSONObject(it.body).str("error") } catch (x: JSONException) { "" }
    }.orEmpty().ifEmpty { if (e is HttpError) "The laptop said no (${e.code})." else "The laptop did not answer." }

    /** Sends the page's draft and files: to a shikigami, as the answer to an open question, or typed into a session. */
    fun pageSend() {
        val pg = page ?: return
        val text = pg.draft.trim()
        val files = pg.files
        if (text.isEmpty() && files.isEmpty()) return
        pg.draft = ""
        pg.files = emptyList()
        viewModelScope.launch {
            try {
                files.forEach { it.job?.join() }
                val body = JSONObject().put("text", text).put("files", JSONArray(files.mapNotNull { it.id }))
                val ask = pg.ask
                when {
                    pg.kind == "worker" -> api.post("api/workers/${pg.id}/send", body)
                    ask != null && ask.optBoolean("answerable") -> api.post("api/sessions/${pg.id}/answer", body.put("tool_id", ask.str("tool_id")))
                    else -> api.post("api/sessions/${pg.id}/send", body)
                }
                pg.note = null
            } catch (e: IOException) {
                if (pg.draft.isEmpty()) pg.draft = text
                pg.files = files.filter { it.id != null } + pg.files
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

    /** Photos and files for the page: each uploads to the laptop at once, and the message names it by path. */
    fun pageAttach(uris: List<Uri>) {
        val pg = page ?: return
        val cr = getApplication<Application>().contentResolver
        for (uri in uris) {
            if (pg.files.size >= MAX_FILES) break
            val (name, size) = try {
                cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                    if (c.moveToFirst()) (c.getString(0) ?: "file") to (if (c.isNull(1)) -1L else c.getLong(1)) else null
                }
            } catch (e: Exception) {
                null
            } ?: ("file" to -1L)
            if (size > MAX_FILE) {
                pg.note = "$name is larger than 50 MB."
                continue
            }
            val preview = if (cr.getType(uri) in PICTURES) try {
                ImageDecoder.decodeBitmap(ImageDecoder.createSource(cr, uri)) { d, info, _ ->
                    val k = maxOf(1, maxOf(info.size.width, info.size.height) / 256)
                    d.setTargetSize(info.size.width / k, info.size.height / k)
                }.asImageBitmap()
            } catch (e: Exception) {
                null
            } else null
            val doc = Attachment.Doc(name, preview)
            pg.files = pg.files + doc
            doc.job = viewModelScope.launch {
                try {
                    withContext(Dispatchers.IO) { cr.openInputStream(uri)?.close() ?: throw IOException("cannot read $name") }
                    doc.id = api.upload(name) { cr.openInputStream(uri) ?: throw IOException("cannot read $name") }.str("id")
                } catch (e: SecurityException) {
                    pg.files = pg.files - doc
                    pg.note = "seimei may not read $name."
                } catch (e: IOException) {
                    pg.files = pg.files - doc
                    pg.note = "Could not upload $name: ${errorOf(e)}"
                } finally {
                    doc.uploading = false
                }
            }
        }
    }

    fun pageDetach(d: Attachment.Doc) {
        val pg = page ?: return
        pg.files = pg.files - d
        d.job?.cancel()
    }

    /** Hides a finished shikigami or an idle session from the list, or brings it back; nothing is stopped. */
    fun setArchived(a: Agent, on: Boolean, offerUndo: Boolean = true) {
        val path = if (a.kind == "worker") "api/workers/${a.id}/archive" else "api/sessions/${a.id}/archive"
        // Shown at once; the laptop's listing follows.
        fun flag(list: List<JSONObject>, key: String) = list.map { if (it.optString(key) == a.id) it.merged(JSONObject().put("archived", on)) else it }
        if (a.kind == "worker") {
            workers = flag(workers, "id")
            allWorkers = allWorkers?.let { flag(it, "id") }
        } else sessions = flag(sessions, "pid")
        page?.takeIf { it.kind == a.kind && it.id == a.id }?.let { it.row = it.row.merged(JSONObject().put("archived", on)) }
        if (offerUndo) {
            undo = { setArchived(a, !on, false) }
            snack = "${if (on) "Archived" else "Unarchived"} ${a.name}"
        }
        viewModelScope.launch {
            try {
                api.post(path, JSONObject().put("archived", on))
            } catch (e: IOException) {
                snack = "Could not reach the laptop."
            }
        }
    }

    fun workerStop(id: String) {
        viewModelScope.launch {
            try {
                api.post("api/workers/$id/stop")
            } catch (e: IOException) {
            }
        }
    }

    /** Opens the shikigami in a terminal on the laptop; null when it worked, else why not. */
    suspend fun workerLaptop(id: String): String? = try {
        api.post("api/workers/$id/laptop")
        null
    } catch (e: HttpError) {
        try { JSONObject(e.body).optString("error") } catch (x: JSONException) { "" }.ifEmpty { "no answer" }
    } catch (e: IOException) {
        "no answer"
    }

    // ---------- usage ----------

    fun refreshCost() {
        val now = System.currentTimeMillis()
        if (now - costFetched < 15_000) return
        costFetched = now
        viewModelScope.launch {
            try {
                val r = api.get("api/cost")
                if (r.has("all")) cost = r
                metrics = api.get("api/metrics").optJSONArray("workers")?.objects()
            } catch (e: IOException) {
            }
        }
    }

    // ---------- actions ----------

    fun edit(text: String) {
        draft = text
        prefs.draft = text
    }

    fun send() {
        val text = draft.trim()
        val sending = attachments
        if (text.isEmpty() && sending.isEmpty()) return
        if (!online) {
            snack = "Not connected to the laptop yet. Your message stays here."
            return
        }
        edit("")
        attachments = emptyList()
        val reply = replyTo
        replyTo = null
        if (lane != "c") lane = "c"
        deliver(Pending(java.util.UUID.randomUUID().toString().take(12), text, sending, reply))
    }

    fun retry(p: Pending) {
        pending.remove(p)
        // A file whose upload failed is gone; the rest go again.
        deliver(Pending(p.cid + "r", p.text, p.sending.filter { it !is Attachment.Doc || it.id != null }, p.reply))
    }

    private fun deliver(p: Pending) {
        pending.add(p)
        sent++
        viewModelScope.launch {
            val docs = p.sending.filterIsInstance<Attachment.Doc>()
            try {
                docs.forEach { it.job?.join() }
                if (docs.any { it.id == null }) throw IOException("upload failed")
                val imgs = JSONArray()
                p.sending.filterIsInstance<Attachment.Photo>().forEach { imgs.put(JSONObject().put("data", it.data)) }
                api.post("api/send", JSONObject().put("text", p.text).put("cid", p.cid).put("images", imgs)
                    .put("files", JSONArray(docs.map { it.id })).apply { p.reply?.let { put("reply", it.first) } })
            } catch (e: IOException) {
                if (p in pending) p.failed = true
            }
        }
    }

    fun interrupt() {
        viewModelScope.launch {
            try {
                api.post("api/interrupt")
            } catch (e: IOException) {
            }
        }
    }

    fun decide(aid: String, approve: Boolean, onFail: () -> Unit) {
        viewModelScope.launch {
            try {
                api.post("api/approvals/$aid", JSONObject().put("decision", if (approve) "approve" else "deny"))
            } catch (e: IOException) {
                onFail()
            }
        }
    }

    fun attach(uris: List<Uri>) {
        viewModelScope.launch {
            for (uri in uris) {
                if (attachments.count { it is Attachment.Photo } >= MAX_IMAGES) break
                try {
                    attachments = attachments + withContext(Dispatchers.Default) { prepare(uri) }
                } catch (e: Exception) {
                    chat.local("Could not read that image.")
                    publish()
                }
            }
        }
    }

    fun detach(a: Attachment) {
        attachments = attachments - a
        if (a is Attachment.Doc) a.job?.cancel()
    }

    /** Files from the system picker: real pictures go the photo way, anything else uploads as it is. */
    fun attachFiles(uris: List<Uri>) {
        val cr = getApplication<Application>().contentResolver
        val pictures = mutableListOf<Uri>()
        for (uri in uris) {
            if (cr.getType(uri) in PICTURES && attachments.count { it is Attachment.Photo } + pictures.size < MAX_IMAGES) {
                pictures += uri
                continue
            }
            if (attachments.count { it is Attachment.Doc } >= MAX_FILES) break
            val (name, size) = try {
                cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                    if (c.moveToFirst()) (c.getString(0) ?: "file") to (if (c.isNull(1)) -1L else c.getLong(1)) else null
                }
            } catch (e: Exception) {
                null
            } ?: ("file" to -1L)
            if (size > MAX_FILE) {
                chat.local("$name is larger than 50 MB.")
                publish()
                continue
            }
            val doc = Attachment.Doc(name)
            attachments = attachments + doc
            doc.job = viewModelScope.launch {
                try {
                    // Open it here first: a share whose read grant is gone fails with a message, not mid-upload.
                    withContext(Dispatchers.IO) { cr.openInputStream(uri)?.close() ?: throw IOException("cannot read $name") }
                    doc.id = api.upload(name) { cr.openInputStream(uri) ?: throw IOException("cannot read $name") }.str("id")
                } catch (e: SecurityException) {
                    attachments = attachments - doc
                    chat.local("seimei may not read $name. Share it again from the app it is in.")
                    publish()
                } catch (e: IOException) {
                    attachments = attachments - doc
                    val why = (e as? HttpError)?.let { try { JSONObject(it.body).str("error") } catch (x: JSONException) { "" } }
                    chat.local("Could not upload $name" + if (why.isNullOrEmpty()) "." else ": $why.")
                    publish()
                } finally {
                    doc.uploading = false
                }
            }
        }
        if (pictures.isNotEmpty()) attach(pictures)
    }

    // ---------- sharing from other apps ----------

    /** What another app shared, held until the fingerprint unlock when the app was locked. */
    private var pendingShare: Shared? = null

    /** Shared text joins the draft and shared files the attachments; nothing goes out until the user taps Send. */
    fun share(s: Shared) {
        show("chat")
        if (phase == Phase.Ready && api.token != null) applyShare(s) else pendingShare = s
    }

    private fun applyShare(s: Shared) {
        if (s.text.isNotBlank()) edit(listOf(draft.trimEnd(), s.text.trim()).filter { it.isNotEmpty() }.joinToString("\n"))
        if (s.uris.isNotEmpty()) attachFiles(s.uris)
    }

    // ---------- app updates ----------

    /** A newer release on the laptop, or null. */
    var release by mutableStateOf<Release?>(null)
        private set
    var checking by mutableStateOf(false)
        private set
    var installAllowed by mutableStateOf(false)
        private set
    var about by mutableStateOf(false)
    private var lastCheck = 0L

    fun checkUpdate() {
        if (!Updater.enabled || checking) return
        checking = true
        lastCheck = System.currentTimeMillis()
        viewModelScope.launch {
            try {
                val a = api.get("api/app")
                val code = a.optInt("code")
                release = if (a.optBoolean("available") && code > BuildConfig.VERSION_CODE)
                    Release(a.str("version"), code, a.optLong("size")) else null
            } catch (e: IOException) {
            } catch (e: JSONException) {
            } finally {
                checking = false
            }
        }
    }

    fun installUpdate() {
        val r = release ?: return
        if (Updater.state is Updating.Downloading) return
        viewModelScope.launch { Updater.install(getApplication(), api, r) }
    }

    // ---------- files from the laptop ----------

    /** laptop file id -> its copy in Downloads. */
    private val saved = try { JSONObject(prefs.saved) } catch (e: JSONException) { JSONObject() }
    var saving by mutableStateOf(setOf<String>())
        private set
    var savedIds by mutableStateOf(saved.keys().asSequence().toSet())
        private set

    /** First tap saves the file into Downloads/seimei; once there, a tap opens it in another app. Never opens on its own. */
    fun fileTap(f: FileRef) {
        if (f.id in saving) return
        val cr = getApplication<Application>().contentResolver
        val have = saved.optString(f.id).takeIf { it.isNotEmpty() }?.let(Uri::parse)
        if (have != null && exists(have)) {
            openFile(have, f)
            return
        }
        forget(f.id)
        saving = saving + f.id
        viewModelScope.launch {
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, f.name)
                put(MediaStore.Downloads.MIME_TYPE, mimeOf(f.name))
                put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/seimei")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            var uri: Uri? = null
            try {
                uri = withContext(Dispatchers.IO) { cr.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) } ?: throw IOException("no Downloads")
                val out = withContext(Dispatchers.IO) { cr.openOutputStream(uri) } ?: throw IOException("cannot write")
                out.use { api.download("api/uploads/${f.id}", it) }
                withContext(Dispatchers.IO) { cr.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null) }
                saved.put(f.id, uri.toString())
                prefs.saved = saved.toString()
                savedIds = savedIds + f.id
            } catch (e: Exception) {
                uri?.let { withContext(Dispatchers.IO + NonCancellable) { cr.delete(it, null, null) } }
                if (e !is kotlinx.coroutines.CancellationException) {
                    chat.local(if ((e as? HttpError)?.code == 404) "${f.name} has expired on the laptop." else "Could not download ${f.name}.")
                    publish()
                }
            } finally {
                saving = saving - f.id
            }
        }
    }

    private fun forget(id: String) {
        if (saved.remove(id) != null) {
            prefs.saved = saved.toString()
            savedIds = savedIds - id
        }
    }

    private fun exists(uri: Uri) = try {
        getApplication<Application>().contentResolver.query(uri, arrayOf(MediaStore.Downloads._ID), null, null, null)?.use { it.count > 0 } == true
    } catch (e: Exception) {
        false
    }

    private fun openFile(uri: Uri, f: FileRef) {
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mimeOf(f.name))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            getApplication<Application>().startActivity(view)
        } catch (e: ActivityNotFoundException) {
            chat.local("No app on this phone opens ${f.name}. It is in Downloads/seimei.")
            publish()
        }
    }

    /** The model reads images up to 1568 px on the long side; anything bigger only costs upload time. */
    private fun prepare(uri: Uri): Attachment {
        val cr = getApplication<Application>().contentResolver
        val raw = cr.openInputStream(uri)!!.use { it.readBytes() }
        val mime = cr.getType(uri) ?: ""
        var scaled = false
        val bmp = ImageDecoder.decodeBitmap(ImageDecoder.createSource(raw)) { dec, info, _ ->
            val side = maxOf(info.size.width, info.size.height)
            if (side > MAX_SIDE) {
                scaled = true
                val f = MAX_SIDE.toFloat() / side
                dec.setTargetSize((info.size.width * f).toInt(), (info.size.height * f).toInt())
            }
            dec.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val lossless = mime in setOf("image/png", "image/gif", "image/webp")
        val bytes = if (!scaled && lossless && raw.size <= 3_500_000) raw else ByteArrayOutputStream().also {
            val flat = Bitmap.createBitmap(bmp.width, bmp.height, Bitmap.Config.ARGB_8888)
            android.graphics.Canvas(flat).apply { drawColor(android.graphics.Color.WHITE); drawBitmap(bmp, 0f, 0f, null) }
            flat.compress(Bitmap.CompressFormat.JPEG, 85, it)
        }.toByteArray()
        return Attachment.Photo(Base64.getEncoder().encodeToString(bytes), bmp.asImageBitmap())
    }

    /** A picture from the laptop, fetched with the login token: the chat-sized thumbnail (kept in a small cache),
     *  or the original for the full-screen view (not cached: it can be big). */
    suspend fun image(name: String, full: Boolean = false): ImageBitmap? {
        if (!full) images.get(name)?.let { return it }
        val cap = if (full) 4096 else 2048
        return try {
            val bytes = api.bytes(if (full) "api/files/$name" else "api/files/$name?w=thumb")
            val bmp = withContext(Dispatchers.Default) {
                ImageDecoder.decodeBitmap(ImageDecoder.createSource(bytes)) { dec, info, _ ->
                    val side = maxOf(info.size.width, info.size.height)
                    if (side > cap) {
                        val f = cap.toFloat() / side
                        dec.setTargetSize((info.size.width * f).toInt(), (info.size.height * f).toInt())
                    }
                }.asImageBitmap()
            }
            if (!full) images.put(name, bmp)
            bmp
        } catch (e: Exception) {
            null
        }
    }

    /** Pictures saved from the full-screen view this run, and the ones on their way. */
    var picturesSaved by mutableStateOf(setOf<String>())
        private set
    var picturesSaving by mutableStateOf(setOf<String>())
        private set

    /** Saves the original into Pictures/seimei. */
    fun savePicture(name: String) {
        if (name in picturesSaving || name in picturesSaved) return
        picturesSaving = picturesSaving + name
        val cr = getApplication<Application>().contentResolver
        viewModelScope.launch {
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, "seimei-${name.take(8)}.${name.substringAfterLast('.')}")
                put(MediaStore.Images.Media.MIME_TYPE, mimeOf(name))
                put(MediaStore.Images.Media.RELATIVE_PATH, Environment.DIRECTORY_PICTURES + "/seimei")
                put(MediaStore.Images.Media.IS_PENDING, 1)
            }
            var uri: Uri? = null
            try {
                uri = withContext(Dispatchers.IO) { cr.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) } ?: throw IOException("no Pictures")
                val out = withContext(Dispatchers.IO) { cr.openOutputStream(uri) } ?: throw IOException("cannot write")
                out.use { api.download("api/files/$name", it) }
                withContext(Dispatchers.IO) { cr.update(uri, ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) }, null, null) }
                picturesSaved = picturesSaved + name
            } catch (e: Exception) {
                uri?.let { withContext(Dispatchers.IO + NonCancellable) { cr.delete(it, null, null) } }
                if (e !is kotlinx.coroutines.CancellationException) {
                    chat.local("Could not save the picture.")
                    publish()
                }
            } finally {
                picturesSaving = picturesSaving - name
            }
        }
    }

    companion object {
        val OPEN = JSONObject()
        const val GRACE = 8000L
        const val LOCK_AFTER = 5 * 60_000L
        const val MAX_SIDE = 1568
        const val MAX_IMAGES = 6
        const val MAX_FILES = 10
        const val MAX_FILE = 50L shl 20
        val PICTURES = setOf("image/jpeg", "image/png", "image/webp", "image/gif")

        fun mimeOf(name: String) = MimeTypeMap.getSingleton()
            .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"
    }
}

fun JSONArray?.objects(): List<JSONObject> = if (this == null) emptyList() else List(length()) { getJSONObject(it) }

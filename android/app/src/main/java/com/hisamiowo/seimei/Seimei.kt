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
import android.util.Log
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
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
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

    /** Any other file: it goes up to the server as soon as it is picked, and the message names it by id. */
    class Doc(val name: String, val preview: ImageBitmap? = null) : Attachment {
        var id by mutableStateOf<String?>(null)
        var uploading by mutableStateOf(true)
        var job: Job? = null
    }
}

/** A message shown before the server echoes it back; a failed one waits for a retry tap. */
class Pending(val cid: String, val text: String, val sending: List<Attachment>, val reply: ReplyTo? = null) {
    var failed by mutableStateOf(false)
}

/** The full-screen page of one agent: a shikigami ("worker", by id), one of the user's sessions ("session", by pid), or a side thread ("thread", by id).
 * A draft thread has no id until its first message starts it. */
class Page(val kind: String, id: String) {
    var id by mutableStateOf(id)
    /** Set once: a draft that becomes a thread stays the same screen. */
    val key = "$kind:$id"
    var row by mutableStateOf(JSONObject())
    var items by mutableStateOf<List<Item>>(emptyList())
    /** The open question (an AskUserQuestion input, plus tool_id and answerable). */
    var ask by mutableStateOf<JSONObject?>(null)
    /** The call that runs now: tool_id, tool, what, since, and its last output lines. */
    var live by mutableStateOf<JSONObject?>(null)
    /** Why the app cannot type into this session, or null. */
    var reach by mutableStateOf<String?>(null)
    /** How a message reaches it: typed into its terminal window, "job" (its background job's reply), "seimei" (the dispatcher's SendMessage), or null. */
    var via by mutableStateOf<String?>(null)
    var note by mutableStateOf<String?>(null)
    var draft by mutableStateOf("")
    var files by mutableStateOf<List<Attachment.Doc>>(emptyList())
    /** The hint's name for this chat (docs/asr.md): "w:<id>" or "s:<pid>". */
    val chatKey get() = (when (kind) { "worker" -> "w:"; "thread" -> "t:"; else -> "s:" }) + id
    var details by mutableStateOf(false)
    /** A shikigami's run so far (metrics.rs), fetched when its details open. */
    var metrics by mutableStateOf<JSONObject?>(null)
    internal var chat = Chat()
    /** The events as they came (the last Store.PAGE_KEEP), for the copy kept on the phone. */
    internal var raw: List<JSONObject> = emptyList()
    internal var next: Long? = null
    internal var last = 0L
    internal var poll: Job? = null
    internal var save: Job? = null
    /** The stream says a session's transcript grew: its poll reads at once. */
    internal val poke = Channel<Unit>(Channel.CONFLATED)
    /** Messages sent to a session, shown dimmed until its transcript has them. */
    val sent = mutableStateListOf<Pending>()

    /** Adds what is new in [evs], or starts over with them ([reset]: the server sent a fresh copy). */
    fun take(evs: List<JSONObject>, reset: Boolean) {
        val fresh = if (reset) evs else Store.fresh(raw, evs)
        if (reset) chat = Chat()
        raw = (if (reset) fresh else raw + fresh).takeLast(Store.PAGE_KEEP)
        fresh.forEach(chat::add)
        // The sent message arrived: its dimmed copy goes (the oldest, when the transcript's text differs).
        for (e in fresh) if (e.str("type") == "user" && sent.isNotEmpty()) sent.remove(sent.firstOrNull { it.text == e.str("text") } ?: sent.first())
        for (e in fresh) if (e.opt("id") is Number) last = maxOf(last, e.getLong("id"))
        items = chat.items.toList()
    }
}

class Seimei(app: Application) : AndroidViewModel(app), Dictation.Host {
    val prefs = Prefs(app)
    private val store = Store(app.noBackupFilesDir)
    /** The chat as last seen, read from the phone while the app starts and unlocks. */
    private val stored = viewModelScope.async(Dispatchers.IO) { if (prefs.device != null) store.read(Store.CHAT) else null }
    private var restored = false
    private val storedBoard = viewModelScope.async(Dispatchers.IO) { if (prefs.device != null) store.read(Store.TASKS) else null }
    private var chatSave: Job? = null
    /** Counts page opens, so a slow answer for a page no longer wanted is dropped. */
    private var opening = 0
    override var api = Api(prefs.server)
        private set
    override val scope get() = viewModelScope

    var phase by mutableStateOf(if (prefs.device != null && DeviceKey.exists()) Phase.Locked else Phase.Pair)
        private set
    var authBusy by mutableStateOf(false)
        private set
    var authError by mutableStateOf<String?>(null)
    /** The server being paired with: the server step's answer, the built-in one, or the last one paired. */
    var pairServer by mutableStateOf(prefs.server.ifEmpty { null })
    var serverError by mutableStateOf<String?>(null)
    /** What the server step's field starts with: the saved server, or the one Change went back from. */
    var lastServer = prefs.server

    var theme by mutableStateOf(prefs.theme)
        private set
    var themeMode by mutableStateOf(prefs.themeMode)
        private set
    var accent by mutableStateOf(prefs.accent)
        private set

    var online by mutableStateOf(false)
        private set
    var awaySince by mutableStateOf<Long?>(null)
        private set
    var connected by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set
    /** What the dispatcher runs on: the default harness, or "acp:<name>" for an ACP agent or Codex. */
    var harness by mutableStateOf("")
        private set
    var items by mutableStateOf<List<Item>>(emptyList())
        private set
    /** The trajectory: what happened on its own (see Chat.fold); the chat shows a line for each run of it. */
    var trail by mutableStateOf<List<Item>>(emptyList())
        private set
    /** Which of the two the chat tab shows: "c" (chat) or "t" (trajectory). */
    var lane by mutableStateOf("c")
    /** The screen has moved off its rest (the chat off its end, a list off its top): the top bar fills. */
    var lifted by mutableStateOf(false)
    /** Where each chat lane was left: first row, its offset, and whether the reader was following the end. */
    val laneSpot = HashMap<String, Triple<Int, Int, Boolean>>()
    /** The task board (tasks.rs): {"tasks": [...], "projects": [...]}, the kept copy until the server sends it. */
    var board by mutableStateOf<JSONObject?>(null)
    /** Board sections opened or shut by hand (project name -> open); the rest follow their default. */
    var boardOpen by mutableStateOf(try { JSONObject(prefs.boardOpen) } catch (e: JSONException) { JSONObject() })
        private set
    /** The message of seimei's or the card that the next send answers (a swipe, or "Reply in chat"). */
    var replyTo by mutableStateOf<ReplyTo?>(null)
        private set
    /** Set by a reply: the composer takes the focus and opens the keyboard, then clears it. */
    var focusComposer by mutableStateOf(false)
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
    /** The line TalkBack reads (one polite live region): a card that arrives, a reply, a card that settles. */
    var said by mutableStateOf("")

    private fun spoken(ev: JSONObject): String? = when (ev.str("type")) {
        "assistant" -> "seimei replied"
        "approval" -> "${ev.str("who").ifEmpty { "seimei" }} wants to ${ev.str("target").ifEmpty { ev.str("action") }}"
        "approval_done" -> when (ev.str("outcome")) { "approved" -> "Allowed"; "denied" -> "Denied"; else -> "Canceled" }
        "worker" -> when (ev.str("event")) {
            "asking" -> "${ev.str("name")} asks: ${firstSentence(ev.str("text"))}"
            "done" -> "${ev.str("name")} finished"
            "failed" -> "${ev.str("name")} failed"
            else -> null
        }
        else -> null
    }

    override fun tell(text: String, retry: (() -> Unit)?) {
        if (retry != null) {
            undo = retry
            snackAction = "Retry"
        }
        snack = text
    }

    /** A short note for a tap that could not do anything; the screen shows it once. */
    var snack by mutableStateOf<String?>(null)
    /** What the snack's Undo does, set together with snack. */
    var undo: (() -> Unit)? = null
    /** The label of the snackbar's action that runs undo. */
    var snackAction = "Undo"
    /** The heights of what holds the bottom edge (the composer and navigation bar; a page's composer): snackbars sit above it. */
    var barPx by mutableStateOf(0)
    var pageBarPx by mutableStateOf(0)
    /** The Archived list is open, and what it is searched for. */
    var archOpen by mutableStateOf(false)
    var archQuery by mutableStateOf("")

    /** An event the chat should scroll to (a tapped "reply ready"), once it has loaded. */
    var focus by mutableStateOf<Long?>(null)
    /** seimei's chat messages that came while the chat was not on screen. */
    var unread by mutableStateOf(0)

    /** The open side threads (the stream's listing) and how many are archived; all of them, archived too, from api/threads. */
    var threads by mutableStateOf<List<JSONObject>>(emptyList())
    var threadsArchived by mutableStateOf(0)
    var allThreads by mutableStateOf<List<JSONObject>?>(null)
    var threadArchOpen by mutableStateOf(false)
    var threadArchQuery by mutableStateOf("")
    /** The folder seimei last summoned in: where a shikigami made from a thread starts. */
    var lastCwd by mutableStateOf("")
    /** The armed watch rules (watches.rs rows), and every picture and file of the chat (api/gallery). */
    var watches by mutableStateOf<List<JSONObject>>(emptyList())
    var gallery by mutableStateOf<List<JSONObject>?>(null)
    /** Whether the chat itself is in front of the user: its tab, no page over it, the app on screen. */
    val chatInView get() = tab == "chat" && page == null && foreground

    var draft by mutableStateOf(prefs.draft)
        private set
    /** The mic in the composers; shown when the server can transcribe (GET api/asr). */
    val dictation = Dictation(this)
    var asrOn by mutableStateOf(false)
        private set
    var attachments by mutableStateOf<List<Attachment>>(emptyList())
        private set

    private var events = ArrayList<JSONObject>()
    private var chat = Chat()
    private var trailChat = Chat(lane = "t")
    private var lastId = 0L
    private var firstId = 0L
    private var stream: Job? = null
    private var foreground = false
    private var backgroundSince = 0L
    private val images = LruCache<String, ImageBitmap>(48)
    private var allFetched = 0L
    private var costFetched = 0L

    // ---------- pairing and login ----------

    /** The server step: is this address seimei? Asks its public hello, which says nothing else. */
    fun checkServer(typed: String) {
        if (authBusy) return
        serverError = null
        val a = when (val a = address(typed)) {
            is Address.Bad -> return run { serverError = a.why }
            is Address.Ok -> a
        }
        authBusy = true
        viewModelScope.launch {
            val notSeimei = "${a.shown} answers, but it isn't seimei"
            try {
                if (Api(a.url).get("api/hello").has("seimei")) {
                    pairServer = a.url
                    authError = null
                } else serverError = notSeimei
            } catch (e: HttpError) {
                // 421: seimei, but its config names another address; a browser login would fail the same way.
                serverError = if (e.code == 421) "${a.shown} is seimei, but its config.toml gives another address as origin. Use that one." else notSeimei
            } catch (e: JSONException) {
                serverError = notSeimei
            } catch (e: IOException) {
                serverError = "Couldn't reach ${a.shown}"
            } finally {
                authBusy = false
            }
        }
    }

    /** Back to the server step. */
    fun changeServer() {
        lastServer = pairServer.orEmpty()
        pairServer = null
        authError = null
    }

    /**
     * Pairs with a code from the web page, or with the password: then the phone logs in as the page would, asks for a code and ends that session.
     * Either way the phone keeps only its own key.
     */
    fun pair(activity: Activity, name: String, code: String = "", password: String = "") {
        val server = pairServer ?: return
        if (authBusy) return
        authBusy = true
        authError = null
        viewModelScope.launch {
            api = Api(server)
            try {
                val fp = canFingerprint(activity)
                if (!fp && !BuildConfig.TEST_KEY) {
                    authError = "Set up a fingerprint on this phone first (Settings, Security)"
                    return@launch
                }
                forgetConversations()
                val claimCode = if (password.isEmpty()) code else {
                    try {
                        api.postAsPage("api/auth/password", JSONObject().put("password", password))
                    } catch (e: HttpError) {
                        if (e.code != 401) throw e
                        val wait = runCatching { JSONObject(e.body).optInt("wait") }.getOrDefault(0)
                        authError = if (wait > 0) "Too many wrong tries. Try again in $wait s." else "Wrong password"
                        return@launch
                    }
                    api.postAsPage("api/devices/pair").getString("code")
                }
                val pub = withContext(Dispatchers.Default) { DeviceKey.create(fp) }
                val r = api.post("api/devices/claim", JSONObject().put("code", claimCode).put("name", name.trim()).put("public_key", pub))
                prefs.server = server
                prefs.keyAuth = fp
                prefs.device = r.getString("device")
                api.endPageSession()
                phase = Phase.Locked
                authBusy = false
                unlock(activity)
            } catch (e: HttpError) {
                DeviceKey.delete()
                authError = when (e.code) {
                    401 -> "Wrong or expired code"
                    // The Origin check: the address typed is not the one in the server's config.
                    403 -> "The server refused this address. Use the one its config.toml gives as origin."
                    else -> "The server said: ${e.message}"
                }
            } catch (e: JSONException) {
                DeviceKey.delete()
                authError = "The server's answer made no sense. Is this seimei?"
            } catch (e: IOException) {
                DeviceKey.delete()
                authError = "Couldn't reach ${server.substringAfter("://")}"
            } finally {
                withContext(NonCancellable) { api.endPageSession() }
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
                    if (e.code == 401) return@launch unpaired("This phone was unpaired on the server. Pair it again with a new code.")
                    throw e
                }
                val signer = try {
                    DeviceKey.signer()
                } catch (e: KeyPermanentlyInvalidatedException) {
                    return@launch unpaired("A fingerprint was added or removed on this phone, so it needs pairing again")
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
                Push.live = api
                phase = Phase.Ready
                start()
                registerPush()
                checkUpdate()
                pendingShare?.let { pendingShare = null; applyShare(it) }
            } catch (e: HttpError) {
                authError = "Login failed (${e.code})"
            } catch (e: IOException) {
                authError = "Couldn't reach the server"
            } finally {
                authBusy = false
            }
        }
    }

    /**
     * The lock screen's other way in: seimei's password, as the web page logs in.
     * That session stands in for the phone's key until the app locks again; the key and the pairing stay as they are.
     */
    fun passwordLogin(password: String) {
        if (authBusy || phase != Phase.Locked || password.isEmpty()) return
        authBusy = true
        authError = null
        viewModelScope.launch {
            try {
                api.postAsPage("api/auth/password", JSONObject().put("password", password))
                Push.live = api
                phase = Phase.Ready
                registerPush()
                start()
                checkUpdate()
                pendingShare?.let { pendingShare = null; applyShare(it) }
            } catch (e: HttpError) {
                val wait = runCatching { JSONObject(e.body).optInt("wait") }.getOrDefault(0)
                authError = when {
                    e.code == 401 && wait > 0 -> "Too many wrong tries. Try again in $wait s."
                    e.code == 401 -> "Wrong password"
                    e.code == 403 -> "The server refused this address. Use the one its config.toml gives as origin."
                    else -> "Login failed (${e.code})"
                }
            } catch (e: IOException) {
                authError = "Couldn't reach the server"
            } finally {
                authBusy = false
            }
        }
    }

    /** The test's outcome, shown in the sheet itself: a snack would sit behind it. */
    var pushTest by mutableStateOf<String?>(null)

    /** The Notifications sheet's test: the server sends one through whatever this phone registered. */
    fun testPush() {
        pushTest = "Sending…"
        viewModelScope.launch {
            pushTest = try {
                api.post("api/devices/push/test")
                "Sent"
            } catch (e: HttpError) {
                if (e.code == 409) "This phone has no notifications registered yet" else "The server said: ${e.message}"
            } catch (e: IOException) {
                "Couldn't reach the server"
            }
        }
    }

    /** Tells the server how to reach this phone (Firebase, a push app, or the background connection). */
    fun registerPush() {
        viewModelScope.launch {
            try {
                Delivery.register(getApplication(), api, prefs)
            } catch (e: IOException) {
            }
        }
    }

    private fun unpaired(why: String?) {
        Delivery.forget(getApplication(), prefs)
        DeviceKey.delete()
        prefs.device = null
        api.token = null
        Push.live = null
        phase = Phase.Pair
        authError = why
        forgetConversations()
    }

    /** No copies of conversations stay on a phone that is not paired (or pairs with another server). */
    private fun forgetConversations() {
        restored = true
        chatSave?.cancel()
        page?.let { it.poll?.cancel(); it.save?.cancel() }
        page = null
        opening++
        events = ArrayList()
        lastId = 0
        firstId = 0
        hasOlder = false
        board = null
        rebuild()
        viewModelScope.launch(Dispatchers.IO) { store.clear() }
    }

    /** Menu, then Log out: the app locks; the fingerprint or the password lets the user back in. */
    fun logOut() = lock()

    private fun lock() {
        stream?.cancel()
        api.closeStream()
        api.token = null
        viewModelScope.launch(NonCancellable) { api.endPageSession() }
        Push.live = null
        online = false
        phase = Phase.Locked
    }

    fun setTheme(name: String? = null, mode: String? = null, accent: String? = null) {
        if (name != null) { prefs.theme = name; theme = name }
        if (mode != null) { prefs.themeMode = mode; themeMode = mode }
        if (accent != null) { prefs.accent = accent; this.accent = accent }
    }

    /** Hands the server this phone's wallpaper seed, so the web page's Material You matches it.
     * Only when it changed: a new wallpaper restarts the app's screen, which comes back through here. */
    private fun sendSeed() {
        val seed = wallpaperSeed(getApplication()) ?: return
        if (seed == prefs.seedSent) return
        viewModelScope.launch {
            try {
                api.post("api/theme", JSONObject().put("seed", seed))
                prefs.seedSent = seed
            } catch (e: Exception) {
                Log.w("seimei", "could not send the wallpaper seed: $e")
            }
        }
    }

    // ---------- foreground and the stream ----------

    fun onStart() {
        foreground = true
        visible = true
        if (chatInView) unread = 0
        Push.visible = true
        Push.lockAt = Long.MAX_VALUE
        // Anything pushed while away is on screen now.
        Push.clear(getApplication<Application>())
        installAllowed = Updater.canInstall(getApplication())
        if (phase != Phase.Ready) return
        if (System.currentTimeMillis() - lastCheck > 60_000) checkUpdate()
        // Back after a while away: ask for the fingerprint again.
        if (backgroundSince > 0 && System.currentTimeMillis() - backgroundSince > LOCK_AFTER) return lock()
        connect()
        sendSeed()
    }

    fun onStop() {
        foreground = false
        visible = false
        Push.visible = false
        backgroundSince = System.currentTimeMillis()
        Push.lockAt = backgroundSince + LOCK_AFTER
        saveChat(now = true)
        stream?.cancel()
        stream = null
        api.closeStream()
        online = false
    }

    private fun start() {
        if (foreground) connect()
        sendSeed()
    }

    /** Try now, on the away block: reconnect at once instead of at the next backoff step. */
    fun retryNow() = connect()

    private fun connect() {
        stream?.cancel()
        val began = System.currentTimeMillis()
        awaySince = null
        online = false
        stream = viewModelScope.launch {
            // Quiet reconnect: only call the server away if it does not answer within the grace period.
            val grace = launch {
                delay(GRACE)
                if (!online && awaySince == null) awaySince = began
            }
            var wait = 1000L
            while (isActive) {
                val ch = Channel<JSONObject>(Channel.UNLIMITED)
                val reader = launch { for (ev in ch) handle(ev) }
                try {
                    restore()
                    if (events.isEmpty()) loadHistory() else catchUp()
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
        lastId = r.getLong("last")
        r.optJSONObject("state")?.let { setState(it) }
        takeWorkers(r.optJSONArray("workers").objects())
        r.optJSONObject("threads")?.let(::takeThreads)
        // A send that got through while the connection was down is in the history now.
        val echoed = events.map { it.str("cid") }.toSet()
        pending.removeAll { it.cid in echoed }
        shown()
        saveChat()
    }

    /** The chat kept on the phone, drawn at once on the first connect; the stream then brings what came since. */
    private suspend fun restore() {
        if (restored) return
        restored = true
        if (board == null) board = storedBoard.await()
        val c = stored.await() ?: return
        if (events.isNotEmpty()) return
        val evs = c.optJSONArray("events").objects()
        if (evs.isEmpty()) return
        events = ArrayList(evs)
        lastId = events.last().optLong("id")
        shown()
    }

    /** With a copy at hand, only the state and the list come fresh; a copy further behind than the chat keeps is fetched anew, as on the web page: the replay would replace it all. */
    private suspend fun catchUp() {
        val r = api.get("api/history?limit=0")
        val last = r.getLong("last")
        if (last < lastId || last - lastId > Store.CHAT_KEEP) return loadHistory()
        r.optJSONObject("state")?.let { setState(it) }
        takeWorkers(r.optJSONArray("workers").objects())
        r.optJSONObject("threads")?.let(::takeThreads)
    }

    private fun shown() {
        firstId = events.firstOrNull()?.optLong("id") ?: 0
        hasOlder = firstId > 1
        rebuild()
    }

    /** Keeps the chat's last events on the phone, a moment after they change (or [now], when leaving the app). */
    private fun saveChat(now: Boolean = false) {
        if (events.isEmpty() || prefs.device == null) return
        chatSave?.cancel()
        chatSave = viewModelScope.launch {
            if (!now) delay(1500)
            val data = JSONObject().put("events", Store.tail(events, Store.CHAT_KEEP)).toString()
            withContext(Dispatchers.IO + NonCancellable) { store.write(Store.CHAT, data) }
        }
    }

    private fun savePage(pg: Page) {
        pg.save?.cancel()
        pg.save = viewModelScope.launch {
            delay(1000)
            val data = JSONObject().put("events", JSONArray(pg.raw)).put("last", pg.last).put("row", pg.row)
                .apply { pg.next?.let { put("next", it) } }.toString()
            withContext(Dispatchers.IO + NonCancellable) { store.write(Store.pageKey(pg.kind, pg.id), data) }
        }
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
        goto = key
    }

    /** A swipe on a message of seimei's or a card, or "Reply in chat" in the trajectory: the next send answers it. */
    fun replyIn(r: ReplyTo) {
        replyTo = r
        focusComposer = true
        showLane("c")
    }

    fun cancelReply() {
        replyTo = null
    }

    private fun setState(ev: JSONObject) {
        connected = ev.optBoolean("connected")
        busy = ev.optBoolean("busy")
        harness = ev.optString("harness")
        if (!busy) live = ""
    }

    private fun handle(ev: JSONObject) {
        if (ev === OPEN) {
            online = true
            awaySince = null
            presence()
            loadBoard()
            refreshPage()
            checkAsr()
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
            // A card answered here withdraws its notification at once, without waiting for the server's cancel
            if (type == "approval_done") Push.settled(getApplication(), ev.str("aid"))
            if (System.currentTimeMillis() / 1000.0 - ev.optDouble("t", 0.0) < 60) spoken(ev)?.let { said = it }
            saveChat()
            // The chat dot is for the conversation; the trajectory keeps its own count.
            if (!chatInView && !ev.inTraj() && type in setOf("assistant", "approval", "worker", "image", "file")) unread++
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
            "threads" -> takeThreads(ev)
            "s" -> sessionEvent(ev)
            "usage" -> usage = ev
            "tasks" -> takeBoard(ev)
        }
    }

    // ---------- the task board ----------

    private fun takeBoard(b: JSONObject) {
        val moved = b.optInt("archived") != board?.optInt("archived")
        board = b
        if (taskArchOpen && moved) loadTaskArch()
        val data = b.toString()
        viewModelScope.launch(Dispatchers.IO + NonCancellable) { store.write(Store.TASKS, data) }
    }

    /** The whole board, on each connect: changes made while away went by on the stream. */
    private fun loadBoard() {
        viewModelScope.launch {
            try {
                takeBoard(api.get("api/tasks"))
            } catch (e: IOException) {
            }
        }
    }

    /** The user ticks one of their own items off, or back on; shown at once, confirmed by the board that follows. */
    fun tick(id: String, done: Boolean, offerUndo: Boolean = true) {
        val b = board ?: return
        val shown = JSONObject(b.toString())
        shown.optJSONArray("tasks").objects().firstOrNull { it.str("id") == id }?.let {
            it.put("state", if (done) "done" else "open").put("done_at", if (done) System.currentTimeMillis() / 1000.0 else JSONObject.NULL)
        }
        board = shown
        viewModelScope.launch {
            try {
                api.post("api/tasks/${id.removePrefix("#")}/tick", JSONObject().put("done", done))
                if (offerUndo) {
                    undo = { tick(id, !done, false) }
                    snack = if (done) "Task done" else "Task reopened"
                }
                if (taskArchOpen) loadTaskArch()
            } catch (e: IOException) {
                snack = errorOf(e)
                loadBoard()
            }
        }
    }

    /** The task archive: done items older than a day, newest first, as the server last sent them. */
    var taskArch by mutableStateOf<List<JSONObject>>(emptyList())
    var taskArchOpen by mutableStateOf(false)
    var taskArchQuery by mutableStateOf("")

    fun loadTaskArch() {
        viewModelScope.launch {
            try {
                taskArch = api.get("api/tasks/archive").optJSONArray("tasks").objects()
            } catch (e: IOException) {
            }
        }
    }

    fun toggleProject(name: String, open: Boolean) {
        boardOpen = JSONObject(boardOpen.toString()).put(name, open)
        prefs.boardOpen = boardOpen.toString()
    }

    // ---------- tabs ----------

    fun show(t: String) {
        tab = t
        when (t) {
            "chat" -> {
                if (page == null) unread = 0
            }
            "agents" -> refreshAll(force = true)
            "threads" -> {
                threadArchOpen = false
                loadThreads()
            }
            "watches" -> loadWatches()
            "files" -> loadGallery()
            "usage" -> refreshCost()
        }
        presence()
    }

    /** Tells the server which view is on screen: a finished turn notifies everywhere but on the chat. */
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
                val all = api.get("api/workers").getJSONArray("workers").objects()
                allWorkers = all
                val gone = all.filter { it.optBoolean("archived") }.map { Store.pageKey("worker", it.str("id")) }
                withContext(Dispatchers.IO) { gone.forEach(store::drop) }
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

    fun openThread(id: String) = openPage("thread", id)

    fun openSession(pid: Long) = openPage("session", pid.toString())

    private fun openPage(kind: String, id: String) {
        val token = ++opening
        page?.poll?.cancel()
        viewModelScope.launch {
            val kept = withContext(Dispatchers.IO) { store.read(Store.pageKey(kind, id)) }
            if (token != opening) return@launch
            val listed = when (kind) {
                "worker" -> workers.firstOrNull { it.str("id") == id }
                "thread" -> threads.firstOrNull { it.str("id") == id } ?: allThreads?.firstOrNull { it.str("id") == id }
                else -> sessions.firstOrNull { it.optLong("pid").toString() == id }
            } ?: JSONObject()
            val shown = kept?.let { k ->
                Page(kind, id).also { pg ->
                    pg.take(k.optJSONArray("events").objects(), reset = true)
                    pg.last = k.optLong("last")
                    if (k.has("next")) pg.next = k.getLong("next")
                    pg.row = (k.optJSONObject("row") ?: JSONObject()).merged(listed)
                    // The copy keeps polling, and the stream's reconnect refreshes a worker's page.
                    if (kind == "session") pg.poll = viewModelScope.launch { pollSession(pg) }
                    page = pg
                    presence()
                }
            }
            try {
                var r = api.get(when {
                    kind == "worker" || kind == "thread" -> "api/${kind}s/$id" + if (shown != null) "?after=${shown.last}" else ""
                    shown?.next != null -> "api/sessions/$id?from=${shown.next}"
                    else -> "api/sessions/$id"
                })
                // A pid the system gave to another session since: that one is new to this phone.
                val sid = shown?.row?.str("session_id").orEmpty()
                if (kind == "session" && shown?.next != null && sid.isNotEmpty() && r.getJSONObject("session").str("session_id") != sid)
                    r = api.get("api/sessions/$id").put("reset", true)
                if (token != opening) return@launch
                val pg = shown ?: Page(kind, id)
                pg.take(r.getJSONArray("events").objects(), reset = shown == null || r.optBoolean("reset"))
                if (kind == "worker") pg.row = r.getJSONObject("worker").merged(listed)
                else if (kind == "thread") pg.row = listed.merged(r.getJSONObject("thread"))
                else {
                    pg.row = listed
                    takeSession(pg, r)
                    if (pg.poll == null) pg.poll = viewModelScope.launch { pollSession(pg) }
                }
                if (shown == null) {
                    page = pg
                    presence()
                }
                savePage(pg)
                if (kind == "thread") seenSoon(pg)
            } catch (e: HttpError) {
                if (shown == null) snack = "Couldn't reach the server"
                else if (e.code == 404) shown.note = "Gone from the server: this is the last copy"
            } catch (e: IOException) {
                snack = if (shown == null) "Couldn't reach the server" else "Offline: this is the last copy"
            }
        }
    }

    /** A shikigami's run so far, for its details: priced on the server when asked, not with every page load. */
    fun loadMetrics(pg: Page) {
        if (pg.kind != "worker") return
        viewModelScope.launch {
            try {
                pg.metrics = api.get("api/workers/${pg.id}/metrics").optJSONObject("metrics")
            } catch (e: IOException) {
            }
        }
    }

    private fun takeSession(pg: Page, r: JSONObject) {
        pg.next = r.optLong("next")
        pg.row = pg.row.merged(r.getJSONObject("session"))
        pg.reach = r.str("reach").ifEmpty { null }
        pg.via = r.str("via").ifEmpty { null }
        pg.ask = r.optJSONObject("ask")
        pg.live = r.optJSONObject("doing")?.let { d -> JSONObject(d.toString()).put("output", r.optJSONObject("live")?.optJSONArray("output") ?: JSONArray()) }
        val s = pg.row
        val id = s.str("session_id")
        if (id.isNotEmpty()) saveSeen(JSONObject(seen.toString()).put(id, s.optDouble("active", 0.0)))
    }

    /** A session's page follows its transcript from where the last read ended: at once when the stream says it grew. */
    private suspend fun pollSession(pg: Page) {
        while (true) {
            withTimeoutOrNull(if (foreground) 2000L else 10_000L) { pg.poke.receive() }
            if (!foreground) continue
            try {
                val r = api.get("api/sessions/${pg.id}?from=${pg.next ?: 0}")
                if (pg.next != null && r.optLong("next") < pg.next!!) {
                    // The transcript was replaced (compacted or continued): load it again.
                    openPage("session", pg.id)
                    return
                }
                pg.take(r.getJSONArray("events").objects(), r.optBoolean("reset"))
                takeSession(pg, r)
                savePage(pg)
            } catch (e: HttpError) {
                if (e.code == 404) pg.reach = "The session has ended"
            } catch (e: IOException) {
            }
        }
    }

    /** Back online: the open page fetches what came while the stream was down. */
    private fun refreshPage() {
        page?.takeIf { it.kind == "session" }?.poke?.trySend(Unit)
        val pg = page?.takeIf { (it.kind == "worker" || it.kind == "thread") && it.id.isNotEmpty() } ?: return
        viewModelScope.launch {
            try {
                val r = api.get("api/${pg.kind}s/${pg.id}?after=${pg.last}")
                if (page !== pg) return@launch
                pg.take(r.getJSONArray("events").objects(), reset = false)
                pg.row = pg.row.merged(r.getJSONObject(pg.kind))
                savePage(pg)
                if (pg.kind == "thread") seenSoon(pg)
            } catch (e: IOException) {
            }
        }
    }

    private fun sessionEvent(ev: JSONObject) {
        page?.takeIf { it.kind == "session" && it.id == ev.optLong("pid").toString() }?.poke?.trySend(Unit)
    }

    private fun workerEvent(ev: JSONObject) {
        val pg = page?.takeIf { (it.kind == "worker" || it.kind == "thread") && it.id == ev.str("worker") } ?: return
        val e = ev.optJSONObject("ev") ?: return
        // The page loaded the log a moment ago; skip what it already holds.
        if (e.has("id") && e.getLong("id") <= pg.last) return
        pg.take(listOf(e), reset = false)
        savePage(pg)
        if (pg.kind == "thread") seenSoon(pg)
    }

    fun closePage() {
        opening++
        page?.poll?.cancel()
        page = null
        if (tab == "chat") unread = 0
        presence()
    }

    // ---------- side threads ----------

    private fun takeThreads(l: JSONObject) {
        threads = l.optJSONArray("list").objects()
        val moved = l.optInt("archived") != threadsArchived
        threadsArchived = l.optInt("archived")
        page?.takeIf { it.kind == "thread" && it.id.isNotEmpty() }?.let { pg ->
            threads.firstOrNull { it.str("id") == pg.id }?.let { pg.row = pg.row.merged(it) }
        }
        if (tab == "threads" && (threadArchOpen || moved)) loadThreads()
    }

    /** All the threads, archived too, and the folder a shikigami made from one starts in. */
    fun loadThreads() {
        viewModelScope.launch {
            try {
                val r = api.get("api/threads")
                allThreads = r.optJSONArray("threads").objects()
                lastCwd = r.str("cwd").ifEmpty { lastCwd }
            } catch (e: IOException) {
            }
        }
    }

    /** A new side thread, from the top bar or about a message of seimei's: a page with no thread yet, until the first send. */
    fun openDraft(quote: ReplyTo? = null) {
        opening++
        page?.poll?.cancel()
        val row = JSONObject().put("name", "").put("state", "new")
        if (quote != null) row.put("quote", JSONObject().put("id", quote.id).put("text", quote.text))
        page = Page("thread", "").also { it.row = row }
        presence()
    }

    private var seenJob: Job? = null

    /** What the user saw of an open thread, told to the server a moment later: its unread count and its pushes go by it. */
    private fun seenSoon(pg: Page) {
        seenJob?.cancel()
        seenJob = viewModelScope.launch {
            delay(800)
            if (page !== pg || !foreground || pg.id.isEmpty()) return@launch
            try {
                api.post("api/threads/${pg.id}/seen", JSONObject().put("last", pg.last))
            } catch (e: IOException) {
            }
        }
    }

    /** The thread's row after an action of the user's: its page and the lists take it. */
    fun threadRow(row: JSONObject) {
        val id = row.str("id")
        fun put(list: List<JSONObject>) = list.map { if (it.str("id") == id) it.merged(row) else it }
        threads = put(threads)
        allThreads = allThreads?.let(::put)
        page?.takeIf { it.kind == "thread" && it.id == id }?.let { it.row = it.row.merged(row) }
    }

    /** The send sheet's draft: the thread's summary of itself. */
    suspend fun threadSummary(id: String): Result<String> = try {
        Result.success(api.post("api/threads/$id/summary").str("text"))
    } catch (e: IOException) {
        Result.failure(IOException(errorOf(e)))
    }

    /** Send to seimei: the text goes into the chat as the user's message, tagged with the thread; null when it went. */
    suspend fun threadPost(id: String, text: String): String? = try {
        api.post("api/threads/$id/post", JSONObject().put("text", text))
        undo = { closePage(); show("chat") }
        snackAction = "Show"
        snack = "Sent to seimei"
        null
    } catch (e: IOException) {
        errorOf(e)
    }

    suspend fun threadTune(id: String, model: String): String? = try {
        threadRow(api.post("api/threads/$id/tune", JSONObject().put("model", model)))
        null
    } catch (e: IOException) {
        errorOf(e)
    }

    /** Make this a shikigami: a summon with the thread's text as context; the thread is read-only after. */
    suspend fun threadShikigami(id: String, body: JSONObject): String? = try {
        val r = api.post("api/threads/$id/shikigami", body)
        r.optJSONObject("thread")?.let(::threadRow)
        val worker = r.str("worker")
        undo = { openWorker(worker) }
        snackAction = "Open"
        snack = "Summoned ${r.str("name")}"
        null
    } catch (e: IOException) {
        errorOf(e)
    }

    // ---------- watches and files ----------

    fun loadWatches() {
        viewModelScope.launch {
            try {
                watches = api.get("api/watches").optJSONArray("watches").objects()
            } catch (e: IOException) {
            }
        }
    }

    fun removeWatch(id: String) {
        viewModelScope.launch {
            try {
                api.post("api/watches/$id/remove")
                watches = watches.filter { it.str("id") != id }
                snack = "Removed"
            } catch (e: IOException) {
                snack = errorOf(e)
            }
        }
    }

    fun loadGallery() {
        viewModelScope.launch {
            try {
                gallery = api.get("api/gallery").optJSONArray("items").objects()
            } catch (e: IOException) {
            }
        }
    }

    private fun errorOf(e: IOException): String = (e as? HttpError)?.let {
        try { JSONObject(it.body).str("error") } catch (x: JSONException) { "" }
    }.orEmpty().ifEmpty { if (e is HttpError) "The server said no (${e.code})" else "The server didn't answer" }

    /** Sends the page's draft and files: to a shikigami, as the answer to an open question, or typed into a session. */
    fun pageSend() {
        val pg = page ?: return
        val text = pg.draft.trim()
        val files = pg.files
        if (text.isEmpty() && files.isEmpty()) return
        dictation.sent(pg.chatKey, text)
        pg.draft = ""
        pg.files = emptyList()
        val ask = pg.ask
        val shown = if (pg.kind == "thread" || (pg.kind == "session" && ask?.optBoolean("answerable") != true))
            Pending(java.util.UUID.randomUUID().toString().take(12), text.ifEmpty { files.joinToString(", ") { it.name } }, emptyList()).also { pg.sent.add(it) }
        else null
        viewModelScope.launch {
            try {
                files.forEach { it.job?.join() }
                val body = JSONObject().put("text", text).put("files", JSONArray(files.mapNotNull { it.id }))
                when {
                    pg.kind == "worker" -> api.post("api/workers/${pg.id}/send", body)
                    pg.kind == "thread" && pg.id.isEmpty() -> {
                        pg.row.optJSONObject("quote")?.let { body.put("quote", it.optLong("id")) }
                        val row = api.post("api/threads", body).getJSONObject("thread")
                        // The draft is a thread now: its log so far, and the stream's events for it from here
                        pg.id = row.str("id")
                        pg.row = pg.row.merged(row)
                        refreshPage()
                    }
                    pg.kind == "thread" -> api.post("api/threads/${pg.id}/send", body)
                    ask != null && ask.optBoolean("answerable") -> api.post("api/sessions/${pg.id}/answer", body.put("tool_id", ask.str("tool_id")))
                    else -> {
                        val r = api.post("api/sessions/${pg.id}/send", body)
                        // The session held a relayed message for the user's OK: say where to give it.
                        if (r.optBoolean("held")) snack = r.str("note").ifEmpty { "It waits for your OK in the session" }
                        else if (r.str("via") == "seimei") snack = "seimei passes it on"
                    }
                }
                pg.note = null
            } catch (e: IOException) {
                shown?.let { pg.sent.remove(it) }
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

    /** Photos and files for the page: each uploads to the server at once, and the message names it by path. */
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
                    pg.note = "Couldn't upload $name: ${errorOf(e)}"
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
        val path = "api/${a.kind}s/${a.id}/archive"
        // Shown at once; the server's listing follows.
        showArchived(a, on)
        if (on && page?.let { it.kind == a.kind && it.id == a.id } != true) viewModelScope.launch(Dispatchers.IO) { store.drop(Store.pageKey(a.kind, a.id)) }
        if (offerUndo) {
            undo = { setArchived(a, !on, false) }
            snack = "${if (on) "Archived" else "Unarchived"} ${a.name}"
        }
        viewModelScope.launch {
            // Refused or unreachable: the list goes back to what the server has, as the page's does.
            val failed = try {
                api.post(path, JSONObject().put("archived", on))
                null
            } catch (e: HttpError) {
                runCatching { JSONObject(e.body).optString("error") }.getOrNull()?.ifEmpty { null } ?: "Couldn't ${if (on) "archive" else "unarchive"} ${a.name}"
            } catch (e: IOException) {
                "Couldn't reach the server"
            }
            if (failed != null) {
                showArchived(a, !on)
                undo = null
                snack = failed
            }
        }
    }

    private fun showArchived(a: Agent, on: Boolean) {
        fun flag(list: List<JSONObject>, key: String) = list.map { if (it.optString(key) == a.id) it.merged(JSONObject().put("archived", on)) else it }
        when (a.kind) {
            "worker" -> {
                workers = flag(workers, "id")
                allWorkers = allWorkers?.let { flag(it, "id") }
            }
            "thread" -> {
                threads = flag(threads, "id")
                allThreads = allThreads?.let { flag(it, "id") }
            }
            else -> sessions = flag(sessions, "pid")
        }
        page?.takeIf { it.kind == a.kind && it.id == a.id }?.let { it.row = it.row.merged(JSONObject().put("archived", on)) }
    }

    fun workerStop(id: String, kind: String = "worker") {
        viewModelScope.launch {
            try {
                api.post("api/${kind}s/$id/stop")
            } catch (e: IOException) {
            }
        }
    }

    /** The Change sheet: a new effort, model or permission mode for a shikigami; null when it took. */
    suspend fun workerTune(pg: Page, body: JSONObject): String? = try {
        pg.row = pg.row.merged(api.post("api/workers/${pg.id}/tune", body))
        loadMetrics(pg)
        null
    } catch (e: HttpError) {
        // The server says why as {"error": ...}; the page shows that line too.
        runCatching { JSONObject(e.body).optString("error") }.getOrNull()?.ifEmpty { null }
            ?: e.body.trim().take(200).ifEmpty { "The server said no (${e.code})" }
    } catch (e: IOException) {
        "The server didn't answer"
    }

    /** Opens the shikigami in a terminal on the server; null when it worked, else why not. */
    suspend fun workerLaptop(id: String): String? = try {
        api.post("api/workers/$id/laptop")
        null
    } catch (e: HttpError) {
        try { JSONObject(e.body).optString("error") } catch (x: JSONException) { "" }.ifEmpty { "The server said no (${e.code})" }
    } catch (e: IOException) {
        "The server didn't answer"
    }

    /** Take over (a shikigami goes on as the user's own session) or Hand off (a session becomes a shikigami once its chat card is approved). */
    fun handoff(pg: Page) {
        val worker = pg.kind == "worker"
        viewModelScope.launch {
            try {
                api.post(if (worker) "api/workers/${pg.id}/release" else "api/sessions/${pg.id}/adopt")
                if (worker) {
                    closePage()
                    snack = "${pg.row.str("name")} is yours, on the server"
                } else snack = "Allow it in Chat to hand it off"
            } catch (e: HttpError) {
                snack = try { JSONObject(e.body).optString("error") } catch (x: JSONException) { "" }.ifEmpty { errorOf(e) }
            } catch (e: IOException) {
                snack = errorOf(e)
            }
        }
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

    /** What the server needs besides the audio: where the user is and what is in the draft (docs/asr.md). */
    fun asrHint(chat: String, text: String): JSONObject =
        JSONObject().put("client", "phone").put("chat", chat).put("screen", if (page != null) "agents" else tab).put("draft", text.takeLast(2000))

    fun send() {
        val text = draft.trim()
        val sending = attachments
        if (text.isEmpty() && sending.isEmpty()) return
        if (!online) {
            snack = "Offline. Your message stays here."
            return
        }
        dictation.sent("chat", text)
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
                    .put("files", JSONArray(docs.map { it.id })).apply { p.reply?.let { put("reply", it.id) } })
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

    /** Answers a card; a permission card also names its option, and a deny can carry a note for the agent. */
    fun decide(aid: String, approve: Boolean, option: String? = null, note: String? = null, landed: (Boolean) -> Unit = {}, onFail: () -> Unit) {
        viewModelScope.launch {
            try {
                api.post("api/approvals/$aid", JSONObject().put("decision", if (approve) "approve" else "deny")
                    .apply { option?.let { put("option", it) }; if (!note.isNullOrEmpty()) put("note", note) })
                landed(true)
            } catch (e: IOException) {
                // 409: answered elsewhere, the event stream settles the card.
                if (e is HttpError && e.code == 409) return@launch
                landed(false)
                onFail()
                undo = { decide(aid, approve, option, note, landed, onFail) }
                snackAction = "Retry"
                snack = "Couldn't send your answer"
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
                    chat.local("Couldn't read that image")
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
                    chat.local("Couldn't upload $name" + if (why.isNullOrEmpty()) "" else ": $why")
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
        if (phase == Phase.Ready && (api.token != null || api.pageSession)) applyShare(s) else pendingShare = s
    }

    private fun applyShare(s: Shared) {
        if (s.text.isNotBlank()) edit(listOf(draft.trimEnd(), s.text.trim()).filter { it.isNotEmpty() }.joinToString("\n"))
        if (s.uris.isNotEmpty()) attachFiles(s.uris)
    }

    // ---------- app updates ----------

    /** A newer release on the server, or null. */
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

    // ---------- files from the server ----------

    /** server file id -> its copy in Downloads. */
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
                    chat.local(if ((e as? HttpError)?.code == 404) "${f.name} has expired on the server" else "Couldn't download ${f.name}")
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
            chat.local("No app on this phone opens ${f.name}. It's in Downloads/seimei.")
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

    /** A picture from the server, fetched with the login token: the chat-sized thumbnail, kept in a small cache.
     * Or the original for the full-screen view, not cached since it can be big. */
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
                    chat.local("Couldn't save the picture")
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

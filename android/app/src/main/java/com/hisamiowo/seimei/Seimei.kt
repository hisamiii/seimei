package com.hisamiowo.seimei

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

sealed interface Attachment {
    class Photo(val data: String, val preview: ImageBitmap) : Attachment

    /** Any other file: it goes up to the laptop as soon as it is picked, and the message names it by id. */
    class Doc(val name: String) : Attachment {
        var id by mutableStateOf<String?>(null)
        var uploading by mutableStateOf(true)
        var job: Job? = null
    }
}

/** The bottom sheet over the tabs: one shikigami with its log, or one of the user's own sessions. */
sealed interface Sheet {
    data class Worker(val info: JSONObject, val task: String, val items: List<Item>) : Sheet
    data class Session(val pid: Long, val data: JSONObject, val sig: String) : Sheet
}

/** What the Screen tab got back: a frame, or why there is none. */
sealed interface Frame {
    data class Image(val bmp: ImageBitmap) : Frame
    data class Problem(val text: String) : Frame
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
    var live by mutableStateOf("")
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
        private set
    /** Every shikigami, finished ones from days ago too; fetched while the Agents tab shows. */
    var allWorkers by mutableStateOf<List<JSONObject>?>(null)
        private set
    var sheet by mutableStateOf<Sheet?>(null)
        private set
    var seen by mutableStateOf(JSONObject(prefs.seen))
        private set
    var visible by mutableStateOf(false)
        private set
    var tab by mutableStateOf("chat")
        private set
    var unread by mutableStateOf(false)

    var draft by mutableStateOf(prefs.draft)
        private set
    var attachments by mutableStateOf<List<Attachment>>(emptyList())
        private set

    private var events = ArrayList<JSONObject>()
    private var chat = Chat()
    private var lastId = 0L
    private var firstId = 0L
    private var stream: Job? = null
    private var foreground = false
    private var backgroundSince = 0L
    private val images = LruCache<String, ImageBitmap>(48)
    private var sheetChat = Chat()
    private var sheetLast = 0L
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
        rebuild()
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
        chat = Chat()
        events.forEach(chat::add)
        publish()
    }

    private fun publish() {
        items = chat.items.toList()
        waiting = chat.waiting
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
            return
        }
        val type = ev.str("type")
        if (ev.has("id")) {
            val id = ev.getLong("id")
            if (id <= lastId) return
            lastId = id
            if (type == "assistant") live = ""
            events.add(ev)
            chat.add(ev)
            publish()
            if (tab != "chat" && type in setOf("assistant", "approval", "worker", "image", "file")) unread = true
            return
        }
        when (type) {
            "state" -> setState(ev)
            "delta" -> live += ev.str("text")
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
            "chat" -> unread = false
            "agents" -> refreshAll(force = true)
            "usage" -> refreshCost()
        }
    }

    // ---------- shikigami and sessions ----------

    private fun takeWorkers(list: List<JSONObject>) {
        workers = list
        (sheet as? Sheet.Worker)?.let { sh ->
            list.firstOrNull { it.str("id") == sh.info.str("id") }?.let { w ->
                val info = JSONObject(sh.info.toString())
                w.keys().forEach { info.put(it, w.get(it)) }
                sheet = sh.copy(info = info)
            }
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
        (sheet as? Sheet.Session)?.let { sh ->
            list.firstOrNull { it.optLong("pid") == sh.pid }?.let { if (sessionSig(it) != sh.sig) openSession(sh.pid) }
        }
    }

    private fun saveSeen(s: JSONObject) {
        seen = s
        prefs.seen = s.toString()
    }

    private fun sessionSig(s: JSONObject) = s.str("status") + s.opt("question") + s.optDouble("active", 0.0)

    fun openWorker(id: String) {
        viewModelScope.launch {
            try {
                val r = api.get("api/workers/$id")
                val evs = r.getJSONArray("events").objects()
                sheetChat = Chat()
                evs.forEach(sheetChat::add)
                sheetLast = evs.lastOrNull()?.optLong("id") ?: 0
                val w = r.getJSONObject("worker")
                sheet = Sheet.Worker(w, w.str("task"), sheetChat.items.toList())
            } catch (e: IOException) {
            }
        }
    }

    private fun workerEvent(ev: JSONObject) {
        val sh = sheet as? Sheet.Worker ?: return
        if (ev.str("worker") != sh.info.str("id")) return
        val e = ev.optJSONObject("ev") ?: return
        // The sheet loaded the log a moment ago; skip what it already holds.
        if (e.has("id") && e.getLong("id") <= sheetLast) return
        if (e.has("id")) sheetLast = e.getLong("id")
        sheetChat.add(e)
        sheet = sh.copy(items = sheetChat.items.toList())
    }

    fun openSession(pid: Long) {
        viewModelScope.launch {
            try {
                val r = api.get("api/sessions/$pid")
                val s = r.getJSONObject("session")
                val id = s.str("session_id")
                if (id.isNotEmpty()) saveSeen(JSONObject(seen.toString()).put(id, s.optDouble("active", 0.0)))
                sheet = Sheet.Session(pid, r, sessionSig(s))
            } catch (e: IOException) {
            }
        }
    }

    fun closeSheet() {
        sheet = null
    }

    fun workerSend(id: String, text: String, onFail: () -> Unit) {
        viewModelScope.launch {
            try {
                api.post("api/workers/$id/send", JSONObject().put("text", text))
            } catch (e: IOException) {
                onFail()
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

    // ---------- usage and the screen ----------

    fun refreshCost() {
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

    suspend fun desktop(): JSONObject? = try {
        api.get("api/desktop")
    } catch (e: IOException) {
        null
    }

    suspend fun frame(query: String): Frame = try {
        val bytes = api.bytes("api/screen?$query")
        Frame.Image(withContext(Dispatchers.Default) { ImageDecoder.decodeBitmap(ImageDecoder.createSource(bytes)).asImageBitmap() })
    } catch (e: HttpError) {
        val body = try { JSONObject(e.body) } catch (x: JSONException) { JSONObject() }
        Frame.Problem(when {
            body.str("paused") == "bench" -> "Paused while a benchmark runs"
            body.optBoolean("down") -> "The desktop is not running"
            else -> body.str("error").ifEmpty { "Could not reach the laptop" }
        })
    } catch (e: IOException) {
        Frame.Problem("Could not reach the laptop")
    } catch (e: Exception) {
        Frame.Problem("Could not read the picture")
    }

    // ---------- actions ----------

    fun edit(text: String) {
        draft = text
        prefs.draft = text
    }

    fun send() {
        val text = draft.trim()
        val sending = attachments
        if ((text.isEmpty() && sending.isEmpty()) || !online) return
        edit("")
        attachments = emptyList()
        viewModelScope.launch {
            val docs = sending.filterIsInstance<Attachment.Doc>()
            try {
                docs.forEach { it.job?.join() }
                if (docs.any { it.id == null }) throw IOException("upload failed")
                val imgs = JSONArray()
                sending.filterIsInstance<Attachment.Photo>().forEach { imgs.put(JSONObject().put("data", it.data)) }
                api.post("api/send", JSONObject().put("text", text).put("images", imgs).put("files", JSONArray(docs.map { it.id })))
            } catch (e: IOException) {
                if (draft.isEmpty()) edit(text)
                // A file whose upload failed is gone; the rest wait for another try.
                attachments = sending.filter { it !is Attachment.Doc || it.id != null } + attachments
                chat.local("Could not send that. Try again.")
                publish()
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
                    doc.id = api.upload(name) { cr.openInputStream(uri) ?: throw IOException("cannot read $name") }.str("id")
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

    /** A picture from the laptop, fetched with the login token and kept in a small cache. */
    suspend fun image(name: String): ImageBitmap? {
        images.get(name)?.let { return it }
        return try {
            val bytes = api.bytes("api/files/$name")
            val bmp = withContext(Dispatchers.Default) {
                ImageDecoder.decodeBitmap(ImageDecoder.createSource(bytes)) { dec, info, _ ->
                    val side = maxOf(info.size.width, info.size.height)
                    if (side > 2048) {
                        val f = 2048f / side
                        dec.setTargetSize((info.size.width * f).toInt(), (info.size.height * f).toInt())
                    }
                }.asImageBitmap()
            }
            images.put(name, bmp)
            bmp
        } catch (e: Exception) {
            null
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

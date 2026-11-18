package com.hisamiowo.seimei

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioAttributes
import android.net.ConnectivityManager
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * One voice call with the partner (docs/voice.md): the microphone goes up the socket as 16 kHz PCM16 without pause, the partner's audio comes down to an AudioTrack.
 * The microphone runs as a voice call, with the platform's echo canceller and noise suppressor; [Uplink] cuts the partner off when the user talks over it.
 * The audio runs on threads of its own, so the call goes on with the screen off; its state is Compose state for the call bar and the notification.
 */
class VoiceCall(private val ctx: Context, private val api: Api, val speed: Double?, echo: Double, private val host: Host, soft: Boolean = false) {
    interface Host {
        fun tell(text: String)
        /** The call is over; [why] is empty when the user or the partner ended it. */
        fun ended(call: VoiceCall, why: String)
    }

    /** The bridge's state: connecting, listening, speaking, reconnecting, ended. */
    var state by mutableStateOf("connecting")
        private set
    var muted by mutableStateOf(false)
        private set
    /** The last levels, 0 to 1, newest last: the user's voice, or the partner's while it plays. */
    var levels by mutableStateOf(List(BARS) { 0f })
        private set
    var seconds by mutableStateOf(0)
        private set
    /** When the call started, in unix seconds: a chat card with a turn since then is this call's. */
    val since = System.currentTimeMillis() / 1000.0
    /** The user talks now. */
    var talking by mutableStateOf(false)
        private set
    /** The partner's audio plays now. */
    var playing by mutableStateOf(false)
        private set
    /** The partner's current line as it speaks; its finished turn in the chat replaces it ([clearSaying]). */
    var saying by mutableStateOf("")
        private set
    /** What the call bar's pill and the notification say. */
    val label get() = if (muted) "Muted" else LABELS[state] ?: ""
    /** The echo this call learned, for the next one (Barge.learned). */
    val echo get() = up.barge.echo

    private val main = Handler(Looper.getMainLooper())
    private val am = ctx.getSystemService(AudioManager::class.java)
    private val lock = Any()
    private val log = Playlog()
    private val jitter = Jitter()
    // Each chunk carries the cut it belongs to: a chunk from before a cut never plays.
    private val queue = LinkedBlockingQueue<Chunk>()
    // Around the track's writes and its flush, so a cut drops everything at once and the count of written frames stays true.
    private val trackLock = Any()
    // Frames written since the last flush: less the play head, what the track still holds. The partner's audio ends at partnerEnd, silence follows.
    private var written = 0L
    private var partnerEnd = 0L
    // How many frames go into the track ahead of its play head: [AHEAD_MS], or the track's start threshold when that is more.
    private var ahead = Voice.RATE * AHEAD_MS / 1000
    // For the stats: the partner's audio actually written to the track (frames, summed power), and the play head before the last flush.
    private var wrote = 0L
    private var wrotePower = 0.0
    private var headBase = 0L
    @Volatile private var headTotal = 0L
    // Where the call's sound goes: the communication device's type and whether the platform took it.
    @Volatile private var route = ""
    @Volatile private var gen = 0
    @Volatile private var running = true
    @Volatile private var open = false
    private val over = AtomicBoolean(false)
    private var ws: WebSocket? = null
    private var rec: AudioRecord? = null
    private var track: AudioTrack? = null
    private val effects = ArrayList<AudioEffect>()
    private var focus: AudioFocusRequest? = null
    private var mode = AudioManager.MODE_NORMAL
    // The partner's last line is done: its next words start a new one.
    @Volatile private var lineDone = false
    @Volatile private var heard = 0f
    // For the stats in the server log.
    @Volatile private var got = 0
    @Volatile private var played = 0.0
    @Volatile private var cutS = 0.0
    @Volatile private var flushes = 0
    // The echo canceller and noise suppressor: on, off (made but not enabled), none (this phone gives apps none), or the error that made it fail.
    private var aec = "none"
    private var ns = "none"
    // What the platform says it has: the effects it lists for apps, and whether it offers an echo canceller.
    private val offered = try {
        AudioEffect.queryEffects()?.map { it.name } ?: emptyList()
    } catch (e: Exception) {
        emptyList()
    }
    private val aecOffered = try {
        AcousticEchoCanceler.isAvailable()
    } catch (e: Exception) {
        false
    }
    // The effects the platform runs on the microphone's stream, read once it records.
    @Volatile private var fx = emptyList<String>()
    // The app's own echo canceller, or null when this build has none: then the platform's runs, where the phone has one.
    private val canceller = Aec.make()
    // The microphone's power before and after the app's canceller while the partner plays, for the stats: the echo it removes.
    private var echoIn = 0.0
    // A watch's speaker: the partner's audio goes through a [Shelf] as it comes in, so the canceller's reference is what plays.
    private val shelf = if (soft) Shelf() else null
    // The user's voice at the microphone, after the canceller: its summed power and frames, for the stats.
    @Volatile private var voiceIn = 0.0
    @Volatile private var voiceFrames = 0
    private var echoOut = 0.0
    private var echoFrames = 0
    // What the track holds ahead of its play head, in ms: the canceller's delay hint.
    @Volatile private var aheadMs = 0
    // The two audio threads; the last one to end frees the canceller.
    private val alive = java.util.concurrent.atomic.AtomicInteger(2)

    private class Chunk(val gen: Int, val samples: ShortArray, val db: Double)

    private val up = Uplink(echo, object : Uplink.Out {
        override fun level() = synchronized(lock) { log.level(now()) }
        override fun busy() = synchronized(lock) { log.busy(now()) }
        override fun barge() {
            cut()
            text(Voice.up("barge"))
        }
        override fun send(f: ShortArray) {
            if (open) ws?.send(Voice.bytes(f).toByteString())
        }
    })

    private fun now() = System.nanoTime() / 1e9

    /** Opens the microphone, the speaker and the socket; false when the microphone would not open. */
    @SuppressLint("MissingPermission") // the voice button asks for it first
    fun start(): Boolean {
        // The call mode first: the platform picks the microphone's voice processing (its echo canceller) when the input opens.
        route()
        val min = AudioRecord.getMinBufferSize(Voice.RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val r = try {
            AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, Voice.RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(min, Voice.FRAME * 2 * 16))
        } catch (e: Exception) {
            return false.also { unroute() }
        }
        if (r.state != AudioRecord.STATE_INITIALIZED) return false.also { r.release(); unroute() }
        rec = r
        // isAvailable() is false on phones that run their canceller below the app; create() says for itself.
        // With the app's own canceller, the platform's are made only to switch them off: two cancellers would fight.
        val on = canceller == null
        aec = attach(r.audioSessionId, on) { AcousticEchoCanceler.create(it) }
        ns = attach(r.audioSessionId, on) { NoiseSuppressor.create(it) }
        // With an echo canceller, the partner's voice does not come back: the user can talk over it, and the mic goes up live.
        up.gate = canceller == null && aec != "on"
        val outMin = AudioTrack.getMinBufferSize(Voice.RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        // In the microphone's session, so an echo canceller that works per session hears what plays.
        // Room for what goes ahead of the play head ([AHEAD_MS]): a burst after a late stretch waits in the jitter buffer.
        track = try {
            AudioTrack.Builder().setAudioAttributes(ATTRS)
                .setAudioFormat(AudioFormat.Builder().setSampleRate(Voice.RATE).setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                .setTransferMode(AudioTrack.MODE_STREAM).setBufferSizeInBytes(maxOf(outMin, Voice.RATE * 2 * TRACK_MS / 1000))
                .setSessionId(r.audioSessionId).build().also(::threshold)
        } catch (e: Exception) {
            for (fx in effects) fx.release()
            r.release()
            unroute()
            return false
        }
        ws = api.socket(Voice.path(speed), listener)
        Thread(::record, "voice-mic").start()
        Thread(::playback, "voice-out").start()
        return true
    }

    /**
     * A streaming track starts to play only once its buffer holds the start threshold, by default the whole buffer.
     * This one holds only [AHEAD_MS] (plus silence), so it would never start: the partner plays from one 20 ms frame on.
     */
    private fun threshold(t: AudioTrack) {
        val n = if (Build.VERSION.SDK_INT >= 31) t.setStartThresholdInFrames(Voice.FRAME)
        else t.setBufferSizeInFrames(Voice.RATE * AHEAD_MS / 1000)
        if (n > 0) ahead = maxOf(ahead, n)
    }

    fun hangUp() = finish("")

    fun toggleMute() {
        muted = !muted
        up.muted = muted
    }

    fun clearSaying() {
        saying = ""
        lineDone = false
    }

    /** Talks as a phone call: the voice-call mode the echo canceller works in, on the loudspeaker unless a headset takes it. */
    @Suppress("DEPRECATION")
    private fun route() {
        mode = am.mode
        am.mode = AudioManager.MODE_IN_COMMUNICATION
        focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT).setAudioAttributes(ATTRS).build().also(am::requestAudioFocus)
        if (Build.VERSION.SDK_INT >= 31) {
            // A headset when one is connected, else the loudspeaker; never the earpiece the call mode picks on its own.
            val all = am.availableCommunicationDevices
            val d = all.firstOrNull { it.type in HEADSETS } ?: all.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
            route = if (d == null) "none" else "${d.type}:${am.setCommunicationDevice(d)}"
        } else if (!am.isWiredHeadsetOn && !am.isBluetoothScoOn) {
            am.isSpeakerphoneOn = true
            route = "speakerphone"
        }
    }

    @Suppress("DEPRECATION")
    private fun unroute() {
        if (Build.VERSION.SDK_INT >= 31) am.clearCommunicationDevice() else am.isSpeakerphoneOn = false
        focus?.let(am::abandonAudioFocusRequest)
        am.mode = mode
    }

    /** Makes an effect on the microphone's session and enables it, or disables it: "on", "off", "none" when the phone has none for apps, or why it failed. */
    private fun attach(session: Int, enable: Boolean, make: (Int) -> AudioEffect?): String {
        val e = try {
            make(session)
        } catch (e: Exception) {
            return "error: ${e.javaClass.simpleName} ${e.message ?: ""}".trim()
        } ?: return "none"
        effects += e
        return try {
            e.enabled = enable
            if (e.enabled) "on" else "off"
        } catch (x: IllegalStateException) {
            "off"
        }
    }

    private fun record() {
        val r = rec ?: return
        var frames = 0
        try {
            r.startRecording()
            // What the platform itself runs on this input: on many phones the voice call's echo canceller lives here, not in [effects].
            fx = try {
                r.activeRecordingConfiguration?.let { c -> (c.effects + c.clientEffects).map { it.name }.distinct() } ?: emptyList()
            } catch (e: Exception) {
                emptyList()
            }
            while (running) {
                val buf = ShortArray(Voice.FRAME)
                var n = 0
                while (n < buf.size && running) {
                    val k = r.read(buf, n, buf.size - n)
                    if (k <= 0) break
                    n += k
                }
                if (!running) break
                if (n < buf.size) {
                    finish("The microphone stopped")
                    break
                }
                frames++
                canceller?.let { clean(it, buf) }
                up.push(buf)
                if (!muted && up.barge.talking()) {
                    voiceIn += power(buf)
                    voiceFrames++
                }
                // The bars move every 80 ms, as on the page.
                if (frames % 4 == 0) tick(frames)
                if (frames % (STATS_S * 50) == 0) stats()
            }
        } finally {
            try { r.stop() } catch (e: IllegalStateException) {}
            for (e in effects) e.release()
            r.release()
            if (alive.decrementAndGet() == 0) canceller?.release()
        }
    }

    /** The app's canceller on a microphone frame; while the partner plays, its power before and after go to the stats. */
    private fun clean(c: Aec, f: ShortArray) {
        val before = power(f)
        // The track's lead, the microphone's frame and the output's period.
        c.near(f, aheadMs + 40)
        if (synchronized(lock) { log.playing(now()) }) {
            echoIn += before
            echoOut += power(f)
            echoFrames++
        }
    }

    private fun power(f: ShortArray): Double {
        var s = 0.0
        for (x in f) s += x.toDouble() * x
        return s / f.size / (32768.0 * 32768.0)
    }

    private fun tick(frames: Int) {
        val p = synchronized(lock) { log.playing(now()) }
        val t = !muted && up.barge.talking()
        val l = if (muted) 0f else if (p && !t) heard else up.mic
        levels = levels.drop(1) + l
        seconds = frames / 50
        if (t != talking) talking = t
        if (p != playing) playing = p
    }

    /**
     * The partner's audio into the track through the jitter buffer: a reply waits until its prebuffer is queued, then plays.
     * When the partner's audio runs out, the next audio waits again. The track plays silence meanwhile ([fill]).
     */
    private fun playback() {
        val t = track ?: return
        val pend = ArrayDeque<Chunk>()
        var g = gen
        var off = 0
        var waiting = true
        try {
            synchronized(trackLock) { t.play() }
            while (running) {
                if (g != gen) {
                    // A cut: what waits here belongs to the old reply; cut() flushed the track.
                    g = gen
                    pend.clear()
                    off = 0
                    waiting = true
                }
                var c = if (pend.isEmpty() || waiting) queue.poll(5, TimeUnit.MILLISECONDS) else queue.poll()
                while (c != null) {
                    if (c.gen == g) pend.addLast(c)
                    c = queue.poll()
                }
                val now = now()
                if (waiting) {
                    val queued = (pend.sumOf { it.samples.size } - off).toDouble() / Voice.RATE
                    if (!synchronized(lock) { jitter.ready(queued, now) }) {
                        fill(t, g)
                        continue
                    }
                    waiting = false
                }
                // Only [AHEAD_MS] ahead of the play head, without blocking: the rest waits here, so what goes to the canceller plays soon after.
                var full = false
                while (pend.isNotEmpty()) {
                    val head = pend.first()
                    val n = synchronized(trackLock) {
                        val room = (ahead - (written - head(t))).toInt()
                        if (g != gen) -1
                        else if (room <= 0) 0
                        else t.write(head.samples, off, minOf(room, head.samples.size - off), AudioTrack.WRITE_NON_BLOCKING).also {
                            if (it > 0) {
                                written += it
                                partnerEnd = written
                                wrote += it
                                for (i in off until off + it) wrotePower += head.samples[i].toDouble() * head.samples[i]
                                canceller?.far(head.samples, off, it)
                            }
                        }
                    }
                    if (n < 0) break
                    if (n == 0) {
                        full = true
                        break
                    }
                    // The echo check and the mic gate follow what is in the track, as it will play.
                    synchronized(lock) { log.push(n.toDouble() / Voice.RATE, head.db, now) }
                    off += n
                    if (off == head.samples.size) {
                        pend.removeFirst()
                        off = 0
                    }
                }
                synchronized(trackLock) {
                    val h = head(t)
                    aheadMs = ((written - h) * 1000 / Voice.RATE).toInt()
                    headTotal = headBase + h
                }
                if (full) {
                    Thread.sleep(5)
                } else if (pend.isEmpty() && g == gen) {
                    val dry = synchronized(trackLock) { head(t) >= partnerEnd }
                    if (dry) {
                        synchronized(lock) { jitter.dried(now) }
                        waiting = true
                    }
                    fill(t, g)
                }
            }
        } catch (e: InterruptedException) {
        } finally {
            try { t.stop() } catch (e: IllegalStateException) {}
            t.release()
            if (alive.decrementAndGet() == 0) canceller?.release()
        }
    }

    private fun head(t: AudioTrack) = t.playbackHeadPosition.toLong() and 0xffffffffL

    /**
     * Keeps [SILENCE_MS] of silence queued while no partner audio is: the track never runs dry and never pauses.
     * A paused voice-call output goes to standby, and the phone's echo canceller then loses what plays as its reference.
     */
    private fun fill(t: AudioTrack, g: Int) {
        synchronized(trackLock) {
            if (g != gen) return
            while (written - head(t) < Voice.RATE * SILENCE_MS / 1000) {
                val n = t.write(SILENCE, 0, SILENCE.size, AudioTrack.WRITE_NON_BLOCKING)
                if (n <= 0) return
                written += n
                canceller?.far(SILENCE, 0, n)
            }
        }
    }

    /** Everything queued to play stops now: the barge's cut does not wait on the playback thread. */
    private fun cut() {
        synchronized(lock) {
            cutS += log.cut(now())
            jitter.cut()
        }
        gen++
        queue.clear()
        if (!running) return
        try {
            synchronized(trackLock) {
                track?.run {
                    headBase += head(this)
                    pause()
                    flush()
                    play()
                }
                written = 0
                partnerEnd = 0
            }
        } catch (e: IllegalStateException) {
        }
    }

    private fun text(s: String) {
        if (open) ws?.send(s)
    }

    /** The app's side of the call, for the server log, as the page sends it: what played, what cut it, and the echo. */
    private fun stats() {
        text(JSONObject().put("type", "stats").put("path", "audiotrack").put("got", got)
            .put("played", round1(played)).put("barges", up.barges).put("cut", round1(cutS)).put("flushes", flushes)
            .put("gated", round1(up.gated * Voice.FRAME.toDouble() / Voice.RATE)).put("echo", round1(echo)).put("muted", muted)
            .put("canceller", if (canceller != null) "webrtc" else if (aec == "on") "platform" else "none").put("echoIn", echoDb(echoIn))
            .put("echoOut", echoDb(echoOut)).put("aec3", canceller?.stats()?.let { JSONArray(it.map { v -> if (v.isNaN()) null else round1(v) }) })
            .put("aec", aec).put("aecOffered", aecOffered).put("ns", ns).put("fx", JSONArray(fx))
            .put("wrote", round1(wrote.toDouble() / Voice.RATE)).put("outDb", if (wrote == 0L) null else round1(10 * Math.log10(maxOf(wrotePower / wrote / (32768.0 * 32768.0), 1e-10))))
            .put("head", round1(headTotal.toDouble() / Voice.RATE)).put("route", route)
            .put("comm", if (Build.VERSION.SDK_INT >= 31) am.communicationDevice?.type ?: -1 else -1)
            .put("session", track?.audioSessionId == rec?.audioSessionId).put("gate", up.gate).put("callMode", am.mode == AudioManager.MODE_IN_COMMUNICATION)
            .put("mic", rec?.routedDevice?.type ?: -1).put("speaker", track?.routedDevice?.type ?: -1).put("phone", Build.MODEL).put("net", net()).put("soft", shelf != null)
            .put("voiceDb", if (voiceFrames == 0) null else round1(10 * Math.log10(maxOf(voiceIn / voiceFrames, 1e-10))))
            .put("underruns", track?.underrunCount ?: -1).put("jitter", synchronized(lock) { jitter.run {
                JSONObject().put("stalls", stalls).put("lateMs", Math.round(lateMax * 1000)).put("prebufferMs", Math.round(target * 1000))
            } })
            // Last: the server log keeps a stats line's first 600 characters, and this list is long.
            .put("effects", JSONArray(offered)).toString())
    }

    /** The transport the call's network runs on now: bluetooth for a watch through its phone, wifi or cellular. */
    private fun net() = try {
        ctx.getSystemService(ConnectivityManager::class.java).run { transport(getNetworkCapabilities(activeNetwork)) }
    } catch (e: SecurityException) {
        "unknown"
    }

    private fun round1(v: Double) = Math.round(v * 10) / 10.0

    /** The mean power of the microphone's frames while the partner played, in dBFS, or null before any. */
    private fun echoDb(sum: Double) = if (echoFrames == 0) null else round1(10 * Math.log10(maxOf(sum / echoFrames, 1e-10)))

    private fun finish(why: String) {
        if (!over.compareAndSet(false, true)) return
        stats()
        text(Voice.up("end"))
        ws?.close(1000, null)
        open = false
        running = false
        unroute()
        main.post {
            saying = ""
            state = "ended"
            host.ended(this, why)
        }
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            open = running
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            if (!running) return
            val s = Voice.samples(bytes.toByteArray())
            if (s.isEmpty()) return
            shelf?.run(s)
            val secs = s.size.toDouble() / Voice.RATE
            synchronized(lock) { jitter.arrived(secs, now()) }
            heard = Voice.level(s)
            got++
            played += secs
            queue.add(Chunk(gen, s, frameDb(s)))
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            when (val m = VoiceMsg.parse(text)) {
                is VoiceMsg.State -> {
                    state = m.state
                    if (m.state == "ended") finish("")
                }
                VoiceMsg.Flush -> {
                    flushes++
                    cut()
                }
                is VoiceMsg.Saying -> {
                    if (lineDone) clearSaying()
                    saying += m.text
                }
                is VoiceMsg.Said -> if (m.who == "partner") lineDone = true
                is VoiceMsg.Error -> main.post { host.tell(m.text) }
                null -> {}
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            finish(if (state == "connecting") "Couldn't start the call" else "")
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            finish(if (state == "connecting") "Couldn't start the call" else "")
        }
    }

    companion object {
        const val BARS = 26
        private val LABELS = mapOf("connecting" to "Connecting…", "listening" to "Listening", "speaking" to "Speaking", "reconnecting" to "Reconnecting…")
        /** The app's numbers go to the server log this often, and at hang-up. */
        const val STATS_S = 10
        /** The track's own buffer: room for what goes ahead, and some. */
        const val TRACK_MS = 120
        /** How far the partner's audio goes into the track ahead of its play head; the rest waits in the jitter buffer. */
        const val AHEAD_MS = 60
        /** The silence kept queued while the partner is quiet: two of the output's 20 ms periods. */
        const val SILENCE_MS = 40
        private val SILENCE = ShortArray(Voice.FRAME)
        private val HEADSETS = setOf(AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_HEARING_AID)
        private val ATTRS = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
    }
}

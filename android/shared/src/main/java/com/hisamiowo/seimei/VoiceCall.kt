package com.hisamiowo.seimei

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioAttributes
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
import org.json.JSONObject
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * One voice call with the partner (docs/voice.md): the microphone goes up the socket as 16 kHz PCM16 without pause, the partner's audio comes down to an AudioTrack.
 * The microphone runs as a voice call, with the platform's echo canceller and noise suppressor; [Uplink] cuts the partner off when the user talks over it.
 * The audio runs on threads of its own, so the call goes on with the screen off; its state is Compose state for the call bar and the notification.
 */
class VoiceCall(private val ctx: Context, private val api: Api, val speed: Double?, echo: Double, private val host: Host) {
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
    // Each chunk carries the cut it belongs to: a chunk from before a cut never plays.
    private val queue = LinkedBlockingQueue<Pair<Int, ShortArray>>()
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
        val min = AudioRecord.getMinBufferSize(Voice.RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val r = try {
            AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, Voice.RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(min, Voice.FRAME * 2 * 16))
        } catch (e: Exception) {
            return false
        }
        if (r.state != AudioRecord.STATE_INITIALIZED) return false.also { r.release() }
        rec = r
        if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(r.audioSessionId)?.let { it.enabled = true; effects += it }
        if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(r.audioSessionId)?.let { it.enabled = true; effects += it }
        val outMin = AudioTrack.getMinBufferSize(Voice.RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        // In the microphone's session, so an echo canceller that works per session hears what plays.
        track = try {
            AudioTrack.Builder().setAudioAttributes(ATTRS)
                .setAudioFormat(AudioFormat.Builder().setSampleRate(Voice.RATE).setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                .setTransferMode(AudioTrack.MODE_STREAM).setBufferSizeInBytes(maxOf(outMin, Voice.RATE * 2 / 10))
                .setSessionId(r.audioSessionId).build()
        } catch (e: Exception) {
            for (fx in effects) fx.release()
            r.release()
            return false
        }
        route()
        ws = api.socket(Voice.path(speed), listener)
        Thread(::record, "voice-mic").start()
        Thread(::playback, "voice-out").start()
        return true
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
            val cur = am.communicationDevice
            if (cur == null || cur.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE)
                am.availableCommunicationDevices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }?.let(am::setCommunicationDevice)
        } else if (!am.isWiredHeadsetOn && !am.isBluetoothScoOn) am.isSpeakerphoneOn = true
    }

    @Suppress("DEPRECATION")
    private fun unroute() {
        if (Build.VERSION.SDK_INT >= 31) am.clearCommunicationDevice() else am.isSpeakerphoneOn = false
        focus?.let(am::abandonAudioFocusRequest)
        am.mode = mode
    }

    private fun record() {
        val r = rec ?: return
        var frames = 0
        try {
            r.startRecording()
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
                up.push(buf)
                // The bars move every 80 ms, as on the page.
                if (frames % 4 == 0) tick(frames)
                if (frames % (STATS_S * 50) == 0) stats()
            }
        } finally {
            try { r.stop() } catch (e: IllegalStateException) {}
            for (e in effects) e.release()
            r.release()
        }
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

    private fun playback() {
        val t = track ?: return
        try {
            t.play()
            while (running) {
                val (g, chunk) = queue.poll(100, TimeUnit.MILLISECONDS) ?: continue
                // In slices, so a cut stops a long chunk within 20 ms.
                var i = 0
                while (i < chunk.size && running && g == gen) {
                    val n = t.write(chunk, i, minOf(Voice.FRAME, chunk.size - i))
                    if (n <= 0) break
                    i += n
                }
            }
        } finally {
            try { t.stop() } catch (e: IllegalStateException) {}
            t.release()
        }
    }

    /** Everything queued to play stops now. */
    private fun cut() {
        synchronized(lock) { cutS += log.cut(now()) }
        gen++
        queue.clear()
        if (!running) return
        try {
            track?.run {
                pause()
                flush()
                play()
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
            .put("aec", effects.any { it is AcousticEchoCanceler }).put("ns", effects.any { it is NoiseSuppressor }).toString())
    }

    private fun round1(v: Double) = Math.round(v * 10) / 10.0

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
            val secs = s.size.toDouble() / Voice.RATE
            synchronized(lock) { log.push(secs, frameDb(s), now()) }
            heard = Voice.level(s)
            got++
            played += secs
            queue.add(gen to s)
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
        private val ATTRS = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
    }
}

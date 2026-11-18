package com.hisamiowo.seimei

import org.json.JSONException
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.max

// Voice calls without the audio devices (docs/voice.md): the socket's messages and frames, the local barge-in and the mic gate.
// VoiceCall feeds them; the unit tests run them on plain arrays, as web/pcm.js runs in node.

object Voice {
    const val RATE = 16000
    /** 20 ms at 16 kHz: one frame up the socket. */
    const val FRAME = 320
    /** How fast the partner talks, kept on the device and sent as `?speed=`; the bridge sets it when a call starts only. */
    val SPEEDS = listOf(0.9, 1.0, 1.1, 1.2, 1.3)
    /** What played this long ago can still come back through the microphone: the output's latency plus the room. */
    const val ECHO_S = 0.3
    /** The mic goes up as silence while the partner plays and this long after, unless the user barges in. */
    const val GATE_TAIL = 0.3
    /** On a barge the partner gets the microphone's last this many frames too (300 ms): the words that set it off. */
    const val GATE_KEEP = 15

    fun nextSpeed(v: Double?) = SPEEDS[(SPEEDS.indexOf(v ?: 1.0) + 1) % SPEEDS.size]

    fun speedText(v: Double?) = "%.1f×".format(java.util.Locale.ROOT, v ?: 1.0)

    /** The socket's path, with the device's speed when it has one. */
    fun path(speed: Double?) = "api/voice/ws" + if (speed != null && speed in SPEEDS) "?speed=$speed" else ""

    /** A text frame up: {"type": "barge"}, {"type": "end"}. */
    fun up(type: String) = JSONObject().put("type", type).toString()

    /** Samples as the socket's binary frame: PCM16 little-endian. */
    fun bytes(f: ShortArray): ByteArray {
        val b = ByteBuffer.allocate(f.size * 2).order(ByteOrder.LITTLE_ENDIAN)
        for (x in f) b.putShort(x)
        return b.array()
    }

    fun samples(b: ByteArray): ShortArray {
        val s = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        return ShortArray(s.remaining()).also { s.get(it) }
    }

    /** A frame's level as 0..1 for the call's level bars: -60 dBFS is 0, -10 is 1. */
    fun level(f: ShortArray) = ((frameDb(f) + 60) / 50).coerceIn(0.0, 1.0).toFloat()
}

/** A text frame down from the bridge; null for one the app does not act on (asking and answered show through the chat's events). */
sealed interface VoiceMsg {
    /** connecting, listening, speaking, reconnecting or ended. */
    data class State(val state: String) : VoiceMsg
    /** Drop what is queued to play: the partner's own barge-in detection heard the user. */
    data object Flush : VoiceMsg
    /** The partner's words that go with the audio just sent. */
    data class Saying(val text: String) : VoiceMsg
    data class Said(val who: String, val text: String) : VoiceMsg
    data class Error(val text: String) : VoiceMsg

    companion object {
        fun parse(text: String): VoiceMsg? {
            val m = try {
                JSONObject(text)
            } catch (e: JSONException) {
                return null
            }
            return when (m.str("type")) {
                "state" -> State(m.str("state"))
                "flush" -> Flush
                "saying" -> Saying(m.str("text"))
                "said" -> Said(m.str("who"), m.str("text"))
                "error" -> Error(m.str("text"))
                else -> null
            }
        }
    }
}

/**
 * The user talks over the partner: 120 ms of voice, above the room's noise by 10 dB and at least -42 dBFS (web/pcm.js Barge).
 * Feed it 16 kHz frames with the level of what plays now (dBFS, or null when nothing plays); push() is true once per stretch of the user's voice over the partner.
 * The partner's own sound comes back through the microphone when the echo canceller misses it.
 * So over the partner, voice must also beat the echo: the microphone's level against what plays, learned as the call goes.
 */
class Barge(echo: Double = ECHO_START) {
    private val c = CUT
    private val vad = Vad(c)
    private var onset = 0
    private var quiet = 0
    private var fired = false
    /** The loudest microphone level over what played, in dB, held and let fall slowly. */
    var echo = echo
        private set

    fun push(f: ShortArray, playDb: Double?): Boolean {
        var v = vad.voice(f, onset >= ONSET_MS / c.frameMs)
        if (playDb != null) {
            val over = frameDb(f) - playDb
            // Louder than the partner's echo can be: the user. Anything else may be the echo.
            if (over <= echo + ECHO_DB) {
                echo = max(over, echo - ECHO_FALL_DB * c.frameMs / 1000)
                v = false
            }
        }
        onset = if (v) onset + 1 else 0
        quiet = if (v) 0 else quiet + 1
        // A pause re-arms it: the next stretch of voice can barge again.
        if (quiet * c.frameMs >= c.silenceMs) fired = false
        if (playDb == null || fired || onset * c.frameMs < ONSET_MS) return false
        fired = true
        return true
    }

    /** The user talks now. */
    fun talking() = onset * c.frameMs >= ONSET_MS

    companion object {
        val CUT = Phrases.Cut(marginDb = 10.0, minDb = -42.0)
        const val ONSET_MS = 120
        /** Over the partner, voice must be this much louder than the loudest echo heard so far. */
        const val ECHO_DB = 6.0
        /** The cautious start, used until a call has taught the phone its echo; it falls this fast while the partner plays. */
        const val ECHO_START = 10.0
        const val ECHO_FALL_DB = 15.0

        /** A little above what this phone showed last time, never above the cautious start. */
        fun learned(last: Double?) = if (last == null || last.isNaN()) ECHO_START else minOf(ECHO_START, last + 3)
    }
}

/** What was queued to play, as start, end and dBFS on a clock in seconds: the echo check and the mic gate read it. */
class Playlog {
    private val played = ArrayDeque<DoubleArray>()
    /** When what is queued ends; 0 after a cut. */
    var until = 0.0
        private set

    /** Queues [seconds] of sound at [db] right after what is queued, or now when nothing is; returns its start. */
    fun push(seconds: Double, db: Double, now: Double): Double {
        val at = max(now, until)
        until = at + seconds
        played.addLast(doubleArrayOf(at, until, db))
        return at
    }

    /** The loudest sound that played within [Voice.ECHO_S], or null when nothing did. */
    fun level(now: Double): Double? {
        while (played.isNotEmpty() && played.first()[1] < now - Voice.ECHO_S) played.removeFirst()
        var db: Double? = null
        for (p in played) if (p[0] <= now) db = max(db ?: Double.NEGATIVE_INFINITY, p[2])
        return db
    }

    fun playing(now: Double) = now < until

    /** Playing, or just done: the mic gate holds. */
    fun busy(now: Double) = now < until + Voice.GATE_TAIL

    /** Everything queued stops now; returns the seconds that will not play. */
    fun cut(now: Double): Double = max(0.0, until - now).also { until = 0.0 }
}

/**
 * The speaker's jitter buffer: a reply starts to play once [target] seconds of it are queued, so a late chunk finds sound still queued, not a dry speaker.
 * Phonic sends 20 ms chunks at about real time, so the network's jitter is all the slack there is.
 * The target follows how late chunks come in this call, from [MIN_S] to [MAX_S]; a stall, the speaker dry while the reply goes on, raises it.
 * Clock in seconds; VoiceCall holds its lock around every call.
 */
class Jitter {
    /** The prebuffer, in seconds. */
    val target get() = (late + MARGIN_S).coerceIn(MIN_S, MAX_S)
    /** How late chunks come: the worst lag behind the burst's best pace, held and let fall slowly. */
    var late = 0.0
        private set
    /** The worst lag seen in this call, for the stats. */
    var lateMax = 0.0
        private set
    /** Times the speaker ran dry and more of the same reply came within [STALL_S]. */
    var stalls = 0
        private set
    private var last = Double.NEGATIVE_INFINITY
    private var got = 0.0
    private var best = 0.0
    private var dry = Double.NaN

    /** [seconds] of the partner's audio came in at [now]. */
    fun arrived(seconds: Double, now: Double) {
        if (!dry.isNaN()) {
            if (now - dry < STALL_S) {
                stalls++
                late = max(late, target + STEP_S - MARGIN_S)
            }
            dry = Double.NaN
        }
        late = max(0.0, late - FALL * max(0.0, now - last))
        if (now - last > BURST_GAP_S) {
            // A new burst: its pace starts from this chunk.
            got = 0.0
            best = now
        }
        // Each chunk is due at the best start seen plus the audio before it; later than that is lag.
        best = minOf(best, now - got)
        val lag = now - got - best
        late = max(late, lag)
        lateMax = max(lateMax, lag)
        got += seconds
        last = now
    }

    /** The speaker waits: it may start once the prebuffer is queued, or once nothing more comes, for a reply shorter than that. */
    fun ready(queued: Double, now: Double) = queued >= target || (queued > 0 && now - last >= END_S)

    /** The speaker ran dry at [now]: what comes within [STALL_S] is a stall, later a new reply. */
    fun dried(now: Double) {
        dry = now
    }

    /** A cut: the next audio is a new reply. */
    fun cut() {
        dry = Double.NaN
        last = Double.NEGATIVE_INFINITY
    }

    companion object {
        const val MIN_S = 0.15
        const val MAX_S = 0.25
        /** Above the worst lag, for the speaker's own scheduling. */
        const val MARGIN_S = 0.05
        /** A stall raises the target this much over what it was. */
        const val STEP_S = 0.05
        /** The held lag falls this many seconds per second. */
        const val FALL = 0.02
        /** A gap this long between chunks starts a new burst. */
        const val BURST_GAP_S = 0.6
        const val STALL_S = 0.6
        /** With this long since the last chunk, a short reply plays without its full prebuffer. */
        const val END_S = 0.15
    }
}

/**
 * The microphone's way up, one 20 ms frame at a time: silence while muted, the local barge-in, then the gate.
 * The gate backs up the echo canceller: while the partner plays, silence goes up and the frames wait, in case they start a barge.
 */
class Uplink(echo: Double, private val out: Out) {
    interface Out {
        /** The level of what plays now, as [Playlog.level]. */
        fun level(): Double?
        fun busy(): Boolean
        /** The user barged in: stop playing now, and tell the bridge. */
        fun barge()
        fun send(f: ShortArray)
    }

    val barge = Barge(echo)
    @Volatile
    var muted = false
    private val held = ArrayDeque<ShortArray>()
    var barges = 0
        private set
    var gated = 0
        private set
    /** The last frame's level for the bars, 0 while muted. */
    var mic = 0f
        private set

    fun push(frame: ShortArray) {
        val quiet = muted
        val f = if (quiet) ShortArray(frame.size) else frame
        if (!quiet && barge.push(f, out.level())) {
            barges++
            out.barge()
            for (h in held) out.send(h)
            held.clear()
        }
        mic = if (quiet) 0f else Voice.level(f)
        if (!quiet && out.busy()) {
            held.addLast(f)
            if (held.size > Voice.GATE_KEEP) held.removeFirst()
            gated++
            out.send(ShortArray(f.size))
        } else {
            held.clear()
            out.send(f)
        }
    }
}

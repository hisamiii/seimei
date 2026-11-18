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
    /** The gate holds the microphone's last this many frames (300 ms), in case they start a barge. */
    const val GATE_KEEP = 15
    /** On a barge the partner gets the voice that set it off and this many frames (100 ms) before it. */
    const val GATE_PRE = 5

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
 * Feed it 16 kHz frames with how far each is above the partner's echo ([Echo.heard], null when nothing plays); push() is true once per stretch of the user's voice over the partner.
 * Over the partner, voice must beat the echo predicted for it by [MARGIN_DB], and must not follow what played: that is the echo, however loud.
 */
class Barge(private val echo: Echo) {
    private val c = CUT
    private val vad = Vad(c)
    /** Frames of voice in a row now. */
    var onset = 0
        private set
    private var quiet = 0
    private var top = 0.0
    /** This stretch of voice barged in already; a pause re-arms it. */
    var fired = false
        private set

    fun push(f: ShortArray, over: Double?): Boolean {
        var v = vad.voice(f, onset >= ONSET_MS / c.frameMs)
        if (over != null && over <= MARGIN_DB) v = false
        top = if (v) max(if (onset == 0) 0.0 else top, over ?: 0.0) else 0.0
        onset = if (v) onset + 1 else 0
        quiet = if (v) 0 else quiet + 1
        if (quiet * c.frameMs >= c.silenceMs) fired = false
        if (over == null || fired || onset * c.frameMs < ONSET_MS) return false
        if (echo.echoing(onset * c.frameMs / 1000.0)) {
            // Louder than the echo was, yet it follows what played: the echo grew, so the estimate does too.
            echo.raise(top)
            onset = 0
            return false
        }
        fired = true
        return true
    }

    /** The user talks now. */
    fun talking() = onset * c.frameMs >= ONSET_MS

    companion object {
        val CUT = Phrases.Cut(marginDb = 10.0, minDb = -42.0)
        const val ONSET_MS = 120
        /** Over the partner, voice must be this much louder than the echo predicted for it. */
        const val MARGIN_DB = 8.0
    }
}

/**
 * The partner's echo at the microphone, predicted from what played: what played, [delay] later and [gain] dB off, with a short tail for the room.
 * The delay is where the microphone's envelope best follows what played; until that is clear, the echo may come at any delay up to [MAX_S].
 * Both sides go in as 10 ms bins on one clock in seconds. It locks itself: the speaker and the microphone feed it from threads of their own.
 */
class Echo(gain: Double = START_DB) {
    private val ref = Bins()
    private val mic = Bins()
    /** The echo's level against what played, in dB: the loudest heard, held, and let fall while the partner plays. */
    @Volatile var gain = gain
        private set
    /** The echo's lag behind what played, in 10 ms bins, or -1 until the microphone clearly follows it. */
    @Volatile var lag = -1
        private set
    /** How well the microphone followed what played at that lag: the correlation of their envelopes, -1 to 1. */
    @Volatile var match = 0.0
        private set
    private var peak = -100.0
    private var last = 0L
    private var frames = 0
    private var cand = -1
    private var candN = 0

    /** The echo's delay in seconds, or null while unknown. */
    val delay get() = if (lag < 0) null else lag * BIN_S

    /** How long after a sound ends its echo can still come. */
    val reach get() = if (lag < 0) Voice.GATE_TAIL else max(Voice.GATE_TAIL, (lag + JITTER) * BIN_S + TAIL_S)

    /** [n] samples from [off] went to the speaker, the first to play at [at]. */
    @Synchronized fun played(at: Double, s: ShortArray, off: Int = 0, n: Int = s.size) {
        for (i in 0 until n) ref.add(bin(at + i.toDouble() / Voice.RATE), s[off + i])
    }

    /** What would play from [now] on will not: a cut. */
    @Synchronized fun cut(now: Double) = ref.clearFrom(bin(now))

    /** A microphone frame that ended at [t]: how many dB it is over the echo predicted for it, or null when no echo can be in it. */
    @Synchronized fun heard(t: Double, f: ShortArray): Double? {
        val start = t - f.size.toDouble() / Voice.RATE
        for (i in f.indices) mic.add(bin(start + i.toDouble() / Voice.RATE), f[i])
        val b0 = bin(start)
        val b1 = bin(start + (f.size - 1).toDouble() / Voice.RATE)
        last = b1
        if (++frames % LOCATE_EVERY == 0) locate(b0 - 1)
        var shape = 0.0
        var m = 0.0
        for (b in b0..b1) {
            shape += predict(b)
            m += mic.power(b)
        }
        val n = (b1 - b0 + 1).toDouble()
        val sdb = db(shape / n)
        val dur = f.size.toDouble() / Voice.RATE
        peak = max(sdb, peak - PEAK_FALL_DB * dur)
        if (sdb < FLOOR_DB) return null
        val g = db(m / n) - sdb
        // Learn from the partner's louder sounds only: under its quiet ones the room's noise would pass for echo.
        // Rise only slowly, within the margin, while the microphone follows what played: the user's voice must never teach it.
        if (sdb >= peak - LEARN_DB) {
            if (g < gain) gain = max(g, gain - FALL_DB * dur)
            else if (g <= gain + Barge.MARGIN_DB && lag >= 0 && following() >= RISE_R) gain = minOf(g, gain + RISE_DB * dur)
        }
        return g - gain
    }

    /**
     * The microphone's last [MATCH_S], or the last [voiced] seconds, followed what played: what it hears is the echo, however loud.
     * The voiced stretch alone catches an echo that just grew: the step in its level would break the longer match.
     */
    @Synchronized fun echoing(voiced: Double) = following(MATCH_S) >= ECHO_R || following(voiced) >= ECHO_SHORT_R

    // How well the microphone's last [span] seconds follow what played, at the lag or at any lag while it is unknown.
    private fun following(span: Double = MATCH_S): Double {
        val w = (span / BIN_S).toInt()
        val lo = if (lag < 0) 0 else max(0, lag - JITTER)
        val hi = if (lag < 0) MAX_LAG else lag + JITTER
        var best = -1.0
        for (k in lo..hi) best = max(best, corr(last, w, k))
        return best
    }

    /** An echo [db] louder than the gain: the gain rises toward it, by at most [RAISE_DB] at a time. */
    @Synchronized fun raise(db: Double) {
        gain += db.coerceIn(0.0, RAISE_DB)
    }

    // The echo predicted for bin [b], before the gain: the loudest of what played around the lag, the room's tail after it.
    private fun predict(b: Long): Double {
        val lo = if (lag < 0) 0 else max(0, lag - JITTER)
        val hi = if (lag < 0) MAX_LAG else lag + JITTER
        var p = 0.0
        for (k in lo..hi + TAIL) {
            val w = if (k <= hi) 1.0 else TAIL_W[k - hi - 1]
            p = max(p, ref.power(b - k) * w)
        }
        return p
    }

    // The lag where the microphone's last [LOCATE_S] best follows what played; kept until another clearly beats it [LOCATE_N] times in a row.
    // A single match may be chance: the user's voice over what played can follow it for a moment.
    private fun locate(end: Long) {
        val w = (LOCATE_S / BIN_S).toInt()
        // Only while the partner played through the whole span: at a reply's end or after a cut, the longer lags would win for holding more of it.
        if (minOf(ref.active(end - w + 1, end), ref.active(end - w + 1 - MAX_LAG, end - MAX_LAG)) < w * LOCATE_ACTIVE) {
            candN = 0
            cand = -1
            return
        }
        var best = -1.0
        var at = -1
        for (k in 0..MAX_LAG) {
            val r = corr(end, w, k)
            if (r > best) {
                best = r
                at = k
            }
        }
        val now = if (lag < 0) -1.0 else corr(end, w, lag)
        val clear = best >= LOCK_R && (lag < 0 || best > now + SWITCH_R)
        if (clear && cand >= 0 && Math.abs(at - cand) <= JITTER) candN++ else candN = if (clear) 1 else 0
        cand = if (clear) at else -1
        if (candN >= LOCATE_N) {
            lag = at
            candN = 0
        }
        match = if (lag == at || lag < 0) best else now
    }

    // The correlation of the microphone's levels over the [w] bins up to [end] with what played [k] bins before; 0 when too little played.
    // Only bins where something played count: the silence between replies would match at any lag.
    private fun corr(end: Long, w: Int, k: Int): Double {
        var sx = 0.0
        var sy = 0.0
        var sxx = 0.0
        var syy = 0.0
        var sxy = 0.0
        var active = 0
        for (i in 0 until w) {
            val b = end - i
            val x = ref.db(b - k)
            if (x <= FLOOR_DB) continue
            val y = mic.db(b)
            active++
            sx += x
            sy += y
            sxx += x * x
            syy += y * y
            sxy += x * y
        }
        if (active < max(w * MIN_ACTIVE, MIN_BINS)) return 0.0
        val vx = sxx - sx * sx / active
        val vy = syy - sy * sy / active
        if (vx <= 0 || vy <= 0) return 0.0
        return (sxy - sx * sy / active) / Math.sqrt(vx * vy)
    }

    private fun bin(t: Double) = Math.floor(t / BIN_S).toLong()

    /** 10 ms bins of a signal's power, in a ring of the last [SIZE]. */
    private class Bins {
        private val idx = LongArray(SIZE) { Long.MIN_VALUE }
        private val sum = DoubleArray(SIZE)
        private val count = IntArray(SIZE)

        fun add(b: Long, x: Short) {
            val i = Math.floorMod(b, SIZE.toLong()).toInt()
            if (idx[i] != b) {
                idx[i] = b
                sum[i] = 0.0
                count[i] = 0
            }
            sum[i] += x.toDouble() * x
            count[i]++
        }

        /** The mean power, 1 for full scale. */
        fun power(b: Long): Double {
            val i = Math.floorMod(b, SIZE.toLong()).toInt()
            return if (idx[i] == b && count[i] > 0) sum[i] / count[i] / (32768.0 * 32768.0) else 0.0
        }

        fun db(b: Long) = db(power(b))

        /** The bins from [from] to [to] where something played. */
        fun active(from: Long, to: Long): Int {
            var n = 0
            for (b in from..to) if (db(b) > FLOOR_DB) n++
            return n
        }

        fun clearFrom(b: Long) {
            for (i in 0 until SIZE) if (idx[i] >= b) idx[i] = Long.MIN_VALUE
        }
    }

    companion object {
        const val BIN_S = 0.01
        private const val SIZE = 1024
        /** The longest delay looked for: the output's latency, the input's and the air. */
        const val MAX_S = 0.5
        private val MAX_LAG = (MAX_S / BIN_S).toInt()
        /** Bins either side of the lag, for the clocks' jitter. */
        private const val JITTER = 3
        /** The room's tail after the echo, falling 1.5 dB per bin. */
        private const val TAIL = 20
        private val TAIL_W = DoubleArray(TAIL) { Math.pow(10.0, -1.5 * (it + 1) / 10) }
        private const val TAIL_S = TAIL * BIN_S
        /** Below this, nothing plays that could come back. */
        const val FLOOR_DB = -70.0
        /** The cautious start, used until a call has taught the phone its echo: louder than what played. */
        const val START_DB = 10.0
        /** The gain falls this fast while the partner plays louder than the echo seemed. */
        const val FALL_DB = 10.0
        private const val LEARN_DB = 20.0
        private const val PEAK_FALL_DB = 10.0
        private const val LOCATE_EVERY = 10
        private const val LOCATE_S = 3.0
        private const val LOCATE_N = 3
        private const val LOCATE_ACTIVE = 0.5
        /** A lag is taken once the microphone follows what played this well, and dropped for one that does better by [SWITCH_R]. */
        private const val LOCK_R = 0.6
        private const val SWITCH_R = 0.1
        /** Voice over the partner whose last 300 ms follow what played this well is the echo. */
        const val MATCH_S = 0.3
        const val ECHO_R = 0.7
        /** The voiced stretch is short, so it must match better: chance alone often reaches 0.7 there. */
        private const val ECHO_SHORT_R = 0.9
        private const val RISE_R = 0.5
        private const val RISE_DB = 20.0
        private const val RAISE_DB = 6.0
        private const val MIN_ACTIVE = 0.3
        private const val MIN_BINS = 8.0

        fun db(p: Double) = 10 * Math.log10(max(p, 1e-10))

        /** A little above what this phone showed last time, never above the cautious start. */
        fun learned(last: Double?) = if (last == null || last.isNaN()) START_DB else minOf(START_DB, last + 3)
    }
}

/** What was queued to play, on a clock in seconds: when it ends, for the mic gate. */
class Playlog {
    /** When what is queued ends; 0 after a cut. */
    var until = 0.0
        private set

    /** Queues [seconds] of sound right after what is queued, or now when nothing is; returns its start. */
    fun push(seconds: Double, now: Double): Double {
        val at = max(now, until)
        until = at + seconds
        return at
    }

    fun playing(now: Double) = now < until

    /** Playing, or done less than [reach] ago (at least [Voice.GATE_TAIL]): its echo may still come, so the mic gate holds. */
    fun busy(now: Double, reach: Double = Voice.GATE_TAIL) = now < until + max(Voice.GATE_TAIL, reach)

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
 * The gate backs up the echo canceller: while the partner's echo may come, silence goes up and the frames wait, in case they start a barge.
 */
class Uplink(val echo: Echo, private val out: Out) {
    interface Out {
        fun playing(): Boolean
        /** The partner plays, or its echo may still come. */
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

    /** A frame that ended at [t], on the clock [Echo] gets what played on. */
    fun push(frame: ShortArray, t: Double) {
        val quiet = muted
        val f = if (quiet) ShortArray(frame.size) else frame
        val over = if (quiet) null else echo.heard(t, f)
        if (!quiet && barge.push(f, over)) {
            // Over the echo's tail, the voice only opens the gate: nothing plays to cut.
            if (out.playing()) {
                barges++
                out.barge()
            }
            // The voice that set it off and a little before, not the echo ahead of it.
            val keep = minOf(held.size, barge.onset - 1 + Voice.GATE_PRE)
            for (h in held.takeLast(keep)) out.send(h)
            held.clear()
        }
        mic = if (quiet) 0f else Voice.level(f)
        if (!quiet && out.busy() && (out.playing() || !barge.fired)) {
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

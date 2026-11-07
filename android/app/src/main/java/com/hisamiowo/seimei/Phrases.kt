package com.hisamiowo.seimei

import kotlin.math.floor
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sqrt

/** A frame's level in dBFS, from 16-bit samples. */
fun frameDb(f: ShortArray): Double {
    if (f.isEmpty()) return -100.0
    var s = 0.0
    for (x in f) s += x.toDouble() * x
    return 20 * log10(max(sqrt(s / f.size), 1.0) / 32768)
}

/**
 * Cuts dictation into phrases at pauses (docs/asr.md), as web/phrases.js and the laptop's dictate tool do, so each phrase is transcribed while the next is spoken.
 * An energy VAD: the level, smoothed over 100 ms, against the room's noise floor (the 5th percentile of the last 10 s).
 * push() takes 20 ms frames; send(seq, frames) gets each phrase, and cancel(seq) drops an early send when speech goes on.
 * cancel returns false when that phrase's text is in the draft already: what follows is then a phrase of its own.
 */
class Phrases(private val send: (Int, List<ShortArray>) -> Unit, private val cancel: (Int) -> Boolean, private val c: Cut = Cut()) {
    data class Cut(
        val frameMs: Int = 20,
        // A pause this long ends a phrase; at specMs into it the phrase is sent already.
        val silenceMs: Int = 400,
        val specMs: Int = 250,
        val maxPhraseMs: Int = 15000,
        // Phrases with less voice are dropped: a cough comes back as "Thank you."
        val minVoiceMs: Int = 300,
        val prerollMs: Int = 500,
        // Voice starts this far above the floor and holds down to holdDb less.
        val marginDb: Double = 8.0,
        val holdDb: Double = 6.0,
        val minDb: Double = -62.0,
        // The mic pops when it opens.
        val warmupMs: Int = 400,
    )

    private fun frames(ms: Int) = (ms.toDouble() / c.frameMs).roundToInt()

    // The VAD
    private var n = 0
    private val powers = ArrayDeque<Double>()
    private val history = ArrayDeque<Double>()

    private fun voice(f: ShortArray, inPhrase: Boolean): Boolean {
        n++
        val db = frameDb(f)
        if (n * c.frameMs <= c.warmupMs) return false
        powers.addLast(10.0.pow(db / 10))
        if (powers.size > 5) powers.removeFirst()
        val level = 10 * log10(powers.sum() / powers.size)
        // Digital silence (a muted mic) says nothing about the room.
        if (db > -85) {
            history.addLast(level)
            if (history.size > 10000 / c.frameMs) history.removeFirst()
        }
        val noise = if (history.size < 10) -60.0 else history.sorted()[floor(history.size / 20.0).toInt()]
        val margin = c.marginDb - if (inPhrase) c.holdDb else 0.0
        return level > max(noise + margin, c.minDb)
    }

    private val pre = ArrayDeque<ShortArray>()
    private var phrase: MutableList<ShortArray>? = null
    private var voiced = 0
    private var quiet = 0
    private var onset = 0
    private var early: Int? = null
    /** Phrases sent so far, early ones included. */
    var seq = 0
        private set
    /** A phrase is being spoken. */
    val speaking get() = phrase != null

    fun push(f: ShortArray) {
        val v = voice(f, phrase != null)
        val p = phrase
        if (p == null) {
            pre.addLast(f)
            if (pre.size > frames(c.prerollMs)) pre.removeFirst()
            onset = if (v) onset + 1 else 0
            // 60 ms of voice starts a phrase, with the audio from just before it.
            if (onset >= frames(60)) {
                phrase = pre.toMutableList()
                pre.clear()
                voiced = onset
                quiet = 0
            }
            return
        }
        p.add(f)
        if (v) {
            voiced++
            val e = early
            if (quiet > 0 && e != null) {
                // Speech went on after the early send: drop that text, or start anew from the pause when it is shown already.
                if (!cancel(e)) {
                    phrase = p.takeLast(quiet + 1).toMutableList()
                    voiced = 1
                }
                early = null
            }
            quiet = 0
        } else {
            quiet++
        }
        val now = phrase!!
        val enough = voiced * c.frameMs >= c.minVoiceMs
        if (c.specMs > 0 && quiet == frames(c.specMs) && early == null && enough) {
            early = seq
            send(seq++, now.toList())
        }
        if (quiet >= frames(c.silenceMs) || now.size >= frames(c.maxPhraseMs)) {
            // 200 ms of the pause stays with it.
            if (early == null && enough) send(seq++, now.subList(0, now.size - max(0, quiet - frames(200))).toList())
            phrase = null
            voiced = 0
            quiet = 0
            onset = 0
            early = null
        }
    }

    /** Stopped: what is left of a phrase goes too. */
    fun end() {
        val p = phrase
        if (p != null && early == null && voiced * c.frameMs >= c.minVoiceMs) send(seq++, p.toList())
        phrase = null
    }
}

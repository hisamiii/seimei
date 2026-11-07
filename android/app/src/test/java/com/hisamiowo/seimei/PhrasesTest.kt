package com.hisamiowo.seimei

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.random.Random

class PhrasesTest {
    private val rnd = Random(7)

    /** ms of noise at about [db] dBFS, as 20 ms frames. */
    private fun sound(ms: Int, db: Double): List<ShortArray> {
        val amp = 32768 * Math.pow(10.0, db / 20) * Math.sqrt(3.0)
        return List(ms / 20) { ShortArray(320) { (amp * (rnd.nextDouble() * 2 - 1)).toInt().toShort() } }
    }

    private fun room(ms: Int) = sound(ms, -55.0)
    private fun talk(ms: Int) = sound(ms, -20.0)

    /** Runs the frames through; returns the phrases that stand (not cancelled) as lengths in ms, and how many were cancelled. */
    private fun cut(frames: List<ShortArray>, shown: Boolean = false): Pair<List<Int>, Int> {
        val sent = linkedMapOf<Int, Int>()
        var cancelled = 0
        val p = Phrases({ seq, f -> sent[seq] = f.size * 20 }, { seq ->
            if (shown) false else true.also { sent.remove(seq); cancelled++ }
        })
        frames.forEach(p::push)
        p.end()
        return sent.values.toList() to cancelled
    }

    @Test fun pausesEndPhrases() {
        val (phrases, cancelled) = cut(room(1000) + talk(2000) + room(1000) + talk(2000) + room(1000) + talk(1500) + room(800))
        assertEquals(3, phrases.size)
        assertEquals(0, cancelled)
        // Each keeps the 500 ms before its onset and 200 ms of its pause.
        assertTrue(phrases.toString(), phrases.all { it in 1900..2800 })
    }

    @Test fun aShortPauseKeepsOnePhrase() {
        val (phrases, cancelled) = cut(room(1000) + talk(1000) + room(400) + talk(1000) + room(800))
        assertEquals("the early send is dropped when speech goes on", 1, cancelled)
        assertEquals(1, phrases.size)
        assertTrue(phrases.toString(), phrases[0] >= 2400)
    }

    @Test fun anEarlySendAlreadyShownStartsANewPhrase() {
        val (phrases, _) = cut(room(1000) + talk(1000) + room(400) + talk(1000) + room(800), shown = true)
        assertEquals(2, phrases.size)
    }

    @Test fun stoppingMidPhraseSendsWhatThereIs() {
        val (phrases, _) = cut(room(1000) + talk(1200))
        assertEquals(1, phrases.size)
    }

    @Test fun noiseAndCoughsAreNoPhrases() {
        assertTrue(cut(room(5000)).first.isEmpty())
        assertTrue("under 300 ms of voice", cut(room(1000) + talk(200) + room(1000)).first.isEmpty())
    }

    @Test fun aLongPhraseIsCut() {
        // Speech dips between syllables; steady noise would become the room's floor after 10 s.
        val speech = (1..36).flatMap { talk(400) + room(160) }
        val (phrases, _) = cut(room(1000) + speech + room(800))
        assertEquals(2, phrases.size)
        assertEquals(15000, phrases[0])
    }
}

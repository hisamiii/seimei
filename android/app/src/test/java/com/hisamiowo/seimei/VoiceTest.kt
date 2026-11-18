package com.hisamiowo.seimei

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.ZoneOffset
import kotlin.random.Random

class VoiceTest {
    private val rnd = Random(11)

    /** ms of noise at about [db] dBFS, as 20 ms frames. */
    private fun sound(ms: Int, db: Double, r: Random = rnd): List<ShortArray> {
        val amp = 32768 * Math.pow(10.0, db / 20) * Math.sqrt(3.0)
        return List(ms / 20) { ShortArray(320) { (amp * (r.nextDouble() * 2 - 1)).toInt().toShort() } }
    }

    /** ms of something like speech at about [db] dBFS: syllables of noise, 60 to 260 ms long, with pauses and levels of their own. */
    private fun speech(ms: Int, db: Double, r: Random): List<ShortArray> {
        val out = ArrayList<ShortArray>()
        while (out.size < ms / 20) {
            out += sound(60 + r.nextInt(10) * 20, db + r.nextDouble(-6.0, 3.0), r)
            out += sound(20 + r.nextInt(5) * 20, -70.0, r)
        }
        return out.take(ms / 20)
    }

    private fun mix(a: ShortArray, b: ShortArray, gainDb: Double = 0.0): ShortArray {
        val g = Math.pow(10.0, gainDb / 20)
        return ShortArray(a.size) { (a[it] + b[it] * g).toInt().coerceIn(-32768, 32767).toShort() }
    }

    /** A call without the devices: what plays comes back at the microphone [lagMs] later and [echoDb] off, over the room's noise. */
    private inner class Rig(val lagMs: Int = 100, var echoDb: Double = -30.0, gain: Double = Echo.START_DB) {
        val echo = Echo(gain)
        val barge = Barge(echo)
        private val played = ArrayDeque<ShortArray>()
        private val partner = Random(5)
        private val room = Random(6)
        var t = 100.0
        var barges = 0

        /** One 20 ms frame: the partner plays [play] frames of speech (or nothing), the user says [user]; true on a barge. */
        fun step(play: ShortArray?, user: ShortArray? = null): Boolean {
            if (play != null) echo.played(t, play)
            played.addLast(play ?: ShortArray(320))
            val back = if (played.size > lagMs / 20) played.removeFirst() else ShortArray(320)
            var mic = mix(sound(20, -60.0, room)[0], back, echoDb)
            if (user != null) mic = mix(mic, user)
            t += 0.02
            return barge.push(mic, echo.heard(t, mic)).also { if (it) barges++ }
        }

        fun partner(ms: Int) = speech(ms, -20.0, partner)

        /** The room alone, for the voice detection's noise floor, then [ms] of the partner. */
        fun warm(ms: Int): Rig {
            repeat(50) { step(null) }
            for (f in partner(ms)) step(f)
            return this
        }
    }

    @Test fun theEchoFindsItsDelayAndLevel() {
        val r = Rig(lagMs = 100, echoDb = -30.0).warm(6000)
        assertEquals(0.1, r.echo.delay!!, 0.03)
        assertTrue(r.echo.gain.toString(), r.echo.gain in -34.0..-24.0)
        assertTrue(r.echo.match > 0.8)
        assertEquals(0, r.barges)
    }

    @Test fun theUserOverThePartnerBargesIn() {
        val r = Rig().warm(6000)
        val fired = r.partner(600).zip(speech(600, -25.0, Random(7))).map { (p, u) -> r.step(p, u) }
        // 120 ms of voice fires it soon, and only once in a stretch of voice.
        assertTrue(fired.toString(), fired.indexOf(true) in 5..12)
        assertEquals(1, fired.count { it })
    }

    @Test fun anEchoLouderThanThePartnerNeverBarges() {
        // No echo canceller at all: the partner comes back 6 dB louder than it played, from the call's first frame.
        val r = Rig(lagMs = 160, echoDb = 6.0).warm(15000)
        assertEquals(0, r.barges)
    }

    @Test fun anEchoThatGrowsStillNeverBarges() {
        val r = Rig(echoDb = -30.0).warm(6000)
        // The volume goes up by 20 dB: louder than the echo learned, yet it follows what played.
        r.echoDb = -10.0
        for (f in r.partner(10000)) r.step(f)
        assertEquals(0, r.barges)
    }

    @Test fun aPauseRearmsIt() {
        val r = Rig().warm(6000)
        val u = Random(8)
        assertEquals(1, r.partner(600).count { r.step(it, sound(20, -25.0, u)[0]) })
        r.partner(600).forEach { r.step(it) }
        assertEquals(1, r.partner(600).count { r.step(it, sound(20, -25.0, u)[0]) })
    }

    @Test fun nothingPlayingIsNoBarge() {
        val r = Rig()
        repeat(50) { r.step(null) }
        for (f in sound(1000, -20.0)) assertFalse(r.step(null, f))
        assertTrue(r.barge.talking())
        assertNull(r.echo.heard(r.t + 0.02, ShortArray(320)))
    }

    @Test fun aCutDropsWhatWouldHavePlayed() {
        val e = Echo()
        e.played(100.0, sound(1000, -20.0).reduce { a, b -> a + b })
        assertNotNull(e.heard(100.3, ShortArray(320)))
        e.cut(100.3)
        // Past the lags looked for and the room's tail, nothing played that could come back.
        assertNull(e.heard(101.2, ShortArray(320)))
    }

    @Test fun theLearnedEchoStartsALittleHigher() {
        assertEquals(Echo.START_DB, Echo.learned(null), 0.0)
        assertEquals(-27.0, Echo.learned(-30.0), 0.0)
        assertEquals(Echo.START_DB, Echo.learned(20.0), 0.0)
    }

    @Test fun thePlaylogKeepsTime() {
        val p = Playlog()
        assertEquals(100.0, p.push(0.5, 100.0), 1e-9)
        // Back to back: the next chunk starts where the last ends, even when it comes early.
        assertEquals(100.5, p.push(0.5, 100.1), 1e-9)
        assertTrue(p.playing(100.9))
        assertTrue(p.busy(101.2))
        assertFalse(p.busy(101.31))
        // An echo that comes later holds the gate longer.
        assertTrue(p.busy(101.4, reach = 0.5))
        p.push(1.0, 200.0)
        assertEquals(0.75, p.cut(200.25), 1e-9)
        assertFalse(p.busy(200.25))
    }

    /** [n] chunks of 20 ms into [j] at real time from [t0], the ones in [late] that many seconds late. */
    private fun feed(j: Jitter, t0: Double, n: Int, late: Map<Int, Double> = emptyMap()): Double {
        for (i in 0 until n) j.arrived(0.02, t0 + i * 0.02 + (late[i] ?: 0.0))
        return t0 + (n - 1) * 0.02
    }

    @Test fun aSteadyReplyWaitsForTheShortestPrebuffer() {
        val j = Jitter()
        val t = feed(j, 10.0, 50)
        assertEquals(0.0, j.lateMax, 1e-9)
        assertEquals(Jitter.MIN_S, j.target, 1e-9)
        assertFalse(j.ready(0.10, t))
        assertTrue(j.ready(0.16, t))
    }

    @Test fun lateChunksRaiseThePrebuffer() {
        val j = Jitter()
        // One chunk 120 ms behind the pace: the target covers it, with the margin.
        feed(j, 10.0, 50, mapOf(30 to 0.12))
        assertEquals(0.12, j.lateMax, 1e-6)
        assertEquals(0.17, j.target, 0.01)
        // Never past the cap, however late.
        feed(j, 20.0, 50, mapOf(10 to 0.5))
        assertEquals(Jitter.MAX_S, j.target, 1e-9)
        // And it falls back as the call runs steady.
        feed(j, 30.0, 1000)
        assertEquals(Jitter.MIN_S, j.target, 1e-9)
    }

    @Test fun aChunkFasterThanThePaceIsNoLag() {
        val j = Jitter()
        // Phonic runs a little ahead of real time: 20 ms of audio every 19 ms.
        for (i in 0 until 100) j.arrived(0.02, 10.0 + i * 0.019)
        assertEquals(0.0, j.lateMax, 1e-9)
    }

    @Test fun aDrySpeakerMidReplyIsAStall() {
        val j = Jitter()
        val t = feed(j, 10.0, 20)
        j.dried(t + 0.03)
        feed(j, t + 0.2, 20)
        assertEquals(1, j.stalls)
        assertTrue(j.target.toString(), j.target >= Jitter.MIN_S + Jitter.STEP_S - 1e-9)
        // The end of a reply, then the next one a second later: no stall.
        j.dried(20.0)
        feed(j, 21.0, 20)
        assertEquals(1, j.stalls)
    }

    @Test fun aShortReplyPlaysWithoutItsFullPrebuffer() {
        val j = Jitter()
        val t = feed(j, 10.0, 3)
        assertFalse(j.ready(0.06, t + 0.05))
        assertTrue(j.ready(0.06, t + Jitter.END_S + 1e-6))
        assertFalse(j.ready(0.0, t + 1.0))
    }

    @Test fun aCutStartsANewReply() {
        val j = Jitter()
        val t = feed(j, 10.0, 20)
        j.dried(t)
        j.cut()
        // A new reply right after the cut: no stall, and no lag carried over from the old one's pace.
        feed(j, t + 0.1, 20)
        assertEquals(0, j.stalls)
        assertEquals(0.0, j.lateMax, 1e-9)
    }

    private class Fake : Uplink.Out {
        var playing = false
        var busy = false
        val sent = ArrayList<ShortArray>()
        var barges = 0
        override fun playing() = playing
        override fun busy() = busy
        override fun barge() {
            barges++
            busy = false
            playing = false
        }
        override fun send(f: ShortArray) {
            sent += f
        }
    }

    private fun silent(f: ShortArray) = f.all { it == 0.toShort() }

    /** An uplink that heard the room, then the partner playing with no echo back. */
    private fun gated(out: Fake): Pair<Uplink, () -> Double> {
        val echo = Echo(-30.0)
        val up = Uplink(echo, out)
        var t = 100.0
        val next = { t += 0.02; t }
        sound(1000, -55.0).forEach { up.push(it, next()) }
        out.playing = true
        out.busy = true
        val p = Random(9)
        for (f in sound(3000, -55.0)) {
            echo.played(t, speech(20, -20.0, p)[0])
            up.push(f, next())
        }
        return up to next
    }

    @Test fun theGateSendsSilenceUntilABarge() {
        val out = Fake()
        val (up, next) = gated(out)
        out.sent.clear()
        val voice = sound(200, -25.0)
        voice.forEach { up.push(it, next()) }
        assertEquals(1, out.barges)
        // Gated, the first five frames went up as silence. The sixth fired: the five voice frames held and 100 ms before them went up, then the voice live.
        assertEquals(5, out.sent.takeWhile(::silent).size)
        assertEquals(5 + 10 + 5, out.sent.size)
        assertTrue(out.sent.subList(10, 20).zip(voice).all { (a, b) -> a === b })
        assertTrue(up.gated >= 150)
    }

    @Test fun voiceInTheEchosTailOpensTheGate() {
        val out = Fake()
        val (up, next) = gated(out)
        // The partner is done, but its echo may still come.
        out.playing = false
        out.sent.clear()
        sound(400, -25.0).forEach { up.push(it, next()) }
        // Nothing to cut: no barge, but the voice goes up once heard.
        assertEquals(0, out.barges)
        assertEquals(5, out.sent.takeWhile(::silent).size)
        assertTrue(out.sent.drop(5).none(::silent))
    }

    @Test fun mutedSendsSilenceAndNeverBarges() {
        val out = Fake()
        val up = Uplink(Echo(-30.0), out)
        up.muted = true
        out.playing = true
        var t = 100.0
        sound(2000, -10.0).forEach { up.push(it, t); t += 0.02 }
        assertEquals(0, out.barges)
        assertEquals(100, out.sent.size)
        assertTrue(out.sent.all(::silent))
        assertEquals(0f, up.mic)
    }

    @Test fun framesArePcm16LittleEndian() {
        val f = shortArrayOf(1, -2, 0x1234, Short.MIN_VALUE)
        val b = Voice.bytes(f)
        assertArrayEquals(byteArrayOf(1, 0, -2, -1, 0x34, 0x12, 0, -128), b)
        assertArrayEquals(f, Voice.samples(b))
        assertEquals(640, Voice.bytes(ShortArray(Voice.FRAME)).size)
    }

    @Test fun theSocketPathCarriesTheSpeed() {
        assertEquals("api/voice/ws", Voice.path(null))
        assertEquals("api/voice/ws?speed=1.2", Voice.path(1.2))
        assertEquals("api/voice/ws", Voice.path(3.0))
        assertEquals(1.1, Voice.nextSpeed(null), 0.0)
        assertEquals(0.9, Voice.nextSpeed(1.3), 0.0)
        assertEquals("1.0×", Voice.speedText(null))
        assertEquals("1.2×", Voice.speedText(1.2))
    }

    @Test fun messagesUpAndDown() {
        assertEquals("barge", JSONObject(Voice.up("barge")).getString("type"))
        assertEquals(VoiceMsg.State("listening"), VoiceMsg.parse("""{"type": "state", "state": "listening"}"""))
        assertEquals(VoiceMsg.Flush, VoiceMsg.parse("""{"type": "flush"}"""))
        assertEquals(VoiceMsg.Saying("Sure, "), VoiceMsg.parse("""{"type": "saying", "text": "Sure, "}"""))
        assertEquals(VoiceMsg.Said("partner", "Sure."), VoiceMsg.parse("""{"type": "said", "who": "partner", "text": "Sure."}"""))
        assertEquals(VoiceMsg.Error("no key"), VoiceMsg.parse("""{"type": "error", "text": "no key"}"""))
        assertNull(VoiceMsg.parse("""{"type": "asking", "request": "uptime"}"""))
        assertNull(VoiceMsg.parse("not json"))
    }

    @Test fun aCallInTheChat() {
        val c = Chat(ZoneOffset.UTC)
        val t = 1_800_000_000.0
        c.add(JSONObject("""{"id": 1, "type": "voice", "who": "user", "text": "what is the uptime", "t": $t}"""))
        c.add(JSONObject("""{"id": 2, "type": "user", "voice": true, "text": "the laptop's uptime", "t": ${t + 2}}"""))
        c.add(JSONObject("""{"id": 3, "type": "voice", "who": "partner", "text": "Asking seimei.", "cut": true, "t": ${t + 3}}"""))
        val ask = c.items.filterIsInstance<Item.Asked>().single()
        assertTrue(ask.waiting)
        c.add(JSONObject("""{"id": 4, "type": "assistant", "re": 2, "text": "Up 3 days.", "t": ${t + 9}}"""))
        // One card holds the turns; the ask and seimei's reply stay in the chat.
        assertEquals(listOf("Day", "Voice", "Asked", "Msg"), c.items.map { it::class.simpleName })
        val card = c.items[1] as Item.Voice
        assertEquals(listOf("e1", "e3"), card.turns.map { it.key })
        assertEquals(Item.Spoken("e3", "partner", "Asking seimei.", true), card.turns[1])
        assertEquals("Asking seimei.", card.last)
        assertEquals(t + 3, card.t1, 0.0)
        assertFalse(card.ended)
        assertFalse((c.items[2] as Item.Asked).waiting)
        // The reply sits right under the ask: no chip naming what it answers.
        assertNull((c.items[3] as Item.Msg).anchor)
        // The call's end closes the card with its length, however late it comes; seimei's summary fills it in after.
        c.add(JSONObject("""{"id": 5, "type": "voice_end", "secs": 372, "t": ${t + 700}}"""))
        val ended = c.items[1] as Item.Voice
        assertTrue(ended.ended)
        assertEquals(372L, ended.secs)
        assertEquals("", ended.summary)
        c.add(JSONObject("""{"id": 8, "type": "voice_summary", "call": 5, "summary": "Asked for the uptime: 3 days.", "t": ${t + 720}}"""))
        assertEquals("Asked for the uptime: 3 days.", (c.items[1] as Item.Voice).summary)
        // A turn after the end, or more than 5 minutes after the last one, is another call.
        c.add(JSONObject("""{"id": 6, "type": "voice", "who": "user", "text": "hi again", "t": ${t + 710}}"""))
        c.add(JSONObject("""{"id": 7, "type": "voice", "who": "user", "text": "still there?", "t": ${t + 1100}}"""))
        assertEquals(3, c.items.count { it is Item.Voice })
    }
}

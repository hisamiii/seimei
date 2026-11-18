package com.hisamiowo.seimei

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.ZoneOffset
import kotlin.random.Random

class VoiceTest {
    private val rnd = Random(11)

    /** ms of noise at about [db] dBFS, as 20 ms frames. */
    private fun sound(ms: Int, db: Double): List<ShortArray> {
        val amp = 32768 * Math.pow(10.0, db / 20) * Math.sqrt(3.0)
        return List(ms / 20) { ShortArray(320) { (amp * (rnd.nextDouble() * 2 - 1)).toInt().toShort() } }
    }

    /** A barge detector that heard the room, then [echoMs] of the partner at -20 dBFS coming back at [echoDb]. */
    private fun heard(echoMs: Int, echoDb: Double): Barge {
        val b = Barge()
        for (f in sound(1000, -55.0)) assertFalse(b.push(f, null))
        for (f in sound(echoMs, echoDb)) assertFalse("the echo is not the user", b.push(f, -20.0))
        return b
    }

    @Test fun theUserOverThePartnerBargesIn() {
        val b = heard(3000, -50.0)
        // The echo canceller left -50 dBFS under a -20 partner: the estimate has come down to about -30 dB.
        assertTrue(b.echo.toString(), b.echo in -31.0..-28.0)
        val fired = sound(400, -25.0).map { b.push(it, -20.0) }
        // 120 ms of voice: the 6th frame fires, and only once in a stretch of voice.
        assertEquals(5, fired.indexOf(true))
        assertEquals(1, fired.count { it })
        assertTrue(b.talking())
    }

    @Test fun anEchoAsLoudAsThePartnerNeverBarges() {
        val b = heard(3000, -22.0)
        for (f in sound(1000, -22.0)) assertFalse(b.push(f, -20.0))
    }

    @Test fun aPauseRearmsIt() {
        val b = heard(3000, -50.0)
        assertEquals(1, sound(400, -25.0).count { b.push(it, -20.0) })
        sound(500, -50.0).forEach { b.push(it, -20.0) }
        assertEquals(1, sound(400, -25.0).count { b.push(it, -20.0) })
    }

    @Test fun nothingPlayingIsNoBarge() {
        val b = Barge()
        for (f in sound(1000, -55.0) + sound(1000, -20.0)) assertFalse(b.push(f, null))
        assertTrue(b.talking())
    }

    @Test fun theLearnedEchoStartsALittleHigher() {
        assertEquals(Barge.ECHO_START, Barge.learned(null), 0.0)
        assertEquals(-27.0, Barge.learned(-30.0), 0.0)
        assertEquals(Barge.ECHO_START, Barge.learned(20.0), 0.0)
    }

    @Test fun thePlaylogKeepsTimeAndLevels() {
        val p = Playlog()
        assertEquals(100.0, p.push(0.5, -20.0, 100.0), 1e-9)
        // Back to back: the next chunk starts where the last ends, even when it comes early.
        assertEquals(100.5, p.push(0.5, -10.0, 100.1), 1e-9)
        assertEquals(-20.0, p.level(100.2)!!, 0.0)
        assertEquals(-10.0, p.level(100.6)!!, 0.0)
        assertTrue(p.playing(100.9))
        assertTrue(p.busy(101.2))
        assertFalse(p.busy(101.31))
        // What played stays an echo for 0.3 s after.
        assertEquals(-10.0, p.level(101.25)!!, 0.0)
        assertNull(p.level(101.4))
        p.push(1.0, -20.0, 200.0)
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
        var playDb: Double? = null
        var busy = false
        val sent = ArrayList<ShortArray>()
        var barges = 0
        override fun level() = playDb
        override fun busy() = busy
        override fun barge() {
            barges++
            busy = false
            playDb = null
        }
        override fun send(f: ShortArray) {
            sent += f
        }
    }

    @Test fun theGateSendsSilenceUntilABarge() {
        val out = Fake()
        val up = Uplink(Barge.ECHO_START, out)
        sound(1000, -55.0).forEach(up::push)
        out.playDb = -20.0
        out.busy = true
        sound(3000, -50.0).forEach(up::push)
        out.sent.clear()
        val voice = sound(200, -25.0)
        voice.forEach(up::push)
        assertEquals(1, out.barges)
        // Gated, the first five frames went up as silence. The sixth fired: the last 300 ms held went up, then the voice live.
        assertEquals(5, out.sent.takeWhile { f -> f.all { it == 0.toShort() } }.size)
        assertEquals(5 + 15 + 5, out.sent.size)
        assertTrue(out.sent.subList(15, 25).zip(voice).all { (a, b) -> a === b })
        assertTrue(up.gated >= 150)
    }

    @Test fun withTheEchoCancellerTheMicGoesUpLive() {
        val out = Fake()
        val up = Uplink(Barge.ECHO_START, out)
        up.gate = false
        sound(1000, -55.0).forEach(up::push)
        out.playDb = -20.0
        out.busy = true
        out.sent.clear()
        val f = sound(1000, -50.0)
        f.forEach(up::push)
        assertEquals(0, up.gated)
        assertTrue(out.sent.zip(f).all { (a, b) -> a === b })
    }

    @Test fun mutedSendsSilenceAndNeverBarges() {
        val out = Fake()
        val up = Uplink(-30.0, out)
        up.muted = true
        out.playDb = -20.0
        sound(2000, -10.0).forEach(up::push)
        assertEquals(0, out.barges)
        assertEquals(100, out.sent.size)
        assertTrue(out.sent.all { f -> f.all { it == 0.toShort() } })
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

    @Test fun aDirectCallInTheChat() {
        val c = Chat(ZoneOffset.UTC)
        val t = 1_800_000_000.0
        c.add(JSONObject("""{"id": 1, "type": "user", "voice": true, "spoken": true, "text": "is the build green", "t": $t}"""))
        c.add(JSONObject("""{"id": 2, "type": "assistant", "re": 1, "text": "Yeah, all green.", "t": ${t + 3}}"""))
        c.add(JSONObject("""{"id": 3, "type": "assistant", "text": "Something else.", "t": ${t + 4}}"""))
        // The words and the spoken reply are turns in the card; a reply to something else stays in the chat.
        assertEquals(listOf("Day", "Voice", "Msg"), c.items.map { it::class.simpleName })
        val card = c.items[1] as Item.Voice
        assertEquals(listOf(Item.Spoken("e1", "user", "is the build green"), Item.Spoken("e2", "partner", "Yeah, all green.")), card.turns)
        assertEquals("Yeah, all green.", card.last)
    }
}

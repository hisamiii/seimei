package com.hisamiowo.seimei

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Dictation in the composer (docs/asr.md): tap the mic, talk, tap the square; the text lands in the draft at the cursor, and nothing is sent on its own.
 * Each phrase goes to the server as soon as a pause ends it ([Phrases]), so only the last one is left to wait for at the square.
 * The texts come back in the order they were said; while recording, the bar shows them under the draft, and they go into the field at the square.
 * One recording at a time, for whichever composer started it ([target]: "chat", or the page's "w:<id>" / "s:<pid>").
 */
class Dictation(val host: Host) {
    /** What a dictation needs from the app: a scope to run in, the server, and a way to tell the user (with Retry when [retry] is set). */
    interface Host {
        val scope: CoroutineScope
        val api: Api
        fun tell(text: String, retry: (() -> Unit)?)
    }

    enum class State { IDLE, RECORDING, TRANSCRIBING }

    var state by mutableStateOf(State.IDLE)
        private set
    var target by mutableStateOf("")
        private set
    /** The last levels, 0 to 1, newest last: the recording bar's waveform. */
    var levels by mutableStateOf(List(BARS) { 0f })
        private set
    var seconds by mutableStateOf(0)
        private set
    /** Texts for [target]'s field to insert in this order, then clear; while recording, the bar shows them. */
    val results = mutableStateListOf<String>()
    /** A phrase is on its way to the server. */
    var waiting by mutableStateOf(false)
        private set
    /** What was dictated into each target's draft since it was last sent: the server logs what the user changed. */
    private val dictated = HashMap<String, String>()
    private var job: Job? = null
    private var kept: List<ByteArray> = emptyList()
    private var keptHint: JSONObject? = null

    /** One phrase on its way: its text once back; cancelled when speech went on after it was sent early. */
    private class Slot {
        var done = false
        var cancelled = false
        var shown = false
        var text = ""
        var wav: ByteArray? = null
        var failed = false
    }

    /** One dictation: its phrases by number, the next one the draft is waiting for. */
    private inner class Run(val target: String, val hint: JSONObject) {
        val slots = HashMap<Int, Slot>()
        var next = 0
        /** How many phrases the cutter sent, once the recording has ended. */
        var total = Int.MAX_VALUE
        var recording = true
        var dropped = false
        val failed = ArrayList<ByteArray>()
        /** The draft's text so far plus what this dictation added: each phrase's context. */
        var said = hint.optString("draft")
        /** What this dictation added, for [dictated] once it is done. */
        var added = ""
    }

    private fun recount(r: Run) {
        val n = synchronized(r) { r.slots.any { (seq, slot) -> seq >= r.next && !slot.cancelled } }
        if (run === r) waiting = n
    }

    private var run: Run? = null

    @SuppressLint("MissingPermission") // the mic button asks for it first
    fun start(target: String, hint: JSONObject) {
        if (state != State.IDLE) return
        this.target = target
        state = State.RECORDING
        seconds = 0
        levels = List(BARS) { 0f }
        results.clear()
        waiting = false
        val r = Run(target, hint)
        run = r
        val cut = Phrases(
            send = { seq, frames -> host.scope.launch { send(r, seq, frames) } },
            cancel = { seq ->
                synchronized(r) { r.slots[seq]?.takeIf { !it.shown }?.let { it.cancelled = true; true } ?: false }.also { recount(r) }
            },
        )
        job = host.scope.launch {
            val ok = withContext(Dispatchers.IO) {
                val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
                val rec = try {
                    AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, RATE, AudioFormat.CHANNEL_IN_MONO,
                        AudioFormat.ENCODING_PCM_16BIT, maxOf(min, FRAME * 2 * 16))
                } catch (e: Exception) {
                    return@withContext false
                }
                if (rec.state != AudioRecord.STATE_INITIALIZED) return@withContext false.also { rec.release() }
                var frames = 0
                var heard = 0
                try {
                    rec.startRecording()
                    while (isActive && state == State.RECORDING && frames < MAX_S * 50 && frames - heard < IDLE_S * 50) {
                        val buf = ShortArray(FRAME)
                        var got = 0
                        while (got < FRAME) {
                            val n = rec.read(buf, got, FRAME - got)
                            if (n <= 0) break
                            got += n
                        }
                        if (got < FRAME) break
                        frames++
                        synchronized(r) { cut.push(buf) }
                        if (cut.speaking) heard = frames
                        // -60 dBFS is silence, -10 is loud speech; the waveform moves every other frame
                        if (frames % 2 == 0) {
                            val level = ((frameDb(buf) + 60) / 50).coerceIn(0.0, 1.0).toFloat()
                            levels = levels.drop(1) + level
                        }
                        seconds = frames / 50
                    }
                } finally {
                    try { rec.stop() } catch (e: IllegalStateException) {}
                    rec.release()
                }
                true
            }
            if (!ok) {
                state = State.IDLE
                run = null
                host.tell("Couldn't open the microphone", null)
                return@launch
            }
            // Dropped with x: nothing goes in. Stopped (the square, the caps): what is left of a phrase goes too.
            if (r.dropped) return@launch
            state = State.TRANSCRIBING
            synchronized(r) {
                r.recording = false
                cut.end()
                r.total = cut.seq
            }
            flush(r)
        }
    }

    /** The square: transcribe what was recorded. */
    fun stop() {
        if (state == State.RECORDING) state = State.TRANSCRIBING
    }

    /** x: drop the recording, the texts shown so far and the phrases on their way. */
    fun drop() {
        if (state != State.RECORDING) return
        run?.dropped = true
        run = null
        results.clear()
        waiting = false
        state = State.IDLE
        job?.cancel()
    }

    private suspend fun send(r: Run, seq: Int, frames: List<ShortArray>) {
        val slot = Slot()
        synchronized(r) { r.slots[seq] = slot }
        recount(r)
        val pcm = ByteBuffer.allocate(frames.size * FRAME * 2).order(ByteOrder.LITTLE_ENDIAN)
        for (f in frames) for (x in f) pcm.putShort(x)
        val audio = wav(pcm.array())
        slot.wav = audio
        try {
            slot.text = ask(audio, r.hint.put("draft", r.said.takeLast(2000)))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            slot.failed = true
        }
        slot.done = true
        flush(r)
    }

    /** One phrase's text; a call not back in [SLOW_MS] goes again alongside, and the first text wins (one in about 25 takes seconds upstream). */
    private suspend fun ask(audio: ByteArray, hint: JSONObject): String = coroutineScope {
        val call = { async { runCatching { host.api.asr(audio, JSONObject(hint.toString())).optString("text").trim() } } }
        val first = call()
        val quick = withTimeoutOrNull(SLOW_MS) { first.await() }
        if (quick != null) return@coroutineScope quick.getOrElse { call().await().getOrThrow() }
        val second = call()
        val won = select<Result<String>> {
            first.onAwait { it }
            second.onAwait { it }
        }
        won.getOrElse { (if (first.isCompleted) second else first).await().getOrThrow() }.also { first.cancel(); second.cancel() }
    }

    /** The texts in the order they were said, as they come; once the recording has ended and the last is in, the dictation is done. */
    private fun flush(r: Run) {
        if (r.dropped) return
        val out = ArrayList<String>()
        synchronized(r) {
            while (true) {
                val slot = r.slots[r.next] ?: break
                if (!slot.cancelled) {
                    if (!slot.done) break
                    slot.shown = true
                    if (slot.failed) slot.wav?.let { r.failed += it }
                    else if (slot.text.isNotEmpty()) out += slot.text
                }
                r.next++
            }
        }
        for (t in out) {
            r.added = (r.added + " " + t).trim()
            r.said = (r.said + " " + t).trim()
        }
        results.addAll(out)
        recount(r)
        if (r.next < r.total) return
        if (r.added.isNotEmpty()) dictated[r.target] = ((dictated[r.target] ?: "") + " " + r.added).trim()
        if (run === r) run = null
        state = State.IDLE
        if (r.failed.isNotEmpty()) {
            kept = r.failed.toList()
            keptHint = r.hint
            host.tell(if (kept.size > 1) "Couldn't transcribe ${kept.size} phrases" else "Couldn't transcribe") { retry() }
        }
    }

    fun retry() {
        val audio = kept
        if (audio.isEmpty() || state != State.IDLE) return
        kept = emptyList()
        state = State.TRANSCRIBING
        host.scope.launch {
            val lost = ArrayList<ByteArray>()
            for (a in audio) {
                try {
                    val t = ask(a, keptHint ?: JSONObject())
                    if (t.isNotEmpty()) {
                        dictated[target] = ((dictated[target] ?: "") + " " + t).trim()
                        results.add(t)
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    lost += a
                }
            }
            state = State.IDLE
            if (lost.isNotEmpty()) {
                kept = lost
                host.tell("Couldn't transcribe") { retry() }
            }
        }
    }

    /** A message went out from [target]: if part of it was dictated, the server compares what changed. */
    fun sent(target: String, text: String) {
        val said = dictated.remove(target) ?: return
        if (said == text.trim()) return
        host.scope.launch {
            try {
                host.api.post("api/asr/edit", JSONObject().put("dictated", said).put("sent", text))
            } catch (e: IOException) {
            }
        }
    }

    companion object {
        const val RATE = 16000
        /** 20 ms of samples: the phrase cutter's frame. */
        const val FRAME = 320
        /** A whole dictation at most, and how long it listens without a phrase. */
        const val MAX_S = 600
        const val IDLE_S = 60
        const val SLOW_MS = 6000L
        const val BARS = 24

        /** 16 kHz mono s16 PCM in a WAV header. */
        fun wav(pcm: ByteArray): ByteArray {
            val b = ByteBuffer.allocate(44 + pcm.size).order(ByteOrder.LITTLE_ENDIAN)
            b.put("RIFF".toByteArray()).putInt(36 + pcm.size).put("WAVE".toByteArray())
            b.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(RATE).putInt(RATE * 2).putShort(2).putShort(16)
            b.put("data".toByteArray()).putInt(pcm.size).put(pcm)
            return b.array()
        }
    }
}

package com.hisamiowo.seimei

import android.Manifest
import android.annotation.SuppressLint
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableStateListOf
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withTimeoutOrNull
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.lifecycle.viewModelScope
import org.json.JSONObject
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Dictation in the composer (docs/asr.md): tap the mic, talk, tap the square; the text lands in the draft at the cursor, and nothing is sent on its own.
 * Each phrase goes to the server as soon as a pause ends it ([Phrases]), so only the last one is left to wait for at the square.
 * The texts reach the draft in the order they were said; the recording bar hides the field, so they wait for the square.
 * One recording at a time, for whichever composer started it ([target]: "chat", or the page's "w:<id>" / "s:<pid>").
 */
class Dictation(val vm: Seimei) {
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
    /** Texts for [target]'s field to insert in this order, then clear. */
    val results = mutableStateListOf<String>()
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
        val r = Run(target, hint)
        run = r
        val cut = Phrases(
            send = { seq, frames -> vm.viewModelScope.launch { send(r, seq, frames) } },
            cancel = { seq -> synchronized(r) { r.slots[seq]?.takeIf { !it.shown }?.let { it.cancelled = true; true } ?: false } },
        )
        job = vm.viewModelScope.launch {
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
                vm.snack = "Couldn't open the microphone"
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

    /** x: drop the recording, and the phrases already on their way. */
    fun drop() {
        if (state != State.RECORDING) return
        run?.dropped = true
        run = null
        state = State.IDLE
        job?.cancel()
    }

    private suspend fun send(r: Run, seq: Int, frames: List<ShortArray>) {
        val slot = Slot()
        synchronized(r) { r.slots[seq] = slot }
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
        val call = { async { runCatching { vm.api.asr(audio, JSONObject(hint.toString())).optString("text").trim() } } }
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

    /** The texts in the order they were said, once the recording has ended; then the dictation is done. */
    private fun flush(r: Run) {
        if (r.dropped) return
        val out = ArrayList<String>()
        synchronized(r) {
            if (r.recording) return
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
            dictated[r.target] = ((dictated[r.target] ?: "") + " " + t).trim()
            r.said = (r.said + " " + t).trim()
        }
        results.addAll(out)
        if (r.next < r.total) return
        if (run === r) run = null
        state = State.IDLE
        if (r.failed.isNotEmpty()) {
            kept = r.failed.toList()
            keptHint = r.hint
            vm.undo = { retry() }
            vm.snackAction = "Retry"
            vm.snack = if (kept.size > 1) "Couldn't transcribe ${kept.size} phrases" else "Couldn't transcribe"
        }
    }

    fun retry() {
        val audio = kept
        if (audio.isEmpty() || state != State.IDLE) return
        kept = emptyList()
        state = State.TRANSCRIBING
        vm.viewModelScope.launch {
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
                vm.undo = { retry() }
                vm.snackAction = "Retry"
                vm.snack = "Couldn't transcribe"
            }
        }
    }

    /** A message went out from [target]: if part of it was dictated, the server compares what changed. */
    fun sent(target: String, text: String) {
        val said = dictated.remove(target) ?: return
        if (said == text.trim()) return
        vm.viewModelScope.launch {
            try {
                vm.api.post("api/asr/edit", JSONObject().put("dictated", said).put("sent", text))
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

/** Highlights [range] (the text just dictated) in the primary container color. */
private class Tint(val range: TextRange?, val color: androidx.compose.ui.graphics.Color) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val r = range ?: return TransformedText(text, OffsetMapping.Identity)
        val start = r.start.coerceIn(0, text.length)
        val end = r.end.coerceIn(start, text.length)
        val b = AnnotatedString.Builder(text)
        b.addStyle(SpanStyle(background = color), start, end)
        return TransformedText(b.toAnnotatedString(), OffsetMapping.Identity)
    }

    override fun equals(other: Any?) = other is Tint && other.range == range && other.color == color
    override fun hashCode() = range.hashCode() * 31 + color.hashCode()
}

/**
 * The composer's text field: [text] and [onChange], plus [target]'s dictated text inserted at the cursor and tinted for 2 s.
 */
@Composable
fun DraftField(
    text: String, onChange: (String) -> Unit, hint: String, target: String, d: Dictation, modifier: Modifier,
    maxLines: Int, enabled: Boolean = true, focus: FocusRequester? = null,
) {
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    var local by remember { mutableStateOf(TextFieldValue(text, TextRange(text.length))) }
    // A draft changed from outside (sent, shared in, a reply) puts the cursor at its end.
    val value = if (local.text == text) local else TextFieldValue(text, TextRange(text.length))
    var tint by remember { mutableStateOf<TextRange?>(null) }
    LaunchedEffect(d.results.size, d.target) {
        if (d.target != target || d.results.isEmpty()) return@LaunchedEffect
        // Phrases that came in together go in as one run, in their order.
        val t = d.results.joinToString(" ")
        d.results.clear()
        val at = value.selection.max.coerceIn(0, value.text.length)
        val before = value.text.substring(0, at)
        val after = value.text.substring(at)
        val lead = if (before.isNotEmpty() && !before.last().isWhitespace()) " " else ""
        val trail = if (after.isNotEmpty() && !after.first().isWhitespace()) " " else ""
        val next = before + lead + t + trail + after
        val end = at + lead.length + t.length
        local = TextFieldValue(next, TextRange(end))
        onChange(next)
        tint = TextRange(at + lead.length, end)
        delay(2000)
        tint = null
    }
    BasicTextField(
        value = value,
        onValueChange = {
            local = it
            if (it.text != text) {
                tint = null
                onChange(it.text)
            }
        },
        enabled = enabled,
        maxLines = maxLines,
        textStyle = TextStyle(fontSize = 15.sp, lineHeight = 22.sp, color = x.text),
        cursorBrush = SolidColor(cs.primary),
        visualTransformation = Tint(tint, cs.primary.copy(alpha = .22f)),
        modifier = modifier.then(if (focus != null) Modifier.focusRequester(focus) else Modifier).semantics { contentDescription = hint },
        decorationBox = { field ->
            Box {
                if (text.isEmpty()) Text(hint, fontSize = 15.sp, lineHeight = 22.sp, color = x.text3, maxLines = 1)
                field()
            }
        },
    )
}

/** The trailing mic in the field, a spinner while seimei transcribes; hidden without a key on the server. */
@Composable
fun MicSlot(d: Dictation, target: String, enabled: Boolean, hint: () -> JSONObject) {
    val ctx = LocalContext.current
    val vm = d.vm
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) d.start(target, hint()) else vm.snack = "Allow the microphone to dictate"
    }
    Box(Modifier.padding(end = 4.dp, bottom = 2.dp).size(48.dp), contentAlignment = Alignment.Center) {
        if (d.state == Dictation.State.TRANSCRIBING && d.target == target) {
            CircularProgressIndicator(Modifier.size(20.dp).semantics { contentDescription = "Transcribing" }, strokeWidth = 2.dp)
        } else {
            IconButton({
                if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) d.start(target, hint())
                else ask.launch(Manifest.permission.RECORD_AUDIO)
            }, Modifier.size(48.dp), enabled = enabled && d.state == Dictation.State.IDLE) {
                Icon(Icons.mic, "Dictate", Modifier.size(22.dp), tint = LocalTokens.current.text3)
            }
        }
    }
}

/** The field while recording: x to drop, the level, the time, the square to stop. */
@Composable
fun RecordingBar(d: Dictation, modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    Row(modifier.heightIn(min = 52.dp).clip(RoundedCornerShape(26.dp)).background(cs.surfaceContainerHigh)
        .border(2.dp, cs.primary, RoundedCornerShape(26.dp)).padding(start = 4.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically) {
        IconButton(d::drop, Modifier.size(44.dp)) { Icon(Icons.close, "Drop the recording", Modifier.size(18.dp), tint = x.text3) }
        Row(Modifier.weight(1f).height(32.dp), horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
            for (l in d.levels) Box(Modifier.width(3.dp).fillMaxHeight(.12f + .88f * l).clip(RoundedCornerShape(2.dp)).background(cs.primary))
        }
        Spacer(Modifier.width(8.dp))
        Text("%d:%02d".format(d.seconds / 60, d.seconds % 60), fontSize = 13.sp, color = x.text2,
            modifier = Modifier.semantics { contentDescription = "Recording, ${d.seconds} seconds" })
        Spacer(Modifier.width(8.dp))
        Box(Modifier.size(44.dp).clip(RoundedCornerShape(16.dp)).background(cs.primary)
            .clickable(onClickLabel = "Stop and transcribe", onClick = d::stop).semantics { contentDescription = "Stop and transcribe" },
            contentAlignment = Alignment.Center) {
            Box(Modifier.size(14.dp).clip(RoundedCornerShape(4.dp)).background(cs.onPrimary))
        }
    }
}

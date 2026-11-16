package com.hisamiowo.seimei

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.sin
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

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
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) d.start(target, hint()) else d.host.tell("Allow the microphone to dictate", null)
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

/**
 * The field while recording: the [draft] with the texts so far (the newest tinted, dots while a phrase is on its way) where they will go, at its end.
 * Under it the bar: x to drop, the level, the time, the square to stop.
 */
@Composable
fun RecordingBar(d: Dictation, draft: String, modifier: Modifier) {
    val cs = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(26.dp)
    Column(modifier.clip(shape).background(cs.surfaceContainerHigh).border(2.dp, cs.primary, shape)) {
        if (draft.isNotEmpty() || d.results.isNotEmpty() || d.waiting) LiveText(d, draft)
        RecordingRow(d)
    }
}

@Composable
private fun LiveText(d: Dictation, draft: String) {
    val cs = MaterialTheme.colorScheme
    val scroll = rememberScrollState()
    // The end is where the text goes: keep it in view.
    LaunchedEffect(scroll) { snapshotFlow { scroll.maxValue }.collect { scroll.scrollTo(it) } }
    val pulse by rememberInfiniteTransition(label = "dots").animateFloat(0f, 1f, infiniteRepeatable(tween(1200, easing = LinearEasing)), label = "dots")
    val texts = d.results.toList()
    val text = buildAnnotatedString {
        append(draft)
        if (texts.isNotEmpty()) {
            if (draft.isNotEmpty() && !draft.last().isWhitespace()) append(" ")
            texts.dropLast(1).forEach { append(it); append(" ") }
            withStyle(SpanStyle(background = cs.primary.copy(alpha = .22f))) { append(texts.last()) }
        }
        if (d.waiting) {
            append(" ")
            for (i in 0..2) withStyle(SpanStyle(color = cs.primary.copy(alpha = .3f + .7f * abs(sin(PI.toFloat() * (pulse - i * .17f)))), letterSpacing = 1.sp)) { append("•") }
        }
    }
    Text(text, Modifier.fillMaxWidth().heightIn(max = 154.dp).verticalScroll(scroll).padding(start = 20.dp, end = 20.dp, top = 14.dp),
        fontSize = 15.sp, lineHeight = 22.sp, color = LocalTokens.current.text)
}

@Composable
private fun RecordingRow(d: Dictation) {
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(start = 4.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
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

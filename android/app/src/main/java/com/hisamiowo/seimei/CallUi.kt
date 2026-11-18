package com.hisamiowo.seimei

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat

// Voice calls in the chat (docs/voice.md), as the page draws them: the talk button, the call bar, and the call's turns.

/** The talk button, in the send button's place on an empty composer; it asks for the microphone first. */
@Composable
fun TalkButton(vm: Seimei) {
    val ctx = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) vm.startCall() else vm.tell("Allow the microphone to talk", null)
    }
    FilledIconButton({
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) vm.startCall()
        else ask.launch(Manifest.permission.RECORD_AUDIO)
    }, Modifier.size(52.dp)) {
        Icon(Icons.wave, "Talk", Modifier.size(24.dp))
    }
}

/** The pulse of the page's dots: full, down to 30%, and back in 1.2 s. */
@Composable
private fun pulse(): Float {
    val a by rememberInfiniteTransition(label = "pulse").animateFloat(1f, .3f, infiniteRepeatable(tween(600), RepeatMode.Reverse), label = "pulse")
    return a
}

/** During a call, in the composer's place: mute, the pill with the state, the level and the time, the speed, and the red hang-up. */
@Composable
fun CallBar(vm: Seimei, c: VoiceCall) {
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Round(if (c.muted) x.onTonal else x.tonal, if (c.muted) "Unmute" else "Mute", c::toggleMute) {
            Icon(if (c.muted) Icons.micOff else Icons.mic, null, Modifier.size(22.dp), tint = if (c.muted) x.tonal else x.onTonal)
        }
        val pill = RoundedCornerShape(28.dp)
        val them = c.playing && !c.talking
        Row(Modifier.weight(1f).height(52.dp).clip(pill).background(cs.surfaceContainerHigh)
            .then(if (c.talking && !c.muted) Modifier.border(2.dp, cs.primary, pill) else Modifier).padding(horizontal = 18.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(c.label, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Clip)
            // Newest at the end, which stays in view when the pill is narrow.
            Row(Modifier.weight(1f).height(30.dp).clipToBounds(), horizontalArrangement = Arrangement.spacedBy(3.dp, Alignment.End),
                verticalAlignment = Alignment.CenterVertically) {
                for (l in c.levels) Box(Modifier.width(3.dp).height((4 + 24 * l).dp).clip(RoundedCornerShape(2.dp))
                    .background(if (them) cs.onPrimaryContainer.copy(alpha = .75f) else cs.primary))
            }
            Text("%d:%02d".format(c.seconds / 60, c.seconds % 60), fontSize = 13.sp, fontWeight = FontWeight.Medium, color = x.text2)
        }
        Round(x.tonal, "Speed ${Voice.speedText(vm.voiceSpeed)}", vm::nextSpeed) {
            Text(Voice.speedText(vm.voiceSpeed), fontSize = 14.sp, fontWeight = FontWeight.Medium, color = x.onTonal)
        }
        Round(x.error, "Hang up", c::hangUp) { Icon(Icons.hangUp, null, Modifier.size(30.dp, 23.3.dp), tint = cs.surface) }
    }
}

@Composable
private fun Round(bg: androidx.compose.ui.graphics.Color, label: String, onClick: () -> Unit, content: @Composable () -> Unit) {
    Box(Modifier.size(52.dp).clip(CircleShape).background(bg).clickable(onClickLabel = label, onClick = onClick)
        .semantics { contentDescription = label }, contentAlignment = Alignment.Center) { content() }
}

/** A turn the user spoke: their bubble, with the wave mark. */
@Composable
fun SpokenMsg(text: String) {
    val cs = MaterialTheme.colorScheme
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) { Box(Modifier.fillMaxWidth(.88f), contentAlignment = Alignment.CenterEnd) {
        Row(Modifier.clip(RoundedCornerShape(20.dp, 20.dp, 4.dp, 20.dp)).background(cs.primaryContainer).padding(start = 12.dp, end = 16.dp, top = 11.dp, bottom = 11.dp)) {
            Icon(Icons.wave, "Spoken", Modifier.padding(top = 4.dp).size(14.dp).alpha(.6f), tint = cs.onPrimaryContainer)
            Spacer(Modifier.width(8.dp))
            Text(text, color = cs.onPrimaryContainer, lineHeight = 23.2.sp, fontSize = 16.sp)
        }
    } }
}

/** A line of the voice partner's: plain, with a bar on the left, so it never reads as seimei's. [live] while it still speaks, [cut] when the user cut it off. */
@Composable
fun PartnerLine(text: String, cut: Boolean = false, live: Boolean = false) {
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    val dot = if (live) pulse() else 1f
    Row(Modifier.fillMaxWidth(.88f).height(IntrinsicSize.Min)) {
        Box(Modifier.width(2.dp).fillMaxHeight().background(cs.primary))
        Text(buildAnnotatedString {
            append(text)
            if (cut) withStyle(SpanStyle(color = x.text3)) { append(" …") }
            if (live) withStyle(SpanStyle(color = cs.primary.copy(alpha = dot), fontSize = 10.sp)) { append("  ●") }
        }, Modifier.padding(start = 10.dp, top = 2.dp, bottom = 2.dp), color = x.text2, fontSize = 16.sp, lineHeight = 23.2.sp)
    }
}

/** What the partner asked seimei; the dot pulses until seimei's reply lands under it. */
@Composable
fun AskedLine(a: Item.Asked) {
    val cs = MaterialTheme.colorScheme
    val dot = if (a.waiting) pulse() else 1f
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(6.dp).clip(CircleShape).background(cs.primary.copy(alpha = dot)))
        Spacer(Modifier.width(8.dp))
        Text("Asked seimei: ${a.text}", fontSize = 12.5.sp, lineHeight = 17.sp, color = LocalTokens.current.text3)
    }
}

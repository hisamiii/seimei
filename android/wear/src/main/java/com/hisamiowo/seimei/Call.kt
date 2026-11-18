package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.wear.compose.material3.Icon
import androidx.wear.compose.material3.IconButton
import androidx.wear.compose.material3.IconButtonDefaults
import androidx.wear.compose.material3.IconToggleButton
import androidx.wear.compose.material3.IconToggleButtonDefaults
import kotlinx.coroutines.delay

/**
 * A voice call (docs/voice.md) lives in the chat, as on the phone: its turns are chat messages, and this bar takes the composer's place.
 * Mute, the call's time, and hang up; the call itself goes on in [CallService], with the screen off or another app in front.
 * A tap on the time sets the next speed, which shows in its place for a moment.
 */
@Composable
fun CallBar(c: VoiceCall, vm: Model) {
    val t = LocalT.current
    var sped by remember { mutableIntStateOf(0) }
    var showSpeed by remember { mutableStateOf(false) }
    LaunchedEffect(sped) {
        if (sped == 0) return@LaunchedEffect
        showSpeed = true
        delay(1500)
        showSpeed = false
    }
    Row(
        Modifier.width(PILL_W).height(30.dp).clip(RoundedCornerShape(15.dp)).background(t.containerHigh).padding(horizontal = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconToggleButton(
            c.muted, { c.toggleMute() }, Modifier.size(24.dp),
            colors = IconToggleButtonDefaults.colors(
                checkedContainerColor = t.onTonal, checkedContentColor = t.tonal, uncheckedContainerColor = t.tonal, uncheckedContentColor = t.onTonal,
            ),
            shapes = IconToggleButtonDefaults.animatedShapes(),
        ) {
            Icon(if (c.muted) Icons.micOff else Icons.mic, if (c.muted) "Unmute" else "Mute", Modifier.size(15.dp))
        }
        val time = "%d:%02d".format(c.seconds / 60, c.seconds % 60)
        val text = when {
            showSpeed -> Voice.speedText(vm.voiceSpeed)
            c.state == "connecting" || c.state == "reconnecting" -> c.label
            else -> time
        }
        Box(Modifier.weight(1f).height(24.dp).padding(horizontal = 4.dp).clip(RoundedCornerShape(12.dp))
            .clickable(onClickLabel = "Speed") {
                vm.nextSpeed()
                sped++
            }, contentAlignment = Alignment.Center) {
            T(text, size = SMALL1, color = if (showSpeed || c.talking) t.primary else t.text2, weight = FontWeight.Medium, lines = 1, align = TextAlign.Center)
        }
        IconButton(
            c::hangUp, Modifier.size(24.dp), colors = IconButtonDefaults.filledIconButtonColors(containerColor = t.error, contentColor = t.surface),
            shapes = IconButtonDefaults.animatedShapes(),
        ) {
            Icon(Icons.hangUp, "Hang up", Modifier.size(15.dp, 11.7.dp))
        }
    }
}

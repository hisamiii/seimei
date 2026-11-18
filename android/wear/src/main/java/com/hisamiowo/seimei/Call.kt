package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
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

/**
 * A voice call (docs/voice.md) lives in the chat, as on the phone: its turns are chat messages, and this bar takes the composer's place.
 * Mute, the call's time, and hang up; the call itself goes on in [CallService], with the screen off or another app in front.
 */
@Composable
fun CallBar(c: VoiceCall) {
    val t = LocalT.current
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
        T(if (c.state == "connecting" || c.state == "reconnecting") c.label else time, Modifier.weight(1f), SMALL1,
            if (c.talking) t.primary else t.text2, FontWeight.Medium, 1, align = TextAlign.Center)
        IconButton(
            c::hangUp, Modifier.size(24.dp), colors = IconButtonDefaults.filledIconButtonColors(containerColor = t.error, contentColor = t.surface),
            shapes = IconButtonDefaults.animatedShapes(),
        ) {
            Icon(Icons.hangUp, "Hang up", Modifier.size(15.dp, 11.7.dp))
        }
    }
}

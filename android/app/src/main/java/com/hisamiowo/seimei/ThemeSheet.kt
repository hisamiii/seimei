package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.layout.height

// ---------- the theme picker: Menu, then Theme; each tile is a small chat in its theme's own tokens ----------

private val MODES = listOf("system" to "System", "light" to "Light", "dark" to "Dark")

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun ThemeSheet(vm: Seimei, done: () -> Unit) {
    val ctx = LocalContext.current
    val cs = MaterialTheme.colorScheme
    val dark = isDark(vm.themeMode)
    Sheet(done, "Theme") {
        Choice("", MODES, vm.themeMode) { vm.setTheme(mode = it) }
        for (pair in THEMES.chunked(2)) Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            for (name in pair) {
                val usable = name != "material-you" || dynamicColors
                val t = remember(name, dark, vm.accent) { themeTokens(ctx, name, dark, vm.accent) }
                val f = themeFile(ctx, name)
                val sub = when {
                    !usable -> "Needs Android 12"
                    name == "material-you" -> "From your wallpaper"
                    else -> f.getJSONObject(if (dark) "dark" else "light").getString("label")
                }
                Tile(f.getString("name"), sub, t, name == vm.theme, usable, Modifier.weight(1f)) { vm.setTheme(name = name) }
            }
            if (pair.size == 1) Spacer(Modifier.weight(1f))
        }
        if (vm.theme == "catppuccin") {
            val f = themeFile(ctx, "catppuccin")
            val accents = f.getJSONObject("accents").getJSONObject(if (dark) "dark" else "light")
            val on = vm.accent ?: f.getString("accent")
            Text("Accent", fontSize = 14.sp, color = LocalExtra.current.text2)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for (n in accents.keys()) {
                    val c = hex(accents.getJSONObject(n).getString("primary"))
                    Box(Modifier.size(32.dp)
                        .then(if (n == on) Modifier.border(2.dp, cs.onSurface, CircleShape) else Modifier)
                        .padding(4.dp).clip(CircleShape).background(c).clickable { vm.setTheme(accent = n) }
                        .semantics { contentDescription = "Accent $n"; selected = n == on })
                }
            }
        }
        // The phone's own extra, after the shared ones
        var icon by remember { mutableStateOf(appIcon(ctx)) }
        Choice("App icon", APP_ICONS, icon) { setAppIcon(ctx, it); icon = it }
    }
}

/** A labelled connected button group: 40 tall, 2 apart, 20 outer and 8 inner corners, the pick on primary. */
@Composable
private fun Choice(title: String, options: List<Pair<String, String>>, current: String, pick: (String) -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        if (title.isNotEmpty()) Text(title, fontSize = 14.sp, color = LocalExtra.current.text2)
        Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            for ((n, o) in options.withIndex()) {
                val (v, label) = o
                val on = v == current
                val start = if (n == 0) 20.dp else 8.dp
                val end = if (n == options.lastIndex) 20.dp else 8.dp
                Text(label,
                    Modifier.height(40.dp).clip(RoundedCornerShape(start, end, end, start)).background(if (on) cs.primary else cs.primaryContainer)
                        .clickable { pick(v) }.padding(horizontal = 16.dp, vertical = 10.dp)
                        .semantics { selected = on },
                    fontSize = 14.sp, lineHeight = 20.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium,
                    color = if (on) cs.onPrimary else cs.onPrimaryContainer)
            }
        }
    }
}

@Composable
private fun Tile(name: String, sub: String, t: Tokens, picked: Boolean, usable: Boolean, modifier: Modifier, pick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    Column(modifier.clip(RoundedCornerShape(28.dp)).background(cs.surfaceContainerHigh)
        .then(if (picked) Modifier.border(2.dp, cs.primary, RoundedCornerShape(28.dp)) else Modifier)
        .clickable(enabled = usable, onClick = pick).alpha(if (usable) 1f else .5f).padding(6.dp)
        .semantics { selected = picked }, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box {
            Preview(t)
            if (picked) Box(Modifier.padding(6.dp).size(22.dp).clip(CircleShape).background(cs.primary), contentAlignment = Alignment.Center) {
                Text("✓", color = cs.onPrimary, fontSize = 13.sp, fontWeight = FontWeight.Bold)
            }
        }
        Column(Modifier.padding(start = 6.dp, end = 6.dp, bottom = 4.dp)) {
            Text(name, fontSize = 14.sp, lineHeight = 21.7.sp, fontWeight = FontWeight.Bold)
            Text(sub, fontSize = 12.sp, lineHeight = 18.6.sp, color = LocalExtra.current.text3)
        }
    }
}

/** A chat bubble, a reply and a permission card, drawn in [t]. */
@Composable
private fun Preview(t: Tokens) {
    Column(Modifier.fillMaxWidth().heightIn(min = 150.dp).clip(RoundedCornerShape(15.dp)).background(t.surface)
        .padding(start = 10.dp, end = 10.dp, top = 12.dp, bottom = 10.dp), verticalArrangement = Arrangement.spacedBy(7.dp)) {
        Text("ship it", Modifier.align(Alignment.End).clip(RoundedCornerShape(12.dp, 12.dp, 3.dp, 12.dp)).background(t.primaryContainer)
            .padding(horizontal = 10.dp, vertical = 5.dp), color = t.onPrimaryContainer, fontSize = 11.5.sp, lineHeight = 17.8.sp)
        Text(buildAnnotatedString {
            append("Deployed ")
            withStyle(SpanStyle(color = t.primary, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Medium)) { append("0ea1eca") }
            append(". Tests pass.")
        }, color = t.text, fontSize = 11.5.sp, lineHeight = 15.5.sp)
        Spacer(Modifier.weight(1f, fill = false))
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(11.dp)).background(t.containerHigh).padding(horizontal = 9.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(7.dp)) {
            Text(buildAnnotatedString {
                append("Run ")
                withStyle(SpanStyle(color = t.text2, fontFamily = FontFamily.Monospace, fontSize = 10.5.sp)) { append("deploy.sh") }
                append("?")
            }, color = t.text, fontSize = 11.5.sp, lineHeight = 17.8.sp)
            // Deny, then Allow on the thumb side, as on the cards
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Deny", Modifier.clip(CircleShape).background(t.tonal).padding(horizontal = 10.dp, vertical = 3.dp),
                    color = t.onTonal, fontSize = 11.sp, lineHeight = 17.sp, fontWeight = FontWeight.SemiBold)
                Text("Allow", Modifier.clip(CircleShape).background(t.primary).padding(horizontal = 10.dp, vertical = 3.dp),
                    color = t.onPrimary, fontSize = 11.sp, lineHeight = 17.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

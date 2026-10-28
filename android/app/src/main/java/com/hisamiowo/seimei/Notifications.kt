package com.hisamiowo.seimei

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// ---------- Menu, then Notifications: how they reach this phone, and what to change ----------

/** How notifications reach this phone, as the sheet's top card says it. */
data class Route(val title: String, val text: String, val ok: Boolean)

private val bell = icon("M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0", stroke = 1.8f)
private val link = icon("M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1", stroke = 1.8f)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NotificationsSheet(vm: Seimei, done: () -> Unit) {
    val ctx = LocalContext.current
    val cs = MaterialTheme.colorScheme
    val x = LocalExtra.current
    // Bumped after a change, so the card and the rows read the phone again.
    var seen by remember { mutableIntStateOf(0) }
    val route = remember(seen) { Delivery.route(ctx, vm.prefs) }
    val apps = remember(seen) { Delivery.pushApps(ctx) }
    ModalBottomSheet(done, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = cs.surfaceContainer) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 20.dp, end = 20.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Notifications", fontSize = 22.sp)
            Spacer(Modifier.height(8.dp))
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(cs.surfaceContainerHigh).padding(16.dp)) {
                val tone = if (route.ok) x.ok else x.warn
                Box(Modifier.size(40.dp).clip(CircleShape).background(tone.copy(alpha = 0.18f)), contentAlignment = Alignment.Center) {
                    Icon(if (route.ok) Icons.check else Icons.fail, null, Modifier.size(20.dp), tint = tone)
                }
                Spacer(Modifier.width(14.dp))
                Column {
                    Text(route.title, fontSize = 16.sp)
                    if (route.text.isNotEmpty()) Text(route.text, fontSize = 13.sp, color = x.text2)
                }
            }
            Spacer(Modifier.height(4.dp))
            if (apps.isNotEmpty()) {
                var picking by remember { mutableStateOf(false) }
                Box {
                    val more = if (apps.size > 1) " (${apps.size} on this phone)" else ""
                    SheetRow(link, "Push app", Delivery.label(ctx, Delivery.current(ctx)) + more, chevron = apps.size > 1) { picking = apps.size > 1 }
                    DropdownMenu(picking, { picking = false }) {
                        for (a in apps) DropdownMenuItem(text = { Text(Delivery.label(ctx, a)) }, onClick = {
                            picking = false
                            Delivery.pick(ctx, a)
                            vm.registerPush()
                            vm.pushTest = null
                            seen++
                        })
                    }
                }
            } else if (Delivery.FOSS) {
                Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Background connection", Modifier.weight(1f), fontSize = 16.sp)
                    Spacer(Modifier.width(12.dp))
                    Switch(vm.prefs.listen, {
                        vm.prefs.listen = it
                        vm.registerPush()
                        vm.pushTest = null
                        seen++
                    })
                }
                SheetRow(link, "Get a push app", "ntfy on F-Droid or Google Play") {
                    val market = Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=io.heckel.ntfy"))
                    try {
                        ctx.startActivity(market)
                    } catch (e: ActivityNotFoundException) {
                        ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://f-droid.org/packages/io.heckel.ntfy/")))
                    }
                }
            }
            // Each kind opens its own channel's settings: sound, vibration, on the lock screen.
            for ((id, name, _) in Push.CHANNELS) {
                if (id == "listen" && !vm.prefs.listen) continue
                SheetRow(bell, name, Push.ABOUT[id].orEmpty()) {
                    ctx.startActivity(Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
                        .putExtra(Settings.EXTRA_APP_PACKAGE, ctx.packageName).putExtra(Settings.EXTRA_CHANNEL_ID, id))
                }
            }
            Spacer(Modifier.height(8.dp))
            FilledTonalButton({ vm.testPush() }, Modifier.fillMaxWidth().height(48.dp), enabled = vm.pushTest != "Sending…") {
                Text("Send a test notification")
            }
            vm.pushTest?.let { Text(it, Modifier.fillMaxWidth().padding(top = 4.dp), fontSize = 13.sp, color = x.text2, textAlign = TextAlign.Center) }
        }
    }
}

@Composable
private fun SheetRow(icon: ImageVector, title: String, sub: String, chevron: Boolean = true, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable(onClick = onClick).padding(vertical = 12.dp, horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, Modifier.size(22.dp), tint = LocalExtra.current.text2)
        Spacer(Modifier.width(16.dp))
        Column(Modifier.weight(1f)) {
            Text(title, fontSize = 16.sp)
            Text(sub, fontSize = 13.sp, color = LocalExtra.current.text2)
        }
        if (chevron) Icon(Icons.chev, null, Modifier.size(20.dp), tint = LocalExtra.current.text2)
    }
}

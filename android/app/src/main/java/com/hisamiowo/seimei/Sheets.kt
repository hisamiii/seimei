package com.hisamiowo.seimei

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** The web's sheet: 28 corners, a 32 x 4 handle, 20 padding, the title at 22, a 14 gap, at most 88% tall. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Sheet(onDismiss: () -> Unit, title: String, content: @Composable ColumnScope.() -> Unit) {
    val cs = MaterialTheme.colorScheme
    val tall = LocalConfiguration.current.screenHeightDp.dp * .844f - 28.dp
    ModalBottomSheet(onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = cs.surfaceContainer,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        dragHandle = { Box(Modifier.padding(top = 16.dp, bottom = 8.dp).size(32.dp, 4.dp).clip(RoundedCornerShape(2.dp)).background(cs.outline)) }) {
        Column(Modifier.heightIn(max = tall).verticalScroll(rememberScrollState()).navigationBarsPadding()
            .padding(start = 20.dp, end = 20.dp, top = 6.dp, bottom = 16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(title, fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.Medium)
            content()
        }
    }
}

/** What a call touched, from its input: the command, the file, the pattern... (the web's auditWhat). */
fun auditWhat(c: JSONObject): String {
    val raw = c.str("input")
    val inp = runCatching { JSONObject(raw) }.getOrNull() ?: return raw
    for (k in listOf("command", "file_path", "path", "pattern", "url", "query", "description", "prompt", "to", "task", "worker", "session")) {
        val v = inp.opt(k)
        if (v is String && v.isNotBlank()) return v.trim().lineSequence().first()
    }
    return if (raw == "{}") "" else raw
}

/** Menu, then Activity log: every tool call of seimei and the shikigami, newest first, a day at a time, one segmented list a day. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ActivitySheet(vm: Seimei, done: () -> Unit) {
    val r by produceState<JSONObject?>(null) { value = runCatching { vm.api.get("api/audit?limit=300") }.getOrNull() }
    val tk = LocalTokens.current
    Sheet(done, "Activity log") {
        val calls = r?.optJSONArray("calls").objects()
        if (r == null) Text("Loading…", fontSize = 14.sp, color = tk.text3)
        else if (calls.isEmpty()) Text("Nothing yet", fontSize = 14.sp, color = tk.text3)
        for ((day, rows) in calls.groupBy { dayOf(it.optDouble("t")) }) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(dayLabel(rows.first().optDouble("t")), Modifier.padding(top = 12.dp, bottom = 4.dp), fontSize = 14.sp, lineHeight = 22.sp,
                    fontWeight = FontWeight.SemiBold, color = tk.text2)
                for ((i, c) in rows.withIndex()) {
                    Column(Modifier.fillMaxWidth().clip(segment(i, rows.size)).background(tk.containerHigh)
                        .padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                            Text(clock(c.optDouble("t")), fontSize = 12.sp, lineHeight = 20.sp, color = tk.text3, style = TNUM)
                            Text(c.str("who"), fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = tk.text)
                            Text(c.str("tool").replace(Regex("^mcp__.+?__"), ""), fontSize = 14.sp, lineHeight = 20.sp, color = tk.text2)
                            if (c.optBoolean("failed")) Text("Failed", fontSize = 12.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = tk.error)
                        }
                        val what = auditWhat(c)
                        if (what.isNotEmpty()) Text(what, fontFamily = FontFamily.Monospace, fontSize = 14.sp, lineHeight = 20.sp, color = tk.text3,
                            maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }
    }
}

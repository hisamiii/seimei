package com.hisamiowo.seimei

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.snapshotFlow
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import androidx.compose.ui.graphics.compositeOver
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.text.style.TextOverflow

val WINDOWS = listOf("five_hour" to "5-hour", "seven_day" to "Weekly")

@Composable
/** Usage is neutral (primary is for working); the error tone only near the limit. */
fun toneColor(u: Double): Color = if (u >= 0.9) MaterialTheme.colorScheme.error else LocalExtra.current.text2

fun tokens(n: Long): String = when {
    n >= 1_000_000_000 -> "%.1fB".format(Locale.US, n / 1e9)
    n >= 1_000_000 -> "%.1fM".format(Locale.US, n / 1e6)
    n >= 1_000 -> "${Math.round(n / 1e3)}k"
    else -> "$n"
}

/** Cents only below $100: past that they are noise. */
fun usd(n: Double): String = if (n >= 100) "$" + "%,d".format(Locale.US, Math.round(n)) else "$" + "%.2f".format(Locale.US, n)

/** claude-opus-5-5 as Opus 5.5. */
fun modelName(k: String): String = k.removePrefix("claude-").replace(Regex("-(\\d+)-(\\d+)$"), " $1.$2").replace(Regex("-(\\d+)$"), " $1")
    .replaceFirstChar { it.uppercase() }

fun resetText(t: Double): String {
    val mins = maxOf(0L, Math.round((t * 1000 - System.currentTimeMillis()) / 60000.0))
    val left = when {
        mins < 60 -> "$mins min"
        mins < 2880 -> "${mins / 60} h ${mins % 60} min"
        else -> "${Math.round(mins / 1440.0)} days"
    }
    val d = Instant.ofEpochMilli((t * 1000).toLong()).atZone(ZoneId.systemDefault())
    val at = if (dayOf(t) == java.time.LocalDate.now()) clock(t) else d.format(DateTimeFormatter.ofPattern("EEE")) + " " + clock(t)
    return "Resets $at, in $left"
}

@Composable
fun UsageTab(vm: Seimei) {
    val u = vm.usage
    val cs = MaterialTheme.colorScheme
    val scroll = rememberScrollState()
    LaunchedEffect(scroll) { snapshotFlow { scroll.value > 0 }.collect { vm.lifted = it } }
    // The page gap is 14, a section's own gap 8, every list one segmented group
    Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 40.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp)) {
        var any = false
        for ((key, label) in WINDOWS) {
            val w = u?.optJSONObject("windows")?.optJSONObject(key) ?: continue
            any = true
            val util = w.optDouble("utilization", 0.0)
            val tone = toneColor(util)
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(cs.surfaceContainer).padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.Bottom) {
                    Text(label, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, lineHeight = 24.sp, modifier = Modifier.weight(1f).padding(bottom = 4.dp))
                    Text("${Math.round(util * 100)}%", fontSize = 34.sp, lineHeight = 40.sp, fontWeight = FontWeight.SemiBold, color = tone, style = TNUM)
                }
                // M3X linear progress: the active part, a 4dp gap, the rest of the track; no stop dot
                LinearProgressIndicator(
                    progress = { util.toFloat().coerceIn(0f, 1f) }, color = tone, trackColor = LocalTokens.current.containerHighest,
                    strokeCap = StrokeCap.Round, gapSize = 4.dp, drawStopIndicator = {},
                    modifier = Modifier.fillMaxWidth().height(8.dp),
                )
                val resets = w.optDouble("resets_at", 0.0)
                if (resets > 0) Text(resetText(resets), fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text2)
            }
        }
        val note = if (!any) "No usage yet"
        else u?.optDouble("updated", 0.0)?.takeIf { it > 0 }?.let { "Updated ${clock(it)}" }
        note?.let { Note(it) }

        val today = u?.optJSONObject("today")?.optJSONObject("sessions")
        if (today != null && today.length() > 0) {
            Heading("Today in seimei")
            val rows = today.keys().asSequence().map { it to today.getJSONObject(it) }.sortedByDescending { it.second.optLong("output") }.toList()
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                for ((i, r) in rows.withIndex()) {
                    val (name, t) = r
                    Column(Modifier.fillMaxWidth().clip(segment(i, rows.size)).background(cs.surfaceContainer).padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(name.removePrefix("shikigami "), fontWeight = FontWeight.Medium, fontSize = 16.sp, lineHeight = 24.sp, modifier = Modifier.weight(1f))
                            val turns = t.optInt("turns")
                            Spacer(Modifier.width(12.dp))
                            Text("$turns ${if (turns == 1) "turn" else "turns"}", fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3)
                        }
                        Text("${tokens(t.optLong("output"))} out · ${tokens(t.optLong("input") + t.optLong("cache_read") + t.optLong("cache_write"))} in",
                            fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3)
                    }
                }
            }
        }
        CostSection(vm.cost)
        MetricsSection(vm.metrics)
    }
}

@Composable
fun Heading(text: String) = Text(text, fontSize = 14.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold, color = LocalExtra.current.text2,
    modifier = Modifier.padding(top = 14.dp))

@Composable
private fun Note(text: String) = Text(text, fontSize = 14.sp, lineHeight = 20.sp, color = LocalExtra.current.text3)

/** One row of a segmented list: the name, the amount large on the right, a chevron, the facts under the name. */
@Composable
private fun CostRow(shape: Shape, title: String, amount: String, sub: String, open: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).clip(shape).background(MaterialTheme.colorScheme.surfaceContainer).clickable(onClick = onClick)
        .padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.Medium, fontSize = 16.sp, lineHeight = 24.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(sub, fontSize = 12.sp, lineHeight = 18.sp, color = LocalExtra.current.text3, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(8.dp))
        Text(amount, fontSize = 22.sp, lineHeight = 34.sp, fontWeight = FontWeight.SemiBold, style = TNUM)
        Spacer(Modifier.width(8.dp))
        Icon(Icons.chev, null, Modifier.size(18.dp).rotate(if (open) 90f else 0f), tint = LocalExtra.current.text3)
    }
}

/** What the tokens would cost on pay-as-you-go API prices; tap a period for its models. */
@Composable
fun CostSection(cost: JSONObject?) {
    val cs = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Heading("At API prices")
        if (cost == null) {
            Note("Adding up…")
            return@Column
        }
        val since = Instant.ofEpochMilli((cost.optDouble("week_start") * 1000).toLong()).atZone(ZoneId.systemDefault())
            .format(DateTimeFormatter.ofPattern("EEE")) + " " + clock(cost.optDouble("week_start"))
        val rows = listOf(Triple("day", "Today", ""), Triple("week", "This week", "Since $since"), Triple("all", "All time", ""))
            .filter { cost.optJSONObject(it.first) != null }
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for ((i, row) in rows.withIndex()) {
                val (key, label, sub) = row
                val p = cost.getJSONObject(key)
                var open by remember(key) { mutableStateOf(false) }
                CostRow(segment(i, rows.size), label, usd(p.optDouble("usd")),
                    listOf(sub, "%,d API calls".format(Locale.US, p.optLong("messages"))).filter { it.isNotEmpty() }.joinToString(" · "), open) { open = !open }
                if (open) Column(Modifier.padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    val models = p.optJSONObject("models") ?: JSONObject()
                    for (m in models.keys().asSequence().sortedByDescending { models.getJSONObject(it).optDouble("usd") }) {
                        val v = models.getJSONObject(m)
                        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(cs.surfaceContainerLow).padding(horizontal = 12.dp, vertical = 8.dp)) {
                            Row {
                                Text(modelName(m), fontSize = 14.sp, modifier = Modifier.weight(1f))
                                Text(usd(v.optDouble("usd")), fontSize = 14.sp, style = TNUM)
                            }
                            Text("${tokens(v.optLong("output"))} out · ${tokens(v.optLong("input"))} in · ${tokens(v.optLong("cache_read"))} cache read · " +
                                "${tokens(v.optLong("cache_write"))} cache write", fontSize = 12.sp, color = LocalExtra.current.text3)
                        }
                    }
                    val unpriced = p.optJSONArray("unpriced")
                    if (unpriced != null && unpriced.length() > 0) Note("No price for ${List(unpriced.length()) { unpriced.getString(it) }.joinToString(", ")}, counted as \$0")
                }
            }
        }
        Note("Prices of ${cost.str("prices_checked")}. This server only.")
    }
}

fun span(sec: Double): String {
    val m = Math.round(sec / 60).toInt()
    return if (sec < 60) "${Math.round(sec)} s" else if (m < 60) "$m min" else "${m / 60} h ${"%02d".format(m % 60)} min"
}

/** The facts of one shikigami's run, for the Usage list and its details sheet. */
fun metricFacts(m: JSONObject): List<Pair<String, String>> {
    val t = m.optJSONObject("tokens") ?: JSONObject()
    val by = m.optJSONObject("followups_by") ?: JSONObject()
    val asks = m.optInt("asks")
    val follow = m.optInt("followups")
    val from = listOfNotNull(by.optInt("seimei").takeIf { it > 0 }?.let { "$it from seimei" }, by.optInt("user").takeIf { it > 0 }?.let { "$it from you" })
    val rounds = listOfNotNull(
        if (follow > 0) "${if (follow == 1) "Once" else "$follow times"} (${from.joinToString(", ")})" else "Never",
        asks.takeIf { it > 0 }?.let { "$it ${if (it == 1) "question" else "questions"}" },
    ).joinToString(" · ")
    return listOf(
        "Cost" to "${usd(m.optDouble("usd"))}${if (m.optBoolean("adopted")) ", with its time as your session" else ""}",
        "Tokens" to "${tokens(t.optLong("input"))} in · ${tokens(t.optLong("cache_read"))} cache read · " +
            "${tokens(t.optLong("cache_write"))} cache write · ${tokens(t.optLong("output"))} out",
        "Time" to "${span(m.optDouble("wall"))}, busy ${span(m.optDouble("busy"))}",
        // Asks while it worked, messages after its first done, and whether one asked for more work.
        "Reopened" to rounds,
    ) + m.optJSONArray("tunes").objects().let { t ->
        // Effort, model or permissions changed on the way, oldest first.
        if (t.isEmpty()) emptyList() else listOf("Changes" to t.joinToString(" · ") {
            "${it.str("what")} at ${clock(it.optDouble("t"))}"
        })
    }
}

/** Each shikigami: what it was given, what it cost, how it went; tap one for its facts. */
@Composable
fun MetricsSection(list: List<JSONObject>?) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Heading("Shikigami")
        if (list == null) {
            Note("Adding up…")
            return@Column
        }
        if (list.isEmpty()) {
            Note("No shikigami yet")
            return@Column
        }
        val finished = list.filter { !it.isNull("first_done") }
        Note("${usd(list.sumOf { it.optDouble("usd") })} in all · ${finished.count { it.optBoolean("reopened") }} of ${finished.size} reopened")
        var all by remember { mutableStateOf(false) }
        val shown = if (all) list else list.take(8)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for ((i, m) in shown.withIndex()) {
                var open by remember(m.str("id")) { mutableStateOf(false) }
                val state = if (m.str("state") == "asking") "question" else m.str("state")
                CostRow(segment(i, shown.size), m.str("name"), usd(m.optDouble("usd")),
                    listOf(modelName(m.str("model")), m.str("effort"), m.str("context"), span(m.optDouble("wall")), state,
                        if (m.optBoolean("reopened")) "reopened" else "").filter { it.isNotEmpty() }.joinToString(" · "), open) { open = !open }
                if (open) Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Fact("Folder", home(m.str("cwd")), mono = true)
                    for ((k, v) in metricFacts(m).drop(1)) Fact(k, v)
                }
            }
        }
        if (list.size > 8 && !all) TextButton({ all = true }) { Text("Show all ${list.size}") }
    }
}

/** The Usage tab's icon: the 5-hour window as a ring (text-2 on an outline-variant track, on-primary on the pill). */
@Composable
fun UsageRing(vm: Seimei, on: Boolean = false) {
    val w = vm.usage?.optJSONObject("windows")?.optJSONObject("five_hour")
    val util = w?.optDouble("utilization", 0.0) ?: 0.0
    val cs = MaterialTheme.colorScheme
    val tone = when {
        on -> cs.onPrimary
        w == null -> Color.Transparent
        vm.usage?.str("status") == "rejected" -> cs.error
        else -> toneColor(util)
    }
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        androidx.compose.material3.CircularProgressIndicator(
            progress = { util.toFloat().coerceIn(0f, 1f) }, modifier = Modifier.size(19.4.dp), color = tone,
            trackColor = if (on) cs.onPrimary.copy(alpha = .3f) else cs.outlineVariant, strokeWidth = 2.4.dp, gapSize = 0.dp,
            strokeCap = androidx.compose.ui.graphics.StrokeCap.Round,
        )
    }
}

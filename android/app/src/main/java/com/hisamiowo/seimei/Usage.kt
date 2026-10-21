package com.hisamiowo.seimei

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
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

val WINDOWS = listOf("five_hour" to "5-hour", "seven_day" to "Weekly")

@Composable
fun toneColor(u: Double): Color = when {
    u >= 0.95 -> MaterialTheme.colorScheme.error
    u >= 0.8 -> LocalExtra.current.warn
    else -> LocalExtra.current.ok
}

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
    val at = if (dayOf(t) == java.time.LocalDate.now()) clock(t) else d.format(DateTimeFormatter.ofPattern("EEE HH:mm"))
    return "resets $at, in $left"
}

@Composable
fun UsageTab(vm: Seimei) {
    val u = vm.usage
    val cs = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        var any = false
        for ((key, label) in WINDOWS) {
            val w = u?.optJSONObject("windows")?.optJSONObject(key) ?: continue
            any = true
            val util = w.optDouble("utilization", 0.0)
            val tone = toneColor(util)
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(cs.surfaceContainerLow).padding(horizontal = 16.dp, vertical = 14.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.Bottom) {
                    Text(label, fontWeight = FontWeight.Medium, fontSize = 15.sp, modifier = Modifier.weight(1f))
                    Text("${Math.round(util * 100)}%", fontSize = 28.sp, color = tone, style = TNUM)
                }
                LinearProgressIndicator(
                    progress = { util.toFloat().coerceIn(0f, 1f) }, color = tone, trackColor = cs.surfaceContainerHigh,
                    strokeCap = StrokeCap.Round, gapSize = 0.dp, drawStopIndicator = {},
                    modifier = Modifier.fillMaxWidth().height(8.dp),
                )
                val resets = w.optDouble("resets_at", 0.0)
                if (resets > 0) Text(resetText(resets), fontSize = 13.sp, color = LocalExtra.current.text3)
            }
        }
        val note = if (!any) "No usage reported yet. Claude Code reports it when a session takes a turn."
        else u?.optDouble("updated", 0.0)?.takeIf { it > 0 }?.let { "Updated ${clock(it)}. Claude Code reports usage when a session takes a turn." }
        note?.let { Text(it, fontSize = 13.sp, color = LocalExtra.current.text3) }

        val today = u?.optJSONObject("today")?.optJSONObject("sessions")
        if (today != null && today.length() > 0) {
            Heading("Today in seimei")
            val rows = today.keys().asSequence().map { it to today.getJSONObject(it) }.sortedByDescending { it.second.optLong("output") }.toList()
            Column {
                for ((i, r) in rows.withIndex()) {
                    if (i > 0) HorizontalDivider(color = cs.surfaceContainerHigh)
                    val (name, t) = r
                    Column(Modifier.padding(vertical = 10.dp)) {
                        Row {
                            Text(name, fontWeight = FontWeight.Medium, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            val turns = t.optInt("turns")
                            Text("$turns ${if (turns == 1) "turn" else "turns"}", fontSize = 12.sp, color = LocalExtra.current.text3)
                        }
                        Text("${tokens(t.optLong("output"))} out · ${tokens(t.optLong("input") + t.optLong("cache_read") + t.optLong("cache_write"))} in",
                            fontSize = 13.sp, color = LocalExtra.current.text3)
                    }
                }
            }
        }
        CostSection(vm.cost)
        MetricsSection(vm.metrics)
        Spacer(Modifier.size(8.dp))
    }
}

@Composable
fun Heading(text: String) = Text(text, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = LocalExtra.current.text2, modifier = Modifier.padding(top = 6.dp))

/** What the tokens would cost on pay-as-you-go API prices; tap a period for its models. */
@Composable
fun CostSection(cost: JSONObject?) {
    val cs = MaterialTheme.colorScheme
    Heading("At API prices")
    if (cost == null) {
        Text("Adding up…", fontSize = 13.sp, color = LocalExtra.current.text3)
        return
    }
    val since = Instant.ofEpochMilli((cost.optDouble("week_start") * 1000).toLong()).atZone(ZoneId.systemDefault())
        .format(DateTimeFormatter.ofPattern("EEE HH:mm"))
    Column {
        for ((i, row) in listOf(Triple("day", "Today", ""), Triple("week", "This week", "since $since"), Triple("all", "All time", "")).withIndex()) {
            val (key, label, sub) = row
            val p = cost.optJSONObject(key) ?: continue
            var open by remember(key) { mutableStateOf(false) }
            if (i > 0) HorizontalDivider(color = cs.surfaceContainerHigh)
            Row(Modifier.fillMaxWidth().clickable { open = !open }.padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(label, fontWeight = FontWeight.Medium, fontSize = 15.sp)
                    Text(listOf(sub, "%,d API calls".format(Locale.US, p.optLong("messages"))).filter { it.isNotEmpty() }.joinToString(" · "),
                        fontSize = 12.sp, color = LocalExtra.current.text3)
                }
                Text(usd(p.optDouble("usd")), fontSize = 20.sp, style = TNUM)
                Spacer(Modifier.width(6.dp))
                Icon(Icons.chev, null, Modifier.size(18.dp).rotate(if (open) 90f else 0f), tint = LocalExtra.current.text3)
            }
            if (open) Column(Modifier.padding(bottom = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
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
                if (unpriced != null && unpriced.length() > 0) Text("No price for ${List(unpriced.length()) { unpriced.getString(it) }.joinToString(", ")}; counted as \$0.",
                    fontSize = 12.sp, color = LocalExtra.current.text3)
            }
        }
    }
    Text("What this laptop's Claude Code use would cost on pay-as-you-go API prices (checked ${cost.str("prices_checked")}). " +
        "Other machines on the account are not included.", fontSize = 13.sp, color = LocalExtra.current.text3)
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
        asks.takeIf { it > 0 }?.let { "$it ${if (it == 1) "ask" else "asks"}" },
        follow.takeIf { it > 0 }?.let { "$it ${if (it == 1) "follow-up" else "follow-ups"} (${from.joinToString(", ")})" },
        "reopened".takeIf { m.optBoolean("reopened") },
    ).joinToString(" · ")
    return listOf(
        "Cost" to "${usd(m.optDouble("usd"))} at API prices${if (m.optBoolean("adopted")) ", with its time as your session" else ""}",
        "Tokens" to "${tokens(t.optLong("input"))} in · ${tokens(t.optLong("cache_read"))} cache read · " +
            "${tokens(t.optLong("cache_write"))} cache write · ${tokens(t.optLong("output"))} out",
        "Time" to "${span(m.optDouble("wall"))}, busy ${span(m.optDouble("busy"))}",
        // Asks while it worked, messages after its first done, and whether one asked for more work.
        "Rounds" to rounds.ifEmpty { "one go" },
    ) + m.optJSONArray("tunes").objects().let { t ->
        // Effort, model or permissions changed on the way (#74), oldest first.
        if (t.isEmpty()) emptyList() else listOf("Changes" to t.joinToString(" · ") {
            "${it.str("what")} at ${java.text.SimpleDateFormat("HH:mm", java.util.Locale.getDefault()).format(java.util.Date((it.optDouble("t") * 1000).toLong()))}"
        })
    }
}

/** Each shikigami: what it was given, what it cost, how it went; tap one for its facts. */
@Composable
fun MetricsSection(list: List<JSONObject>?) {
    val cs = MaterialTheme.colorScheme
    Heading("Shikigami")
    if (list == null) {
        Text("Adding up…", fontSize = 13.sp, color = LocalExtra.current.text3)
        return
    }
    if (list.isEmpty()) {
        Text("No shikigami yet.", fontSize = 13.sp, color = LocalExtra.current.text3)
        return
    }
    val finished = list.filter { !it.isNull("first_done") }
    Text("${list.size} shikigami, ${usd(list.sumOf { it.optDouble("usd") })} at API prices. " +
        "${finished.count { it.optBoolean("reopened") }} of ${finished.size} that finished got more work after.",
        fontSize = 13.sp, color = LocalExtra.current.text3)
    var all by remember { mutableStateOf(false) }
    Column {
        for ((i, m) in (if (all) list else list.take(8)).withIndex()) {
            var open by remember(m.str("id")) { mutableStateOf(false) }
            if (i > 0) HorizontalDivider(color = cs.surfaceContainerHigh)
            Row(Modifier.fillMaxWidth().clickable { open = !open }.padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(m.str("name"), fontWeight = FontWeight.Medium, fontSize = 15.sp)
                    Text(listOf(modelName(m.str("model")).replaceFirstChar { it.uppercase() }, m.str("effort"), m.str("context"),
                        span(m.optDouble("wall")), m.str("state"), if (m.optBoolean("reopened")) "reopened" else "")
                        .filter { it.isNotEmpty() }.joinToString(" · "), fontSize = 12.sp, color = LocalExtra.current.text3)
                }
                Text(usd(m.optDouble("usd")), fontSize = 20.sp, style = TNUM)
                Spacer(Modifier.width(6.dp))
                Icon(Icons.chev, null, Modifier.size(18.dp).rotate(if (open) 90f else 0f), tint = LocalExtra.current.text3)
            }
            if (open) Column(Modifier.padding(bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Fact("Folder", home(m.str("cwd")), mono = true)
                for ((k, v) in metricFacts(m).drop(1)) Fact(k, v)
            }
        }
    }
    if (list.size > 8 && !all) TextButton({ all = true }) { Text("Show all ${list.size}") }
}

/** The Usage tab's icon: the 5-hour window as a ring. */
@Composable
fun UsageRing(vm: Seimei) {
    val w = vm.usage?.optJSONObject("windows")?.optJSONObject("five_hour")
    val util = w?.optDouble("utilization", 0.0) ?: 0.0
    val tone = if (w == null) LocalExtra.current.text3 else if (vm.usage?.str("status") == "rejected") MaterialTheme.colorScheme.error else toneColor(util)
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        androidx.compose.material3.CircularProgressIndicator(
            progress = { util.toFloat().coerceIn(0f, 1f) }, modifier = Modifier.size(19.dp), color = tone,
            trackColor = MaterialTheme.colorScheme.outline, strokeWidth = 2.dp, gapSize = 0.dp,
        )
    }
}

package com.hisamiowo.seimei

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import org.json.JSONObject

val ACTIVE = setOf("running", "starting", "asking")

// ---------- tones and words, as on the web page ----------

// The theme's tokens (Theme.kt) under the names the pages use.
val PEACH: Color @Composable get() = LocalTokens.current.warn
val MAUVE: Color @Composable get() = LocalTokens.current.primary

/** "at 10:28 PM" today, "on 18 Oct at 10:28 PM" another day. */
fun atTime(t: Double): String =
    if (dayOf(t) == LocalDate.now()) "at ${clock(t)}" else "on ${dayOf(t).format(DateTimeFormatter.ofPattern("d MMM"))} at ${clock(t)}"

fun stateText(w: JSONObject): String {
    val at = atTime(w.optDouble("updated", 0.0))
    val since = at.removePrefix("at ").removePrefix("on ")
    return when (w.str("state")) {
        "starting" -> "Starting"
        "running" -> "Working since $since"
        "asking" -> "Needs you"
        "done" -> "Finished $at"
        "failed" -> "Failed $at"
        "stopped" -> "Stopped $at"
        "interrupted" -> "Interrupted $at"
        "laptop" -> "In a terminal on the server since $since"
        "released" -> "Your session since $since"
        else -> w.str("state")
    }
}

fun String.cap() = replaceFirstChar { it.uppercase() }

/** What a running shikigami is up to: its latest step. */
fun workerLine(w: JSONObject): String {
    val d = w.optJSONObject("doing") ?: return if (w.str("state") == "starting") "Starting" else ""
    if (d.str("text").isNotEmpty()) return d.str("text")
    // A command is the news itself
    if (d.str("name") == "Bash" && d.str("summary").isNotEmpty()) return d.str("summary")
    val what = doingPhrase(d.str("name")).cap()
    return if (d.str("summary").isNotEmpty()) "$what: ${d.str("summary")}" else what
}

/** What a session is up to: its open call and how long it has run. */
fun sessionLine(d: JSONObject?): String {
    d ?: return ""
    val what = if (d.str("tool") == "Bash") d.str("what") else "${doingPhrase(d.str("tool"))}: ${d.str("what")}"
    val since = d.optDouble("since", 0.0)
    return (if (d.str("tool") == "Bash") what else what.cap()) + if (since > 0) " · ${elapsed(since).substringBefore(' ')}" else ""
}

/** Seconds ago as now, 5m, 3h, 2d. */
fun short(t: Double): String {
    val s = maxOf(0.0, System.currentTimeMillis() / 1000.0 - t)
    return when {
        s < 60 -> "now"
        s < 3600 -> "${(s / 60).toInt()}m"
        s < 86400 -> "${(s / 3600).toInt()}h"
        else -> "${(s / 86400).toInt()}d"
    }
}

/** A clock time today, a date before that. */
fun whenText(t: Double): String =
    if (dayOf(t) == LocalDate.now()) clock(t) else dayOf(t).format(DateTimeFormatter.ofPattern("d MMM"))

/** The theme's project colors; a project keeps its place in the list everywhere (the same hash as the web page). */
@Composable
fun tint(project: String): Color = tintOf(project, LocalTokens.current)

fun tintOf(project: String, t: Tokens): Color {
    if (project.isEmpty()) return t.text3
    var h = 0L
    for (c in project) h = (h * 31 + c.code) and 0xFFFFFFFFL
    return t.tints[(h % t.tints.size).toInt()]
}

/** One row of the list and the page head, from a shikigami or one of the user's sessions. */
data class Agent(
    val kind: String, val id: String, val name: String, val project: String, val context: Int?, val state: String,
    val since: Double, val line: String, val fresh: Boolean = false, val bad: Boolean = false,
    /** A finished shikigami's end state (stopped, interrupted, ...), for its tile icon. */
    val end: String = "",
    /** Hidden from the list; find: what the Archived search looks in. */
    val archived: Boolean = false,
    val find: List<String> = emptyList(),
    /** A side thread's tile wears the thread icon; sent: how often it went to seimei. */
    val icon: ImageVector? = null,
    val sent: Int = 0,
)

fun JSONObject.context(): Int? = if (has("context") && !isNull("context")) optInt("context") else null

fun fromWorker(w: JSONObject): Agent {
    val st = w.str("state")
    val state = if (st == "asking") "ask" else if (st in ACTIVE) "work" else "done"
    return Agent(
        "worker", w.str("id"), w.str("name"), w.str("project"), w.context(), state, w.optDouble("updated", 0.0),
        when (state) {
            "ask" -> w.str("question")
            "work" -> workerLine(w)
            else -> w.str("summary").ifEmpty { stateText(w) }
        },
        bad = st == "failed", end = st, archived = w.optBoolean("archived"),
        find = listOf(w.str("name"), w.str("task"), w.str("summary"),
            w.optJSONObject("now")?.let { "${it.str("id")} ${it.str("title")}" } ?: ""),
    )
}

fun fromSession(s: JSONObject, seen: JSONObject): Agent {
    val q = s.optBoolean("question")
    val asking = q || s.str("status") == "waiting"
    val state = if (asking) "ask" else if (s.str("status") == "busy") "work" else "idle"
    return Agent(
        "session", s.optLong("pid").toString(), s.str("name"), s.str("project"), s.context(), state, s.optDouble("since", 0.0),
        when {
            q -> s.str("ask")
            asking -> s.str("waiting_text").ifEmpty { "Waiting on the server" }
            state == "work" -> sessionLine(s.optJSONObject("doing"))
            else -> s.str("line")
        },
        fresh = state == "idle" && s.optDouble("active", 0.0) > seen.optDouble(s.str("session_id"), 0.0) + 1,
        archived = s.optBoolean("archived"), find = listOf(s.str("name"), s.str("project"), s.str("line")),
    )
}

/** A side thread as a row of the lists and the page's header: the agents' idle tile wearing the thread icon. */
fun fromThread(t: JSONObject): Agent {
    val work = t.str("state") in setOf("running", "starting")
    val line = when {
        work -> t.optJSONObject("doing")?.let { workerLine(JSONObject().put("doing", it)) }?.ifEmpty { null } ?: "Working"
        t.str("shikigami").isNotEmpty() -> "Became a shikigami"
        else -> t.str("last")
    }
    return Agent("thread", t.str("id"), t.str("name"), "", t.context(), if (work) "work" else "idle", t.optDouble("updated", 0.0), line,
        fresh = t.optInt("unread") > 0, archived = t.optBoolean("archived"),
        find = listOf(t.str("name"), t.str("last"), t.str("quote")), icon = Icons.thread, sent = t.optInt("sent"))
}

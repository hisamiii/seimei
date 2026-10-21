package com.hisamiowo.seimei

import org.json.JSONObject
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

/** One row of the chat, built from logged events (a port of the web page's Renderer). */
sealed interface Item {
    val key: String

    data class Day(override val key: String, val t: Double) : Item
    /** anchor: the message this one answers, when something sits between them; reply: the event id "Reply in chat"
     *  answers (a message of seimei's in the trajectory), 0 for none. */
    data class Msg(override val key: String, val role: String, val text: String, val images: List<String>, val t: Double,
                   val files: List<FileRef> = emptyList(), val anchor: Anchor? = null, val reply: Long = 0) : Item
    /** A run of trajectory events, as one line in the chat; goto is the key of the first one. */
    data class Fold(override val key: String, val text: String, val goto: String) : Item
    /** What started a dispatcher turn in the trajectory: "after app-tester finished". */
    data class Trig(override val key: String, val text: String) : Item
    data class Note(override val key: String, val text: String, val level: String, val n: Int = 1, val t: Double = 0.0) : Item
    data class Activity(override val key: String, val calls: List<Call>) : Item
    data class Worker(
        override val key: String, val event: String, val name: String, val worker: String,
        val text: String, val model: String, val cwd: String, val effort: String = "", val contextCap: String = "",
        /** An "asks" card that something later settled: "Answered" or "Closed". */
        val settled: String? = null,
    ) : Item
    data class Approval(
        override val key: String, val aid: String, val who: String, val action: String, val target: String,
        val detail: String, val expires: Double, val outcome: String?,
    ) : Item
    /** w and h: the picture's size from the laptop (0 when unknown), so its space is kept before it loads. */
    data class Shown(override val key: String, val file: String, val caption: String, val w: Int = 0, val h: Int = 0) : Item
    /** A file seimei handed over to download. */
    data class Sent(override val key: String, val file: FileRef, val caption: String, val t: Double = 0.0) : Item
}

/** A chip quoting another message; a tap goes to it (lane "c": the chat, "t": the trajectory). */
data class Anchor(val text: String, val goto: String, val lane: String)

fun anchorText(text: String) = text.replace(Regex("\\s+"), " ").trim().let { if (it.length > 60) it.take(60) + "…" else it }

/** Events in lane "t" happened on their own (events.py): the trajectory gets them whole, the chat a fold line. */
fun JSONObject.inTraj() = str("lane") == "t"

/** What counts toward the Trajectory badge: reports, notes and seimei's own words, not its tool calls. */
fun JSONObject.worth() = inTraj() && str("type") in setOf("assistant", "worker", "system", "rate_limit")

/** Shikigami id -> its open "asks" card, wherever it was drawn: a report in the trajectory settles a card in the chat. */
class Asks : HashMap<String, Pair<Chat, Int>>()

private val ENDED = mapOf("done" to "finished", "failed" to "failed", "stopped" to "was stopped", "interrupted" to "was interrupted")

/** A file stored on the laptop (uploads.py), downloaded only when tapped. */
data class FileRef(val id: String, val name: String, val size: Long, val kind: String)

fun JSONObject.fileRef(id: String = str("id")) = FileRef(id, str("name"), optLong("size"), str("kind"))

/** The laptop deletes files after this long (uploads.py KEEP_DAYS); older chips show as expired. */
const val KEEP_SECONDS = 30 * 86400.0

fun expired(t: Double, now: Double = System.currentTimeMillis() / 1000.0) = t > 0 && now - t > KEEP_SECONDS

fun sizeText(n: Long) = if (n >= 1 shl 20) "%.1f MB".format(n / 1048576.0) else "${maxOf(1L, Math.round(n / 1024.0))} KB"

data class Call(val id: String, val name: String, val summary: String, val running: Boolean = true,
                val error: Boolean = false, val out: String = "")

/** A system line repeated within this many seconds counts up on the first one instead of showing again. */
const val REPEAT = 900.0

fun dayOf(t: Double, zone: ZoneId = ZoneId.systemDefault()): LocalDate =
    Instant.ofEpochMilli((t * 1000).toLong()).atZone(zone).toLocalDate()

fun JSONObject.str(k: String) = optString(k, "").let { if (it == "null") "" else it }

class Chat(private val zone: ZoneId = ZoneId.systemDefault(), private val lane: String = "c", private val asks: Asks = Asks()) {
    val items = ArrayList<Item>()
    /** Texts of messages by event id, for anchors and "Reply in chat". */
    val said = HashMap<Long, String>()
    // Chat: the open fold line and what it names, the user's last message, and whether something sits after it.
    private var fold = -1
    private val reports = ArrayList<String>()
    private val notesSeen = ArrayList<String>()
    private var lastUser = 0L
    private var between = false
    // Trajectory: the dispatcher turn being drawn.
    private var turn = 0L
    private var from = 0L
    private var day: LocalDate? = null
    private var activity = -1
    private val tools = HashMap<String, Int>()
    private val notes = HashMap<String, Int>()
    private val approvals = HashMap<String, Int>()
    private val outcomes = HashMap<String, String>()
    private var seq = 0

    val waiting get() = approvals.keys.count { it !in outcomes }

    private fun put(item: Item): Int {
        items.add(item)
        return items.size - 1
    }

    private fun note(text: String, level: String, t: Double, key: String) {
        val i = notes[text]
        val seen = i?.let { items[it] as? Item.Note }
        if (seen != null && t - seen.t < REPEAT) {
            items[i] = seen.copy(n = seen.n + 1, t = if (t > 0) t else seen.t)
            return
        }
        notes[text] = put(Item.Note(key, text, level, 1, t))
    }

    /** A local line (like a failed send) that is not in the log. */
    fun local(text: String, level: String = "error") {
        activity = -1
        put(Item.Note("local${seq++}", text, level))
    }

    private fun keyOf(ev: JSONObject) = when (val id = ev.opt("id")) {
        // The event log numbers events; a session's transcript names them by uuid.
        is Number -> "e${id.toLong()}"
        is String -> "s$id"
        else -> "x${seq++}"
    }

    private fun daily(t: Double) {
        if (t > 0) {
            val d = dayOf(t, zone)
            if (d != day) {
                day = d
                put(Item.Day("d$d", t))
            }
        }
    }

    private fun settle(i: Int, word: String) {
        (items[i] as? Item.Worker)?.let { items[i] = it.copy(settled = word) }
    }

    /** An event of the trajectory, seen from the chat: one line per run, naming the shikigami reports first. */
    fun fold(ev: JSONObject) {
        val type = ev.str("type")
        if (type == "tool_result" || type == "approval_done") return
        daily(ev.optDouble("t", 0.0).let { if (it.isNaN()) 0.0 else it })
        val key = keyOf(ev)
        if (fold < 0) {
            reports.clear()
            notesSeen.clear()
            fold = put(Item.Fold("f$key", "", key))
            activity = -1
            between = true
        }
        val label = when (type) {
            "worker" -> "${ev.str("name")} ${ENDED[ev.str("event")] ?: ev.str("event")}"
            "system" -> ev.str("text").removeSuffix(".")
            "rate_limit" -> limitText(ev)
            // A reply of the user's turn that seimei marked trajectory:.
            "assistant" -> if (ev.optLong("re") > 0) "a reply to you" else ""
            else -> ""
        }
        val list = if (type == "worker") reports else notesSeen
        if (label.isNotEmpty() && label !in list) list.add(label)
        val all = reports + notesSeen
        val shown = all.take(maxOf(1, minOf(3, reports.size)))
        val text = if (all.isEmpty()) "seimei acted on its own"
            else shown.joinToString(", ") + if (all.size > shown.size) " and ${all.size - shown.size} more" else ""
        items[fold] = (items[fold] as Item.Fold).copy(text = text)
    }

    fun add(ev: JSONObject) {
        val type = ev.str("type")
        val t = ev.optDouble("t", 0.0).let { if (it.isNaN()) 0.0 else it }
        val key = keyOf(ev)
        if (type == "approval_done") {
            val aid = ev.str("aid")
            outcomes[aid] = ev.str("outcome")
            approvals[aid]?.let { i -> items[i] = (items[i] as Item.Approval).copy(outcome = outcomes[aid]) }
            return
        }
        if (type != "tool_result") fold = -1
        daily(t)
        // Each dispatcher turn in the trajectory opens with what started it.
        val turnId = ev.optLong("turn")
        if (lane == "t" && turnId > 0 && turnId != turn && type != "tool_result") {
            turn = turnId
            activity = -1
            val why = ev.str("why")
            put(Item.Trig("g$key", if (why.isEmpty()) "on its own" else "after $why"))
        } else if (lane == "t" && turnId == 0L) turn = 0
        // The part of an event turn that came to the chat (a picture, a reply for the user) says what started it.
        val fromId = ev.optLong("from")
        if (lane == "c" && fromId > 0 && fromId != from && type != "tool_result") {
            from = fromId
            activity = -1
            val why = ev.str("why")
            put(Item.Trig("from$key", if (why.isEmpty()) "on its own" else "after $why"))
        } else if (lane == "c" && fromId == 0L) from = 0
        // The summoned card already says it.
        if (type == "tool" && ev.str("name") == "mcp__seimei__summon") return
        if (type == "tool") {
            val call = Call(ev.str("tool_id"), ev.str("name"), ev.str("summary"))
            if (activity < 0) activity = put(Item.Activity(key, listOf(call)))
            else {
                val a = items[activity] as Item.Activity
                items[activity] = a.copy(calls = a.calls + call)
            }
            tools[call.id] = activity
            return
        }
        if (type == "tool_result") {
            val i = tools[ev.str("tool_id")] ?: return
            val a = items[i] as Item.Activity
            items[i] = a.copy(calls = a.calls.map {
                if (it.id == ev.str("tool_id") && it.running)
                    it.copy(running = false, error = ev.optBoolean("error"), out = ev.str("text").ifEmpty { "(no output)" })
                else it
            })
            return
        }
        activity = -1
        val id = ev.optLong("id")
        if (type == "user" || type == "assistant") {
            if (id > 0) said[id] = ev.str("text")
            val re = ev.optLong("re")
            val anchor = when {
                type == "user" && ev.str("quote").isNotEmpty() -> Anchor(anchorText(ev.str("quote")), "e${ev.optLong("quote_id")}", "t")
                // A reply far from what it answers says which message that was.
                type == "assistant" && re > 0 && (re != lastUser || between) && said.containsKey(re) -> Anchor(anchorText(said[re]!!), "e$re", "c")
                else -> null
            }
            if (type == "user") {
                lastUser = id
                between = false
            } else if (anchor != null) between = false
            val imgs = ev.optJSONArray("images")
            val files = ev.optJSONArray("files")
            put(Item.Msg(key, type, ev.str("text"), List(imgs?.length() ?: 0) { imgs!!.getString(it) }, t,
                List(files?.length() ?: 0) { files!!.getJSONObject(it).fileRef() }, anchor,
                if (lane == "t" && type == "assistant") id else 0))
            return
        }
        // What the turn itself shows (lane c: a summoned card, a picture) does not come between.
        if (ev.str("lane") != "c") between = true
        when (type) {
            "system" -> note(ev.str("text"), ev.str("level"), t, key)
            "worker" -> {
                val event = ev.str("event")
                val wid = ev.str("worker")
                if (event != "summoned") asks.remove(wid)?.let { (owner, i) ->
                    owner.settle(i, if (event == "replied" || event == "done") "Answered" else "Closed")
                }
                if (event == "replied") {
                    put(Item.Note(key, "You ${if (ev.optBoolean("asked")) "answered" else "messaged"} ${ev.str("name")}: ${ev.str("text")}", ""))
                } else if (event in WORKER_CARDS) {
                    if (event == "asking") asks[wid] = this to items.size
                    put(Item.Worker(key, event, ev.str("name"), ev.str("worker"), ev.str("text"), ev.str("model"), ev.str("cwd"),
                        ev.str("effort"), ev.str("context_cap")))
                } else {
                    val what = if (event == "stopped") "was stopped" else "was interrupted by a restart"
                    put(Item.Note(key, "${ev.str("name")} $what", ""))
                }
            }
            "image" -> put(Item.Shown(key, ev.str("file"), ev.str("caption"), ev.optInt("w"), ev.optInt("h")))
            // "file", not "id": the event log numbers every event with its own id.
            "file" -> put(Item.Sent(key, ev.fileRef(ev.str("file")), ev.str("caption"), t))
            "approval" -> {
                val aid = ev.str("aid")
                approvals[aid] = put(Item.Approval(key, aid, ev.str("who"), ev.str("action"), ev.str("target"),
                    ev.str("detail"), ev.optDouble("expires", 0.0), outcomes[aid]))
            }
            "rate_limit" -> put(Item.Note(key, limitText(ev), if (ev.str("status") == "rejected") "error" else "warn"))
        }
    }

    companion object {
        val WORKER_CARDS = setOf("summoned", "done", "failed", "asking")
    }
}

fun limitText(ev: JSONObject): String {
    val name = mapOf("five_hour" to "5-hour", "seven_day" to "Weekly")[ev.str("window")] ?: ev.str("window")
    val status = ev.str("status")
    val what = when (status) {
        "rejected" -> "limit reached"
        "allowed" -> "limit cleared"
        else -> "limit getting close"
    }
    val resets = ev.optDouble("resets_at", 0.0)
    return "$name $what" + if (resets > 0 && status != "allowed") ", resets ${clock(resets)}" else ""
}

fun clock(t: Double): String =
    java.time.format.DateTimeFormatter.ofPattern("HH:mm")
        .format(Instant.ofEpochMilli((t * 1000).toLong()).atZone(ZoneId.systemDefault()))

fun plural(n: Int, one: String, many: String) = if (n == 1) one else "$n $many"

/** "ran 3 commands", "read a file": what a group of tool calls did. */
fun phrase(name: String, n: Int): String = when {
    name == "Bash" -> "ran ${plural(n, "a command", "commands")}"
    name == "Read" -> "read ${plural(n, "a file", "files")}"
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> "edited ${plural(n, "a file", "files")}"
    name in setOf("Grep", "Glob") -> if (n == 1) "searched the files" else "searched $n times"
    name == "WebSearch" -> "searched the web"
    name == "WebFetch" -> "opened ${plural(n, "a page", "pages")}"
    name in setOf("Agent", "Task") -> "started ${plural(n, "an agent", "agents")}"
    name in setOf("TodoWrite", "TaskCreate", "TaskUpdate", "TaskList", "TaskGet") -> "updated its tasks"
    name == "ToolSearch" -> "loaded tools"
    name == "AskUserQuestion" -> if (n == 1) "asked you a question" else "asked you $n questions"
    name == "mcp__seimei__workers" -> "checked the shikigami"
    name == "mcp__seimei__peek" -> if (n == 1) "looked in on a shikigami" else "looked in on shikigami $n times"
    name == "mcp__seimei__message" -> if (n == 1) "messaged a shikigami" else "sent $n shikigami messages"
    name == "mcp__seimei__stop" -> "stopped a shikigami"
    name == "mcp__seimei__show_image" -> if (n == 1) "showed you an image" else "showed you $n images"
    name == "mcp__seimei__send_file" -> if (n == 1) "sent you a file" else "sent you $n files"
    name == "mcp__seimei__browser_tabs" -> "looked at your tabs"
    name == "mcp__seimei__browser_read" -> if (n == 1) "read a page" else "read $n page parts"
    name == "mcp__seimei__browser_open" -> if (n == 1) "opened a tab" else "opened $n tabs"
    name == "mcp__seimei__browser_switch" -> "switched tabs"
    name == "mcp__seimei__browser_close" -> "closed tabs"
    name == "mcp__seimei__browser_download" -> if (n == 1) "started a download" else "started $n downloads"
    name == "mcp__seimei__browser_downloads" -> "checked the downloads"
    name == "mcp__seimei__discord_guilds" -> "looked at your servers"
    name == "mcp__seimei__discord_dms" -> "looked at your DMs"
    name == "mcp__seimei__discord_read" -> if (n == 1) "read Discord" else "read Discord $n times"
    name == "mcp__seimei__discord_search" -> if (n == 1) "searched Discord" else "searched Discord $n times"
    name == "mcp__seimei__discord_send" -> if (n == 1) "sent a Discord message" else "sent $n Discord messages"
    name == "mcp__seimei__discord_edit" -> if (n == 1) "edited a Discord message" else "edited $n Discord messages"
    name == "mcp__seimei__discord_delete" -> if (n == 1) "deleted a Discord message" else "deleted $n Discord messages"
    name == "mcp__seimei__discord_react" -> if (n == 1) "reacted on Discord" else "reacted $n times on Discord"
    name == "mcp__seimei__discord_mark_read" -> if (n == 1) "marked a channel read" else "marked $n channels read"
    else -> {
        val short = if (name.startsWith("mcp__")) name.split("__").last().replace('_', ' ') else name
        if (n == 1) "used $short" else "used $short $n times"
    }
}

/** What a call that still runs does: "running a command". */
fun doingPhrase(name: String): String = when {
    name == "Bash" -> "running a command"
    name == "Read" -> "reading a file"
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> "editing a file"
    name in setOf("Grep", "Glob") -> "searching the files"
    name == "WebSearch" -> "searching the web"
    name == "WebFetch" -> "opening a page"
    name in setOf("Agent", "Task") -> "running an agent"
    else -> "using " + if (name.startsWith("mcp__")) name.split("__").last().replace('_', ' ') else name
}

/** The calls done so far, then the one still running (live) as what it does now. */
fun activityLabel(calls: List<Call>, live: String? = null): String {
    val counts = LinkedHashMap<String, Int>()
    for (c in calls) if (c.id != live) counts[c.name] = (counts[c.name] ?: 0) + 1
    val parts = counts.entries.map { phrase(it.key, it.value) } + listOfNotNull(calls.firstOrNull { it.id == live }?.let { doingPhrase(it.name) })
    return parts.joinToString(", ").replaceFirstChar { it.uppercase() }
}

/** How long something has run: 42s, 4m 12s, 1h 5m. */
fun elapsed(since: Double, now: Long = System.currentTimeMillis()): String {
    val s = maxOf(0L, (now / 1000.0 - since).toLong())
    return when {
        s < 60 -> "${s}s"
        s < 3600 -> "${s / 60}m ${s % 60}s"
        else -> "${s / 3600}h ${s % 3600 / 60}m"
    }
}

fun home(path: String) = path.replace(Regex("^/home/[^/]+"), "~")

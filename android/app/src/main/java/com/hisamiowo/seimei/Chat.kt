package com.hisamiowo.seimei

import org.json.JSONObject
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

/** One row of the chat, built from logged events (a port of the web page's Renderer). */
sealed interface Item {
    val key: String

    data class Day(override val key: String, val t: Double) : Item
    data class Msg(override val key: String, val role: String, val text: String, val images: List<String>, val t: Double,
                   val files: List<FileRef> = emptyList()) : Item
    data class Note(override val key: String, val text: String, val level: String, val n: Int = 1, val t: Double = 0.0) : Item
    data class Activity(override val key: String, val calls: List<Call>) : Item
    data class Worker(
        override val key: String, val event: String, val name: String, val worker: String,
        val text: String, val model: String, val cwd: String,
    ) : Item
    data class Approval(
        override val key: String, val aid: String, val who: String, val action: String, val target: String,
        val detail: String, val expires: Double, val outcome: String?,
    ) : Item
    data class Shown(override val key: String, val file: String, val caption: String) : Item
    /** A file seimei handed over to download. */
    data class Sent(override val key: String, val file: FileRef, val caption: String, val t: Double = 0.0) : Item
}

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

class Chat(private val zone: ZoneId = ZoneId.systemDefault()) {
    val items = ArrayList<Item>()
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

    fun add(ev: JSONObject) {
        val type = ev.str("type")
        val t = ev.optDouble("t", 0.0).let { if (it.isNaN()) 0.0 else it }
        val key = if (ev.has("id")) "e${ev.getLong("id")}" else "x${seq++}"
        if (type == "approval_done") {
            val aid = ev.str("aid")
            outcomes[aid] = ev.str("outcome")
            approvals[aid]?.let { i -> items[i] = (items[i] as Item.Approval).copy(outcome = outcomes[aid]) }
            return
        }
        if (t > 0) {
            val d = dayOf(t, zone)
            if (d != day) {
                day = d
                put(Item.Day("d$d", t))
            }
        }
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
        when (type) {
            "user", "assistant" -> {
                val imgs = ev.optJSONArray("images")
                val files = ev.optJSONArray("files")
                put(Item.Msg(key, type, ev.str("text"), List(imgs?.length() ?: 0) { imgs!!.getString(it) }, t,
                    List(files?.length() ?: 0) { files!!.getJSONObject(it).fileRef() }))
            }
            "system" -> note(ev.str("text"), ev.str("level"), t, key)
            "worker" -> {
                val event = ev.str("event")
                if (event in WORKER_CARDS) {
                    put(Item.Worker(key, event, ev.str("name"), ev.str("worker"), ev.str("text"), ev.str("model"), ev.str("cwd")))
                } else {
                    val what = if (event == "stopped") "was stopped" else "was interrupted by a restart"
                    put(Item.Note(key, "${ev.str("name")} $what", ""))
                }
            }
            "image" -> put(Item.Shown(key, ev.str("file"), ev.str("caption")))
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

fun activityLabel(calls: List<Call>): String {
    val counts = LinkedHashMap<String, Int>()
    for (c in calls) counts[c.name] = (counts[c.name] ?: 0) + 1
    val text = counts.entries.joinToString(", ") { phrase(it.key, it.value) }
    return text.replaceFirstChar { it.uppercase() }
}

fun home(path: String) = path.replace(Regex("^/home/[^/]+"), "~")

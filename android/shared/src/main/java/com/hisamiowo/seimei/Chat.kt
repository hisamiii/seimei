package com.hisamiowo.seimei

import org.json.JSONArray
import org.json.JSONObject
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

/** One row of the chat, built from logged events (a port of the web page's Renderer). */
sealed interface Item {
    val key: String

    data class Day(override val key: String, val t: Double) : Item
    /** anchor: the message this one answers, when something sits between them.
     * reply: the event id that "Reply in chat" answers (a message of seimei's in the trajectory), 0 for none.
     * thread and threadName: the side thread a message of the user's came from.
     * sender: who wrote a user turn that is not the user's ("seimei" in a thread). offer: seimei offers to take this reply to a side thread. */
    data class Msg(override val key: String, val role: String, val text: String, val images: List<String>, val t: Double,
                   val files: List<FileRef> = emptyList(), val anchor: Anchor? = null, val reply: Long = 0,
                   val thread: String = "", val threadName: String = "", val sender: String = "", val offer: Boolean = false) : Item
    /** A voice call's turn of the user's (their bubble with a wave mark) or of the partner's (a line with a bar), cut when the user barged in or hung up. */
    data class Spoken(val key: String, val who: String, val text: String, val cut: Boolean = false)
    /** A voice call as one card: t0 and t1 are its first and last turn, secs its length once it [ended] (-1 before), with the summary when there is one.
     * The turns open on a tap; last is the partner's latest line, shown while the call runs. */
    data class Voice(override val key: String, val t0: Double, val t1: Double, val turns: List<Spoken> = emptyList(),
                    val last: String = "", val secs: Long = -1, val summary: String = "", val ended: Boolean = false) : Item
    /** The voice partner asked seimei; [waiting] until seimei's reply lands under it. */
    data class Asked(override val key: String, val text: String, val waiting: Boolean = true) : Item
    /** A run of trajectory events, as one line in the chat; goto is the key of the first one. */
    data class Fold(override val key: String, val text: String, val goto: String) : Item
    /** What started a dispatcher turn in the trajectory: "after app-tester finished". */
    data class Trig(override val key: String, val text: String) : Item
    data class Note(override val key: String, val text: String, val level: String, val n: Int = 1, val t: Double = 0.0) : Item
    data class Activity(override val key: String, val calls: List<Call>) : Item
    data class Worker(
        override val key: String, val event: String, val name: String, val worker: String,
        val text: String, val model: String, val cwd: String, val effort: String = "",
        /** An "asks" card that something later settled: "Answered", "Done" (the user marked it) or "Closed". */
        val settled: String? = null,
    ) : Item
    /** A card: Approve/Deny from seimei's own tools and plugins, or (with options) a permission prompt of the agent's.
     * expires is 0 for a shikigami's permission card, which waits. done is the approval_done event once settled. */
    data class Approval(
        override val key: String, val aid: String, val who: String, val action: String, val target: String,
        val detail: String, val expires: Double, val outcome: String?,
        val options: List<Opt> = emptyList(), val sub: String = "", val subagent: Boolean = false, val done: Done? = null,
    ) : Item
    /** w and h: the picture's size from the server (0 when unknown), so its space is kept before it loads.
     * from and worker: the shikigami or session that showed it, empty for seimei's own. */
    data class Shown(override val key: String, val file: String, val caption: String, val w: Int = 0, val h: Int = 0,
        val from: String = "", val worker: String = "") : Item
    /** A file seimei, a shikigami or a session handed over to download. */
    data class Sent(override val key: String, val file: FileRef, val caption: String, val t: Double = 0.0,
        val from: String = "", val worker: String = "") : Item
}

/** One choice on a permission card: kind once, rule, dir, mode or deny; a rule option carries its rule and where. */
data class Opt(val id: String, val label: String, val kind: String, val rule: String, val where: String, val broad: Boolean)

/** How a card was settled: what was picked (a rule), the note sent with a deny, "chat" when a message settled it. */
data class Done(val outcome: String, val chosen: String, val note: String, val by: String)

/** What the next message answers: an event id, the composer's quote and the lane the event shows in. */
data class ReplyTo(val id: Long, val text: String, val lane: String)

private val CARD_VERB = mapOf("done" to "finished", "failed" to "failed", "asking" to "asks")

/** A row the user can reply to (seimei's messages, report, question and approval cards), as the reply; else null. */
fun Item.quote(lane: String): ReplyTo? {
    val id = key.removePrefix("e").toLongOrNull() ?: return null
    return when (this) {
        is Item.Msg -> if (role == "assistant") ReplyTo(id, text, lane) else null
        is Item.Worker -> CARD_VERB[event]?.let { verb ->
            ReplyTo(id, "$name $verb: ${(text.lineSequence().firstOrNull { it.isNotBlank() } ?: "").replace(Regex("^[#>*\\-\\s]+"), "").trim()}", lane)
        }
        is Item.Approval -> ReplyTo(id, "$who wants to $target", lane)
        else -> null
    }
}

/** A chip quoting another message; a tap goes to it (lane "c": the chat, "t": the trajectory). */
data class Anchor(val text: String, val goto: String, val lane: String)

fun anchorText(text: String) = text.replace(Regex("\\s+"), " ").trim().let { if (it.length > 60) it.take(60) + "…" else it }

/** Events in lane "t" happened on their own (events.rs): the trajectory gets them whole, the chat a fold line. */
fun JSONObject.inTraj() = str("lane") == "t"


/** Shikigami id -> its open "asks" card, wherever it was drawn: a report in the trajectory settles a card in the chat. */
class Asks : HashMap<String, Pair<Chat, Int>>()

private val ENDED = mapOf("done" to "finished", "failed" to "failed", "stopped" to "was stopped", "interrupted" to "was interrupted")

/** A file stored on the server (uploads.rs), downloaded only when tapped. */
data class FileRef(val id: String, val name: String, val size: Long, val kind: String)

fun JSONObject.fileRef(id: String = str("id")) = FileRef(id, str("name"), optLong("size"), str("kind"))

/** The server deletes files after this long (uploads.rs KEEP_DAYS); older chips show as expired. */
const val KEEP_SECONDS = 30 * 86400.0

fun expired(t: Double, now: Double = System.currentTimeMillis() / 1000.0) = t > 0 && now - t > KEEP_SECONDS

fun sizeText(n: Long) = if (n >= 1 shl 20) "%.1f MB".format(n / 1048576.0) else "${maxOf(1L, Math.round(n / 1024.0))} KB"

data class Call(val id: String, val name: String, val summary: String, val running: Boolean = true,
                val error: Boolean = false, val out: String = "")

/** A system line repeated within this many seconds counts up on the first one instead of showing again. */
const val REPEAT = 900.0

/** Seconds after a card settles in which trajectory events get no fold line under it, as on the web. */
const val AFTER_CARD = 300.0

fun dayOf(t: Double, zone: ZoneId = ZoneId.systemDefault()): LocalDate =
    Instant.ofEpochMilli((t * 1000).toLong()).atZone(zone).toLocalDate()

fun JSONObject.str(k: String) = optString(k, "").let { if (it == "null") "" else it }

fun JSONArray?.objects(): List<JSONObject> = if (this == null) emptyList() else List(length()) { getJSONObject(it) }

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
    private val outcomes = HashMap<String, Done>()
    private val doneAt = HashMap<String, Double>()
    /** The approval card drawn last, if nothing came after it, and when it settled (0 while it waits). */
    private var card: Pair<String, Double>? = null
    /** Where each message sits in items, by event id: an offer marks the reply it is about. */
    private val msgAt = HashMap<Long, Int>()
    private var seq = 0
    // Voice: where the open call's card sits in items, and the partner's open asks to seimei by event id.
    private var call = -1
    /** The ended calls' cards by their voice_end's id: seimei's voice_summary names one. */
    private val calls = HashMap<Long, Int>()
    private val asked = HashMap<Long, Int>()
    /** The user's words in a direct call, by event id -> where the call's card sits: seimei's replies to them go in the card too. */
    private val spoken = HashMap<Long, Int>()

    val waiting get() = approvals.keys.count { it !in outcomes }

    /** What the last item says happened ("dev finished"): a trigger line right after it that repeats it is left out, as on the web. */
    private var lastSaid: String? = null

    private fun put(item: Item): Int {
        items.add(item)
        lastSaid = null
        card = null
        return items.size - 1
    }

    private fun trig(key: String, why: String) {
        if (why.isNotEmpty() && why == lastSaid) return
        put(Item.Trig(key, if (why.isEmpty()) "on its own" else "after $why"))
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

    /** An event of the trajectory, seen from the chat: one line per run, naming the shikigami reports first.
     * A run with nothing to name (seimei handling the user's sessions on its own, say) leaves no line: the trajectory has it. */
    fun fold(ev: JSONObject) {
        val type = ev.str("type")
        if (type == "tool_result" || type == "approval_done") return
        // What a settled card set off right after it stays in the trajectory: the card's state line says enough.
        card?.let { (_, done) -> if (done > 0 && ev.optDouble("t", 0.0) - done <= AFTER_CARD) return }
        val label = when (type) {
            "worker" -> "${ev.str("name")} ${ENDED[ev.str("event")] ?: ev.str("event")}"
            "system" -> noteText(ev).removeSuffix(".")
            "rate_limit" -> limitText(ev)
            // A reply of the user's turn that seimei marked trajectory:.
            "assistant" -> if (ev.optLong("re") > 0) "a reply to you" else ""
            else -> ""
        }
        if (label.isEmpty() && fold < 0) return
        daily(ev.optDouble("t", 0.0).let { if (it.isNaN()) 0.0 else it })
        val key = keyOf(ev)
        if (fold < 0) {
            reports.clear()
            notesSeen.clear()
            fold = put(Item.Fold("f$key", "", key))
            activity = -1
            between = true
        }
        val list = if (type == "worker") reports else notesSeen
        if (label.isNotEmpty() && label !in list) list.add(label)
        val all = reports + notesSeen
        val shown = all.take(maxOf(1, minOf(3, reports.size)))
        val text = shown.joinToString(", ") + if (all.size > shown.size) "${if (shown.size > 1) "," else ""} and ${all.size - shown.size} more" else ""
        items[fold] = (items[fold] as Item.Fold).copy(text = text)
    }

    /** A voice call is one card where it starts, with its turns inside; what the partner asked seimei stays in the chat, as the reply that answers it does.
     * Turns more than 5 minutes apart are another call; a voice_end closes the card with the call's length, and a voice_summary from seimei fills in its summary. */
    private fun voice(ev: JSONObject, key: String, t: Double) {
        if (ev.str("type") == "voice_summary") {
            calls[ev.optLong("call")]?.let { i -> (items[i] as? Item.Voice)?.let { items[i] = it.copy(summary = ev.str("summary")) } }
            return
        }
        val open = (items.getOrNull(call) as? Item.Voice)?.takeIf { !it.ended }
        if (ev.str("type") == "voice_end") {
            // It closes the open card, however long the call stayed silent; with no card open, its turns are on an older page.
            if (open != null) {
                items[call] = open.copy(secs = ev.optLong("secs"), ended = true)
                calls[ev.optLong("id")] = call
            }
            return
        }
        val c = if (open != null && t - open.t1 <= 300) open else Item.Voice("v$key", t, t).also { call = put(it) }
        between = false
        if (ev.str("type") == "user" && ev.optBoolean("spoken")) {
            // The user talked with seimei itself: the words and their replies stay in the card.
            val id = ev.optLong("id")
            lastUser = id
            if (id > 0) said[id] = ev.str("text")
            spoken[id] = call
            items[call] = c.copy(t1 = t, turns = c.turns + Item.Spoken(key, "user", ev.str("text")))
            return
        }
        if (ev.str("type") == "user") {
            items[call] = c.copy(t1 = t)
            // The partner asked seimei: the reply that follows answers it, with no chip.
            val id = ev.optLong("id")
            lastUser = id
            if (id > 0) said[id] = ev.str("text")
            asked[id] = put(Item.Asked(key, ev.str("text")))
            return
        }
        val turn = Item.Spoken(key, ev.str("who"), ev.str("text"), ev.optBoolean("cut"))
        items[call] = c.copy(t1 = t, turns = c.turns + turn, last = if (turn.who == "partner") turn.text else c.last)
    }

    fun add(ev: JSONObject) {
        val type = ev.str("type")
        val t = ev.optDouble("t", 0.0).let { if (it.isNaN()) 0.0 else it }
        val key = keyOf(ev)
        if (type == "approval_done") {
            val aid = ev.str("aid")
            val done = Done(ev.str("outcome"), ev.str("chosen"), ev.str("note"), ev.str("by"))
            outcomes[aid] = done
            doneAt[aid] = if (t > 0) t else System.currentTimeMillis() / 1000.0
            if (card?.first == aid) card = aid to doneAt.getValue(aid)
            approvals[aid]?.let { i -> items[i] = (items[i] as Item.Approval).copy(outcome = done.outcome, done = done) }
            return
        }
        if (type == "offer") {
            msgAt[ev.optLong("re")]?.let { i -> (items[i] as? Item.Msg)?.let { items[i] = it.copy(offer = true) } }
            return
        }
        if (type != "tool_result") fold = -1
        daily(t)
        // Each dispatcher turn in the trajectory opens with what started it.
        val turnId = ev.optLong("turn")
        if (lane == "t" && turnId > 0 && turnId != turn && type != "tool_result") {
            turn = turnId
            activity = -1
            trig("g$key", ev.str("why"))
        } else if (lane == "t" && turnId == 0L) turn = 0
        // The part of an event turn that came to the chat (a picture, a reply for the user) says what started it.
        val fromId = ev.optLong("from")
        if (lane == "c" && fromId > 0 && fromId != from && type != "tool_result") {
            from = fromId
            activity = -1
            trig("from$key", ev.str("why"))
        } else if (lane == "c" && fromId == 0L) from = 0
        // The summoned card already says it; a question shows as its own card (as on the web).
        if (type == "tool" && ev.str("name") in setOf("mcp__seimei__summon", "AskUserQuestion")) return
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
                    it.copy(running = false, error = ev.optBoolean("error"), out = ev.str("text").ifEmpty { "No output" })
                else it
            })
            return
        }
        activity = -1
        val id = ev.optLong("id")
        if (type == "voice" || type == "voice_end" || type == "voice_summary" || (type == "user" && ev.optBoolean("voice"))) return voice(ev, key, t)
        // seimei's spoken reply in a direct call: a line in the call's card.
        val inCall = spoken[ev.optLong("re")]?.takeIf { type == "assistant" && ev.str("text").isNotEmpty() }
        val callCard = inCall?.let { items.getOrNull(it) as? Item.Voice }
        if (inCall != null && callCard != null) {
            if (id > 0) said[id] = ev.str("text")
            items[inCall] = callCard.copy(turns = callCard.turns + Item.Spoken(key, "partner", ev.str("text")), last = ev.str("text"))
            return
        }
        if (type == "user" || type == "assistant") {
            if (id > 0) said[id] = ev.str("text")
            val re = ev.optLong("re")
            val anchor = when {
                type == "user" && ev.str("quote").isNotEmpty() ->
                    Anchor(anchorText(ev.str("quote")), "e${ev.optLong("quote_id")}", ev.str("quote_lane").ifEmpty { "t" })
                // A reply far from what it answers says which message that was.
                // re_pin: seimei named the message (re:), so the chip shows even right after it.
                type == "assistant" && re > 0 && (ev.optBoolean("re_pin") || re != lastUser || between)
                    && (said.containsKey(re) || ev.str("re_text").isNotEmpty()) ->
                    Anchor(anchorText(said[re] ?: ev.str("re_text")), "e$re", "c")
                else -> null
            }
            if (type == "user") {
                lastUser = id
                between = false
            } else if (anchor != null) between = false
            val imgs = ev.optJSONArray("images")
            val files = ev.optJSONArray("files")
            val at = put(Item.Msg(key, type, ev.str("text"), List(imgs?.length() ?: 0) { imgs!!.getString(it) }, t,
                List(files?.length() ?: 0) { files!!.getJSONObject(it).fileRef() }, anchor,
                if (lane == "t" && type == "assistant") id else 0,
                thread = ev.str("thread"), threadName = ev.str("thread_name"),
                // "from" is a number on seimei's events (the turn's trigger), a name on a thread's
                sender = (ev.opt("from") as? String).orEmpty()))
            if (id > 0) msgAt[id] = at
            if (type == "assistant") asked.remove(re)?.let { i -> items[i] = (items[i] as Item.Asked).copy(waiting = false) }
            return
        }
        // What the turn itself shows (lane c: a summoned card, a picture) does not come between.
        if (ev.str("lane") != "c") between = true
        when (type) {
            "system" -> note(noteText(ev), ev.str("level"), t, key)
            "worker" -> {
                val event = ev.str("event")
                val wid = ev.str("worker")
                if (event != "summoned") asks.remove(wid)?.let { (owner, i) ->
                    owner.settle(i, when {
                        event == "settled" && ev.str("by") == "user" -> "Done"
                        event in setOf("replied", "done", "settled") -> "Answered"
                        else -> "Closed"
                    })
                }
                if (event == "replied" || event == "settled") {
                    Unit
                } else if (event in WORKER_CARDS) {
                    if (event == "asking") asks[wid] = this to items.size
                    put(Item.Worker(key, event, ev.str("name"), ev.str("worker"), ev.str("text"), ev.str("model"), ev.str("cwd"),
                        ev.str("effort")))
                    lastSaid = "${ev.str("name")} ${ENDED[event] ?: event}"
                } else {
                    val what = if (event == "stopped") "was stopped" else "was interrupted by a restart"
                    put(Item.Note(key, "${ev.str("name")} $what", ""))
                }
            }
            "image" -> put(Item.Shown(key, ev.str("file"), ev.str("caption"), ev.optInt("w"), ev.optInt("h"), ev.str("from"), ev.str("worker")))
            // "file", not "id": the event log numbers every event with its own id.
            "file" -> put(Item.Sent(key, ev.fileRef(ev.str("file")), ev.str("caption"), t, ev.str("from"), ev.str("worker")))
            "approval" -> {
                val aid = ev.str("aid")
                approvals[aid] = put(Item.Approval(key, aid, ev.str("who"), ev.str("action"), ev.str("target"),
                    ev.str("detail"), ev.optDouble("expires", 0.0).let { if (it.isNaN()) 0.0 else it }, outcomes[aid]?.outcome,
                    ev.optJSONArray("options")?.let { a -> (0 until a.length()).map { a.getJSONObject(it) }.map { o ->
                        Opt(o.str("id"), o.str("label"), o.str("kind"), o.str("rule"), o.str("where").ifEmpty { o.str("sub") }, o.optBoolean("broad")) } }
                        ?: emptyList(),
                    ev.str("sub"), ev.optBoolean("subagent"), outcomes[aid]))
                card = aid to (doneAt[aid] ?: 0.0)
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
    // A warning comes when usage runs ahead of pace for the window, at any level, so it says how much is used.
    val used = if (ev.has("utilization") && !ev.isNull("utilization"))
        "${Math.round(ev.optDouble("utilization") * 100)}% used, " else ""
    val what = when (status) {
        "rejected" -> "limit reached"
        "allowed" -> "limit cleared"
        else -> "${used}ahead of pace"
    }
    val resets = ev.optDouble("resets_at", 0.0)
    return "$name $what" + if (resets > 0 && status != "allowed") ", resets ${resetsAt(resets)}" else ""
}

/** "16:00" today, "thu 16:00" on another day. */
fun resetsAt(t: Double): String {
    val at = Instant.ofEpochMilli((t * 1000).toLong()).atZone(ZoneId.systemDefault())
    if (at.toLocalDate() == java.time.LocalDate.now()) return clock(t)
    return at.dayOfWeek.getDisplayName(java.time.format.TextStyle.SHORT, java.util.Locale.ENGLISH).lowercase() +
        " " + clock(t)
}

// A server note's times in this device's clock: "clock" pairs each time with its text in the note.
fun noteText(ev: JSONObject): String {
    var text = ev.str("text")
    val pairs = ev.optJSONArray("clock") ?: return text
    for (i in 0 until pairs.length()) pairs.optJSONArray(i)?.let { text = text.replace(it.optString(1), clock(it.optDouble(0))) }
    return text
}

// The locale's short time, as the web page shows it ("6:13 PM" in English)
/** A voice call's length on its card: "6 min", at least 1. */
fun callMinutes(secs: Double) = "${maxOf(1L, Math.round(secs / 60))} min"

fun clock(t: Double): String =
    java.time.format.DateTimeFormatter.ofLocalizedTime(java.time.format.FormatStyle.SHORT)
        .format(Instant.ofEpochMilli((t * 1000).toLong()).atZone(ZoneId.systemDefault())).replace('\u202f', ' ')

fun plural(n: Int, one: String, many: String) = if (n == 1) one else "$n $many"

/** The built-in plugins' tools (mcp__browser__browser_tabs) read like seimei's own, as they were named before them. */
private val PLUGIN_TOOL = Regex("^mcp__(desktop|browser|music|queue)__")

/** "ran 3 commands", "read a file": what a group of tool calls did. */
fun phrase(tool: String, n: Int): String = phraseOf(PLUGIN_TOOL.replace(tool, "mcp__seimei__"), n)

private fun phraseOf(name: String, n: Int): String = when {
    name == "Bash" -> "ran ${plural(n, "a command", "commands")}"
    name == "Read" -> "read ${plural(n, "a file", "files")}"
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> "edited ${plural(n, "a file", "files")}"
    name in setOf("Grep", "Glob") -> if (n == 1) "searched the files" else "searched $n times"
    name == "WebSearch" -> "searched the web"
    name == "WebFetch" -> "opened ${plural(n, "a page", "pages")}"
    name in setOf("Agent", "Task") -> "started ${plural(n, "a subagent", "subagents")}"
    name in setOf("TodoWrite", "TaskCreate", "TaskUpdate", "TaskList", "TaskGet") -> "updated its steps"
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
    name in setOf("Agent", "Task") -> "running a subagent"
    else -> "using " + if (name.startsWith("mcp__")) name.split("__").last().replace('_', ' ') else name
}

/** The calls done so far, then the one still running (live, or not yet streaming) as what it does now. */
fun activityLabel(calls: List<Call>, live: String? = null): String {
    val now = live ?: calls.lastOrNull { it.running }?.id
    val counts = LinkedHashMap<String, Int>()
    for (c in calls) if (c.id != now) counts[c.name] = (counts[c.name] ?: 0) + 1
    val parts = counts.entries.map { phrase(it.key, it.value) } + listOfNotNull(calls.firstOrNull { it.id == now }?.let { doingPhrase(it.name) })
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

package com.hisamiowo.seimei

import java.io.File
import java.io.IOException
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * Conversations kept on the phone, so the chat and every agent page show at once (after a cold start too) and only
 * fetch what is new. Each is a file of its raw events (the last [keep]) and where to go on from: the chat and each
 * page are drawn from these again, as from the server. App-private and never backed up; unpairing clears it.
 */
class Store(dir: File) {
    private val root = File(dir, "conversations")

    fun read(key: String): JSONObject? = try {
        JSONObject(File(root, "$key.json").readText())
    } catch (e: IOException) {
        null
    } catch (e: JSONException) {
        null
    }

    /** Writes [data] whole (a crash mid-write leaves the old copy), then keeps only the newest [PAGES] pages. */
    fun write(key: String, data: String) {
        try {
            root.mkdirs()
            val tmp = File(root, "$key.tmp")
            tmp.writeText(data)
            tmp.renameTo(File(root, "$key.json"))
        } catch (e: IOException) {
            return
        }
        if (key != CHAT && key != TASKS) {
            val pages = root.listFiles { f -> f.name.endsWith(".json") && f.name != "$CHAT.json" && f.name != "$TASKS.json" }.orEmpty()
            pages.sortedByDescending { it.lastModified() }.drop(PAGES).forEach { it.delete() }
        }
    }

    fun drop(key: String) {
        File(root, "$key.json").delete()
    }

    fun clear() {
        root.listFiles()?.forEach { it.delete() }
    }

    companion object {
        const val CHAT = "chat"
        /** The task board, whole, as the server last sent it. */
        const val TASKS = "tasks"
        /** Events the chat keeps; older ones load from the server when scrolled to, as before. */
        const val CHAT_KEEP = 300
        /** Events a page keeps: what the server sends for a page at most. */
        const val PAGE_KEEP = 400
        /** Agent pages kept, the most recently opened ones. */
        const val PAGES = 40

        fun pageKey(kind: String, id: String) = "$kind-$id"

        /** The last [n] of [events], as stored. */
        fun tail(events: List<JSONObject>, n: Int): JSONArray = JSONArray().also { a -> events.takeLast(n).forEach { a.put(it) } }

        /**
         * What of [new] [have] does not hold yet: an event with an id already held is skipped, so a delta that overlaps
         * the stream does no harm. Events without an id (a session's tool results) always count as new.
         */
        fun fresh(have: List<JSONObject>, new: List<JSONObject>): List<JSONObject> {
            val ids = have.mapNotNullTo(HashSet()) { it.opt("id")?.toString() }
            return new.filter { e -> e.opt("id")?.toString()?.let(ids::add) ?: true }
        }
    }
}

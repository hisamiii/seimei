package com.hisamiowo.seimei

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

class TasksTest {
    private fun task(id: String, project: String, owner: String, state: String, pos: Int = 0, doneAt: Double? = null) =
        JSONObject().put("id", id).put("title", "t $id").put("project", project).put("owner", owner).put("state", state)
            .put("done_at", doneAt ?: JSONObject.NULL).put("worker", JSONObject.NULL).put("pos", pos)

    private fun project(name: String, paused: Boolean = false) = JSONObject().put("name", name).put("color", JSONObject.NULL).put("paused", paused)

    private val raw = JSONObject()
        .put("projects", JSONArray(listOf(project("held", true), project("mine"), project("work"), project("busy"), project("asky"), project("over"), project("empty"))))
        .put("tasks", JSONArray(listOf(
            task("t1", "held", "mc", "open"),
            task("t2", "mine", "you", "open", 1),
            task("t3", "mine", "you", "open", 2),
            task("#3", "work", "dev", "open", 3),
            task("#4", "busy", "dev", "open", 2),
            task("#5", "busy", "kernel", "running", 1).put("steps", JSONArray(listOf(
                JSONObject().put("id", "1").put("title", "a").put("state", "completed"),
                JSONObject().put("id", "2").put("title", "b").put("state", "in_progress"),
                JSONObject().put("id", "3").put("title", "c").put("state", "pending")))),
            task("#6", "busy", "dev", "done", doneAt = 10.0),
            task("#7", "busy", "dev", "done", doneAt = 20.0),
            task("#8", "over", "dev", "done", doneAt = 5.0),
            task("#9", "asky", "gentoo", "asks", 1),
            task("#10", "asky", "gentoo", "review", 2),
        )))

    @Test
    fun projectsInOrder() {
        val b = board(raw)
        // A question first, then running work, then the rest in the server's order.
        // Projects without tasks drop out; those with only done items fold into the Archived row.
        assertEquals(listOf("asky", "busy", "held", "mine", "work"), b.projects.map { it.name })
        assertEquals(listOf("over"), b.folded)
        assertEquals(3, b.done.size)
        val busy = b.projects.first { it.name == "busy" }
        assertEquals(listOf("#5", "#4"), busy.live.map { it.id })
        // A timeline: what waits for a check before the current item
        assertEquals(listOf("#10", "#9"), b.projects.first { it.name == "asky" }.live.map { it.id })
        assertEquals(listOf("#7", "#6"), busy.done.map { it.id })
    }

    @Test
    fun staleParkedItems() {
        val at = 1_000_000.0
        val old = at - 4.5 * 86400
        // Paused, or live in a paused project, and untouched 3+ days: flagged with the days; fresh or running ones not
        assertEquals(4, task(task("a", "work", "dev", "paused").put("since", old).put("updated", old), at = at).stale)
        assertEquals(4, task(task("b", "held", "mc", "open").put("since", old).put("updated", old), setOf("held"), at).stale)
        assertEquals(0, task(task("c", "work", "dev", "paused").put("since", old).put("updated", at - 3600), at = at).stale)
        assertEquals(0, task(task("d", "work", "dev", "running").put("since", old).put("updated", old), at = at).stale)
        val held = JSONObject(raw.toString())
        held.getJSONArray("tasks").getJSONObject(0).put("since", 1.0).put("updated", 1.0)
        assertEquals("untouched", summary(board(held).projects.first { it.name == "held" }).last().first.substringBefore(" "))
    }

    @Test
    fun summaries() {
        val b = board(raw)
        fun s(name: String) = summary(b.projects.first { it.name == name })
        assertEquals(listOf("1 question" to "warn", "1 review" to "warn"), s("asky"))
        assertEquals(listOf("1 running" to "run", "1 open" to ""), s("busy"))
        assertEquals(listOf("2 for you" to "warn"), s("mine"))
    }

    @Test
    fun nowView() {
        val n = now(board(raw))
        assertEquals(listOf("#9"), n.asks.map { it.id })
        assertEquals(listOf("#10"), n.review.map { it.id })
        assertEquals(listOf("t2", "t3"), n.mine.map { it.id })
        assertEquals(4, n.needs)
        assertEquals(listOf("#5"), n.running.map { it.id })
        assertEquals("b", n.running[0].current)
        assertEquals(1, n.running[0].stepsDone)
        // One next per shikigami, never a paused project's
        assertEquals(listOf("#4"), n.next.map { it.id })
    }
}

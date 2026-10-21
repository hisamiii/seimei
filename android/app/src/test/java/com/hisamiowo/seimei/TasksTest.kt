package com.hisamiowo.seimei

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

class TasksTest {
    private fun task(id: String, project: String, owner: String, state: String, doneAt: Double? = null) =
        JSONObject().put("id", id).put("title", "t $id").put("project", project).put("owner", owner).put("state", state)
            .put("done_at", doneAt ?: JSONObject.NULL).put("worker", JSONObject.NULL)

    private fun project(name: String, paused: Boolean = false) = JSONObject().put("name", name).put("color", JSONObject.NULL).put("paused", paused)

    private val board = JSONObject()
        .put("projects", JSONArray(listOf(project("held", true), project("mine"), project("work"), project("busy"), project("over"), project("empty"))))
        .put("tasks", JSONArray(listOf(
            task("t1", "held", "mc", "paused"),
            task("t2", "mine", "you", "open"),
            task("#3", "work", "dev", "open"),
            task("#4", "busy", "dev", "open"),
            task("#5", "busy", "dev", "running"),
            task("#6", "busy", "dev", "done", 10.0),
            task("#7", "busy", "dev", "done", 20.0),
            task("#8", "over", "dev", "done", 5.0),
        )))

    @Test
    fun orderAndLabels() {
        val s = sections(board)
        // Running first, then open, then the user's own, then all done, paused last; projects without tasks drop out.
        assertEquals(listOf("busy", "work", "mine", "over", "held"), s.map { it.name })
        assertEquals(listOf("running", "open", "you", "done", "paused"), s.map { it.label })
    }

    @Test
    fun asksReviewQueueNextAndSteps() {
        fun t(id: String, owner: String, state: String, pos: Int) = task(id, "p", owner, state).put("pos", pos)
        val b = JSONObject()
            .put("projects", JSONArray(listOf(project("p"), project("q"))))
            .put("tasks", JSONArray(listOf(
                t("#1", "dev", "open", 3), t("#2", "dev", "open", 2), t("#3", "dev", "review", 1),
                t("#4", "mc", "asks", 1).put("steps", JSONArray(listOf(JSONObject().put("id", "1").put("title", "a").put("state", "completed")))),
                task("#5", "q", "dev", "running").put("pos", 4),
            )))
        val s = sections(b)
        assertEquals(listOf("p" to "asks", "q" to "running"), s.map { it.name to it.label })
        assertEquals(listOf("#4", "#3", "#2", "#1"), s[0].live.map { it.id })
        assertEquals(listOf("#2"), s.flatMap { it.live }.filter { it.next }.map { it.id })
        assertEquals(listOf(Step("a", "completed")), s[0].live[0].steps)
        assertEquals(true, s[0].live[0].working)
    }

    @Test
    fun runningFirstDoneNewestFirst() {
        val busy = sections(board).first()
        assertEquals(listOf("#5", "#4"), busy.live.map { it.id })
        assertEquals(listOf("#7", "#6"), busy.done.map { it.id })
        assertEquals(true, sections(board).first { it.name == "held" }.paused)
    }
}

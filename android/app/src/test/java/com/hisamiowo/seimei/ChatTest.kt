package com.hisamiowo.seimei

import androidx.compose.ui.graphics.Color
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.ZoneId

class ChatTest {
    private val utc = ZoneId.of("UTC")
    private var id = 0L
    private fun ev(type: String, t: Double = 1_800_000_000.0, vararg kv: Pair<String, Any>) =
        JSONObject().put("id", ++id).put("type", type).put("t", t).apply { kv.forEach { put(it.first, it.second) } }

    @Test fun toolsGroupIntoOneActivity() {
        val c = Chat(utc)
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "a", "summary" to "ls")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "b", "summary" to "pwd")))
        c.add(ev("tool", kv = arrayOf("name" to "Read", "tool_id" to "c", "summary" to "x")))
        c.add(ev("tool_result", kv = arrayOf("tool_id" to "a", "text" to "out", "error" to false)))
        val a = c.items.filterIsInstance<Item.Activity>().single()
        assertEquals(3, a.calls.size)
        assertEquals(false, a.calls[0].running)
        assertEquals("out", a.calls[0].out)
        assertEquals("Ran 2 commands, reading a file", activityLabel(a.calls))
        c.add(ev("assistant", kv = arrayOf("text" to "hi")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "d", "summary" to "x")))
        assertEquals(2, c.items.filterIsInstance<Item.Activity>().size)
    }

    @Test fun updatesAndTextsComeBetweenToolGroups() {
        val c = Chat(utc)
        c.add(ev("update", kv = arrayOf("text" to "Checking which frames are there.")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "a", "summary" to "ls frames")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "b", "summary" to "ls old")))
        c.add(ev("update", kv = arrayOf("text" to "Both are there.")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "c", "summary" to "mv a b")))
        c.add(ev("assistant", kv = arrayOf("text" to "Renamed.")))
        val rows = c.items.filter { it !is Item.Day }.map {
            when (it) {
                is Item.Update -> "update ${it.text}"
                is Item.Activity -> "tools ${it.calls.size}"
                is Item.Msg -> "${it.role} ${it.text}"
                else -> it.toString()
            }
        }
        assertEquals(listOf("update Checking which frames are there.", "tools 2", "update Both are there.", "tools 1", "assistant Renamed."), rows)
    }

    @Test fun threadTagsSenderAndOffer() {
        val c = Chat(utc)
        c.add(ev("user", kv = arrayOf("text" to "reuse the sheet", "thread" to "t1", "thread_name" to "kryosheet")))
        c.add(ev("assistant", kv = arrayOf("text" to "ok")))
        val re = id
        c.add(ev("offer", kv = arrayOf("re" to re, "lane" to "c")))
        c.add(ev("user", kv = arrayOf("text" to "check the mount", "from" to "seimei")))
        val msgs = c.items.filterIsInstance<Item.Msg>()
        assertEquals("t1" to "kryosheet", msgs[0].thread to msgs[0].threadName)
        assertEquals(listOf(false, true, false), msgs.map { it.offer })
        assertEquals("seimei", msgs[2].sender)
        assertTrue(c.items.none { it is Item.Trig })
    }

    @Test fun summonToolIsSkipped() {
        val c = Chat(utc)
        c.add(ev("tool", kv = arrayOf("name" to "mcp__seimei__summon", "tool_id" to "s", "summary" to "x")))
        assertTrue(c.items.none { it is Item.Activity })
    }

    @Test fun outcomeBeforeCardStillSettles() {
        val c = Chat(utc)
        c.add(JSONObject().put("id", ++id).put("type", "approval_done").put("aid", "q1").put("outcome", "denied"))
        c.add(ev("approval", kv = arrayOf("aid" to "q1", "who" to "seimei", "action" to "desktop", "target" to "x", "detail" to "d", "expires" to 1e10)))
        assertEquals("denied", c.items.filterIsInstance<Item.Approval>().single().outcome)
        assertEquals(0, c.waiting)
        c.add(ev("approval", kv = arrayOf("aid" to "q2", "who" to "seimei", "action" to "edit", "target" to "x", "detail" to "d", "expires" to 1e10)))
        assertEquals(1, c.waiting)
        assertNull(c.items.filterIsInstance<Item.Approval>().last().outcome)
        c.add(JSONObject().put("id", ++id).put("type", "approval_done").put("aid", "q2").put("outcome", "approved"))
        assertEquals("approved", c.items.filterIsInstance<Item.Approval>().last().outcome)
        assertEquals(0, c.waiting)
    }

    @Test fun settledCardHasNoFollowUp() {
        val c = Chat(utc)
        val t = 1_800_000_000.0
        c.add(ev("approval", t, "aid" to "a1", "who" to "seimei", "action" to "desktop", "target" to "close firefox", "detail" to "d", "expires" to 1e10))
        c.add(ev("approval_done", t + 5, "aid" to "a1", "outcome" to "approved"))
        c.fold(ev("system", t + 8, "text" to "Closed firefox.", "lane" to "t"))
        assertTrue(c.items.none { it is Item.Fold })
        c.fold(ev("system", t + 8 + AFTER_CARD, "text" to "The desktop is back.", "lane" to "t"))
        assertEquals(1, c.items.filterIsInstance<Item.Fold>().size)
    }

    @Test fun repeatedNotesCountUp() {
        val c = Chat(utc)
        c.add(ev("system", 1_800_000_000.0, "text" to "The desktop is back."))
        c.add(ev("system", 1_800_000_100.0, "text" to "The desktop is back."))
        c.add(ev("system", 1_800_000_100.0 + REPEAT + 200, "text" to "The desktop is back."))
        val notes = c.items.filterIsInstance<Item.Note>()
        assertEquals(listOf(2, 1), notes.map { it.n })
    }

    @Test fun dayChips() {
        val c = Chat(utc)
        c.add(ev("user", 1_800_000_000.0, "text" to "a"))
        c.add(ev("user", 1_800_000_060.0, "text" to "b"))
        c.add(ev("user", 1_800_000_000.0 + 86_400, "text" to "c"))
        assertEquals(2, c.items.count { it is Item.Day })
        assertTrue(c.items[0] is Item.Day)
    }

    @Test fun workerEvents() {
        val c = Chat(utc)
        c.add(ev("worker", kv = arrayOf("event" to "summoned", "name" to "fox", "worker" to "w1", "text" to "task", "model" to "opus", "cwd" to "/home/me/x")))
        c.add(ev("worker", kv = arrayOf("event" to "stopped", "name" to "fox", "worker" to "w1")))
        assertEquals("Summoned", c.items.filterIsInstance<Item.Worker>().single().event.replaceFirstChar { it.uppercase() }.take(8))
        assertEquals("fox was stopped", c.items.filterIsInstance<Item.Note>().single().text)
        assertEquals("~/x", home("/home/me/x"))
    }

    @Test fun userImages() {
        val c = Chat(utc)
        c.add(ev("user", kv = arrayOf("text" to "", "images" to org.json.JSONArray(listOf("a.jpg", "b.png")))))
        assertEquals(listOf("a.jpg", "b.png"), (c.items.last() as Item.Msg).images)
    }

    @Test fun files() {
        val c = Chat(utc)
        val f = JSONObject().put("id", "ab12").put("name", "notes.txt").put("size", 2048).put("kind", "text")
        c.add(ev("user", kv = arrayOf("text" to "", "files" to org.json.JSONArray().put(f))))
        assertEquals(listOf(FileRef("ab12", "notes.txt", 2048, "text")), (c.items.last() as Item.Msg).files)
        // The event's own id must not stand in for the file's.
        c.add(ev("file", kv = arrayOf("file" to "cd34", "name" to "a.pdf", "size" to (3 shl 20), "kind" to "pdf", "caption" to "c")))
        assertEquals(Item.Sent((c.items.last() as Item.Sent).key, FileRef("cd34", "a.pdf", 3L shl 20, "pdf"), "c", 1_800_000_000.0), c.items.last())
        assertEquals(false, expired(1_800_000_000.0, now = 1_800_000_000.0 + 29 * 86400))
        assertEquals(true, expired(1_800_000_000.0, now = 1_800_000_000.0 + 31 * 86400))
        assertEquals("2 KB", sizeText(2048))
        assertEquals("3.0 MB", sizeText(3L shl 20))
    }

    @Test fun sessionTimelineEvents() {
        // A session's transcript names events by uuid, and tool results carry no id at all.
        val c = Chat(utc)
        c.add(JSONObject().put("id", "u-1").put("type", "user").put("text", "go").put("t", 1_800_000_000.0))
        c.add(JSONObject().put("id", "u-2:0").put("type", "tool").put("name", "Read").put("tool_id", "r").put("summary", "a").put("t", 1_800_000_001.0))
        c.add(JSONObject().put("type", "tool_result").put("tool_id", "r").put("text", "x"))
        c.add(JSONObject().put("id", "u-3:0").put("type", "tool").put("name", "Bash").put("tool_id", "b").put("summary", "make").put("t", 1_800_000_002.0))
        val a = c.items.filterIsInstance<Item.Activity>().single()
        assertEquals("su-2:0", a.key)
        assertEquals("Read a file, running a command", activityLabel(a.calls, "b"))
        assertEquals("Read a file, running a command", activityLabel(a.calls))
        assertTrue(c.items.map { it.key }.toSet().size == c.items.size)
    }

    @Test fun agentRows() {
        val now = System.currentTimeMillis() / 1000.0
        val seen = JSONObject()
        val busy = JSONObject().put("pid", 7).put("name", "gardener").put("status", "busy").put("since", now).put("project", "plants")
            .put("doing", JSONObject().put("tool", "Bash").put("what", "make").put("since", now - 250))
        val r = fromSession(busy, seen)
        assertEquals("work", r.state)
        assertEquals("make · 4m", r.line)
        val asking = JSONObject().put("pid", 8).put("name", "q").put("status", "waiting").put("question", true).put("ask", "Keep 4?")
        assertEquals("ask" to "Keep 4?", fromSession(asking, seen).let { it.state to it.line })
        // The server words the wait (sessions.rs waiting_text); without it the row says only where it waits.
        val prompt = JSONObject().put("pid", 9).put("name", "p").put("status", "waiting").put("waiting_text", "Waiting for your OK to run Bash")
        assertEquals("Waiting for your OK to run Bash", fromSession(prompt, seen).line)
        assertEquals("Waiting on the server", fromSession(JSONObject().put("pid", 11).put("status", "waiting"), seen).line)
        val idle = JSONObject().put("pid", 10).put("name", "i").put("status", "idle").put("line", "Done.").put("context", 41)
        assertEquals(41, fromSession(idle, seen).context)
        assertNull(fromSession(prompt, seen).context)
        val w = JSONObject().put("id", "w1").put("name", "dot").put("state", "running")
            .put("doing", JSONObject().put("name", "Edit").put("summary", "router.cpp"))
        assertEquals("Editing a file: router.cpp", fromWorker(w).line)
    }

    @Test fun projectTintsMatchTheWebPage() {
        // The page's tint(): h = (h * 31 + code) >>> 0, then TINTS[h % 10]; weather -> 2, plants -> 8, seimei -> 6.
        assertEquals(Color(0xFF94E2D5), tintOf("weather", MOCHA))
        assertEquals(Color(0xFFEBA0AC), tintOf("plants", MOCHA))
        assertEquals(Color(0xFFF9E2AF), tintOf("seimei", MOCHA))
    }

    /** Routes like the view model: trajectory events whole into one Chat, a fold line into the other. */
    private fun lanes(vararg evs: JSONObject): Pair<Chat, Chat> {
        val asks = Asks()
        val chat = Chat(utc, asks = asks)
        val trail = Chat(utc, "t", asks)
        for (e in evs) if (e.inTraj()) { trail.add(e); chat.fold(e) } else chat.add(e)
        return chat to trail
    }

    @Test fun trajectoryFoldsIntoOneLine() {
        val (chat, trail) = lanes(
            ev("user", kv = arrayOf("text" to "go")),
            ev("worker", kv = arrayOf("event" to "done", "worker" to "w1", "name" to "app-tester", "text" to "ok", "lane" to "t")),
            ev("system", kv = arrayOf("text" to "Session is back.", "lane" to "t")),
            ev("worker", kv = arrayOf("event" to "failed", "worker" to "w2", "name" to "asr-bench", "text" to "no", "lane" to "t")),
            ev("assistant", kv = arrayOf("text" to "noted", "lane" to "t", "turn" to 5, "why" to "app-tester finished")),
        )
        val f = chat.items.filterIsInstance<Item.Fold>().single()
        assertEquals("app-tester finished, asr-bench failed, and 1 more", f.text)
        assertTrue(chat.items.none { it is Item.Worker })
        assertEquals(2, trail.items.filterIsInstance<Item.Worker>().size)
        assertEquals("after app-tester finished", trail.items.filterIsInstance<Item.Trig>().single().text)
        assertEquals(5L, trail.items.filterIsInstance<Item.Msg>().single().reply)
        assertEquals("noted", trail.said[5L])
    }

    @Test fun aRunWithNothingToNameLeavesNoLine() {
        // seimei on its own, over the user's sessions: tools and a trajectory: reply, nothing for the user.
        val quiet = arrayOf(
            ev("tool", kv = arrayOf("name" to "session_peek", "tool_id" to "a", "lane" to "t", "turn" to 9, "why" to "notes waited 30 min")),
            ev("tool_result", kv = arrayOf("tool_id" to "a", "text" to "idle", "lane" to "t", "turn" to 9)),
            ev("assistant", kv = arrayOf("text" to "nothing for the user", "lane" to "t", "turn" to 9, "why" to "notes waited 30 min")),
        )
        val (chat, trail) = lanes(ev("user", kv = arrayOf("text" to "go")), *quiet)
        assertTrue(chat.items.none { it is Item.Fold })
        assertEquals("the trajectory keeps it", 1, trail.items.filterIsInstance<Item.Msg>().size)
        // A report later in the same stretch still gets its line.
        val (named, _) = lanes(*quiet, ev("worker", kv = arrayOf("event" to "done", "worker" to "w1", "name" to "dev", "text" to "ok", "lane" to "t")))
        assertEquals("dev finished", named.items.filterIsInstance<Item.Fold>().single().text)
    }

    @Test fun anchorOnlyWhenSomethingSitsBetween() {
        val (chat, _) = lanes(
            ev("user", kv = arrayOf("text" to "first question")),
            ev("assistant", kv = arrayOf("text" to "right below", "lane" to "c", "re" to 1)),
            ev("worker", kv = arrayOf("event" to "asking", "worker" to "w1", "name" to "x", "text" to "which?")),
            ev("assistant", kv = arrayOf("text" to "after a card", "lane" to "c", "re" to 1)),
        )
        val msgs = chat.items.filterIsInstance<Item.Msg>()
        assertNull(msgs[1].anchor)
        assertEquals(Anchor("first question", "e1", "c"), msgs[2].anchor)
    }

    @Test fun reportInTrajectorySettlesAskInChat() {
        val (chat, _) = lanes(
            ev("worker", kv = arrayOf("event" to "asking", "worker" to "w1", "name" to "x", "text" to "which?")),
            ev("worker", kv = arrayOf("event" to "done", "worker" to "w1", "name" to "x", "text" to "ok", "lane" to "t")),
        )
        assertEquals("Answered", chat.items.filterIsInstance<Item.Worker>().single().settled)
    }

    @Test fun aSettledAskFoldsWithNoNote() {
        // seimei's message answers one; the user marks the other done.
        val (chat, _) = lanes(
            ev("worker", kv = arrayOf("event" to "asking", "worker" to "w1", "name" to "x", "text" to "which?")),
            ev("worker", kv = arrayOf("event" to "settled", "worker" to "w1", "name" to "x", "by" to "seimei")),
            ev("worker", kv = arrayOf("event" to "asking", "worker" to "w2", "name" to "y", "text" to "still?")),
            ev("worker", kv = arrayOf("event" to "settled", "worker" to "w2", "name" to "y", "by" to "user")),
        )
        assertEquals(listOf("Answered", "Done"), chat.items.filterIsInstance<Item.Worker>().map { it.settled })
        assertTrue(chat.items.none { it is Item.Note })
    }

    @Test fun quotedReplyShowsItsAnchor() {
        val (chat, _) = lanes(ev("user", kv = arrayOf("text" to "ship it", "quote" to "app-tester is done", "quote_id" to 9)))
        assertEquals(Anchor("app-tester is done", "e9", "t"), chat.items.filterIsInstance<Item.Msg>().single().anchor)
    }

    @Test fun archivedSearchShowsTheMatch() {
        val w = JSONObject().put("id", "a1").put("name", "prefill-sweep").put("state", "done").put("archived", true)
            .put("task", "Sweep the prefill tiles and report the best").put("summary", "Report: pp2048 1612 ms at matched clock; GPU held 41 min")
        val a = fromWorker(w)
        assertTrue(a.archived)
        val (line, at) = searchLine(a, "gpu")
        assertTrue(line.startsWith("…") && !line.startsWith("…12"))
        assertEquals("gpu", line.substring(at, at + 3).lowercase())
        assertEquals("Sweep the prefill tiles and report the best" to 10, searchLine(a, "prefill"))
        assertEquals(a.line to -1, searchLine(a, ""))
    }

    @Test fun repliesQuoteMessagesAndCards() {
        val c = Chat(utc)
        c.add(ev("user", kv = arrayOf("text" to "go")))
        c.add(ev("assistant", kv = arrayOf("text" to "On it.")))
        c.add(ev("worker", kv = arrayOf("event" to "asking", "name" to "npu-dma", "worker" to "ab12", "text" to "\nWhich branch?\nmore")))
        c.add(ev("worker", kv = arrayOf("event" to "summoned", "name" to "x", "worker" to "cd34", "text" to "task")))
        c.add(ev("approval", kv = arrayOf("aid" to "a1", "who" to "seimei", "action" to "command", "target" to "delete a session",
            "detail" to "x", "expires" to 0.0)))
        val quotes = c.items.mapNotNull { it.quote("c") }
        assertEquals(listOf("On it.", "npu-dma asks: Which branch?", "seimei wants to delete a session"), quotes.map { it.text })
        assertEquals(listOf(2L, 3L, 5L), quotes.map { it.id })
        assertNull(c.items.filterIsInstance<Item.Msg>().first { it.role == "user" }.quote("c"))
    }

    @Test fun userAnchorGoesToTheQuotedLane() {
        val c = Chat(utc)
        c.add(ev("user", kv = arrayOf("text" to "yes", "quote" to "npu-dma finished: done", "quote_id" to 7, "quote_lane" to "c")))
        c.add(ev("user", kv = arrayOf("text" to "old", "quote" to "earlier", "quote_id" to 3)))
        val a = c.items.filterIsInstance<Item.Msg>().map { it.anchor!! }
        assertEquals(listOf("c", "t"), a.map { it.lane })
        assertEquals("e7", a[0].goto)
    }

    @Test fun headerNamesAnAcpHarness() {
        assertEquals("on codex", harnessLabel("acp:codex"))
        // The default harness, and a server from before the field, go without saying.
        assertNull(harnessLabel("claude"))
        assertNull(harnessLabel(""))
    }

}

class MarkdownTest {
    private val md = Markdown(MdColors(Color.Magenta, Color.Gray))

    @Test fun blocks() {
        val b = md.parse("# Title\n\npara **bold** and `code`\nnext line\n\n- one\n- two\n  - nested\n\n1. a\n2. b\n\n```\nx < y\n```\n\n> quote\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |")
        assertEquals(listOf("Heading", "Para", "Bullets", "Bullets", "Code", "Quote", "Rule", "Table"), b.map { it::class.simpleName })
        val p = b[1] as Block.Para
        assertEquals("para bold and code\nnext line", p.text.text)
        val ul = b[2] as Block.Bullets
        assertEquals(2, ul.items.size)
        assertEquals(2, ul.items[1].size)
        assertTrue(ul.items[1][1] is Block.Bullets)
        val ol = b[3] as Block.Bullets
        assertTrue(ol.ordered)
        assertEquals("x < y", (b[4] as Block.Code).text)
        val t = b[7] as Block.Table
        assertEquals(listOf("a", "b"), t.head.map { it.text })
        assertEquals(listOf("1", "2"), t.rows[0].map { it.text })
    }

    @Test fun inlineLinks() {
        val s = md.inline("see [the docs](https://x.org/a) or https://y.org/b. and snake_case_name, 2*3*4")
        assertEquals("see the docs or https://y.org/b. and snake_case_name, 2*3*4", s.text)
        val links = s.getLinkAnnotations(0, s.length).map { (it.item as androidx.compose.ui.text.LinkAnnotation.Url).url }
        assertEquals(listOf("https://x.org/a", "https://y.org/b"), links)
    }

    @Test fun italicAndStrike() {
        val s = md.inline("*it* _also_ ~~gone~~ **b** *i*")
        assertEquals("it also gone b i", s.text)
        assertTrue(s.spanStyles.size >= 5)
    }

    @Test fun pluginToolsPhraseLikeBefore() {
        // #61: the built-in plugins' tools have their own servers; old events keep the old names.
        assertEquals(phrase("mcp__seimei__browser_tabs", 1), phrase("mcp__browser__browser_tabs", 1))
        assertEquals("used screenshot", phrase("mcp__desktop__screenshot", 1))
        assertEquals("used x tool", phrase("mcp__mytool__x_tool", 1))
        assertEquals("checked the shikigami", phrase("mcp__seimei__workers", 1))
    }
}

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
        assertEquals("Ran 2 commands, read a file", activityLabel(a.calls))
        c.add(ev("assistant", kv = arrayOf("text" to "hi")))
        c.add(ev("tool", kv = arrayOf("name" to "Bash", "tool_id" to "d", "summary" to "x")))
        assertEquals(2, c.items.filterIsInstance<Item.Activity>().size)
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
        assertEquals(Item.Sent((c.items.last() as Item.Sent).key, FileRef("cd34", "a.pdf", 3L shl 20, "pdf"), "c"), c.items.last())
        assertEquals("2 KB", sizeText(2048))
        assertEquals("3.0 MB", sizeText(3L shl 20))
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
}

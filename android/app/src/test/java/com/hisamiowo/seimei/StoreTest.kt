package com.hisamiowo.seimei

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

class StoreTest {
    private fun ev(id: Any?, text: String = "x") =
        JSONObject().put("type", "assistant").put("text", text).put("t", 1_800_000_000.0).apply { if (id != null) put("id", id) }

    @Test fun freshSkipsWhatIsHeld() {
        val have = listOf(ev(1), ev(2), ev("u:0"))
        val got = Store.fresh(have, listOf(ev(2), ev(3), ev("u:0"), ev("u:1"), ev(null), ev(3)))
        assertEquals(listOf("3", "u:1", null), got.map { it.opt("id")?.toString() })
    }

    @Test fun pageMergesInPlaceAndKeepsTheBound() {
        val pg = Page("worker", "w")
        pg.take((1..10).map { ev(it.toLong()) }, reset = true)
        assertEquals(10L, pg.last)
        val chat = pg.chat
        pg.take((8..12).map { ev(it.toLong()) }, reset = false)
        assertTrue("a delta merges into the same chat", chat === pg.chat)
        assertEquals((1..12).map { it.toLong() }, pg.raw.map { it.getLong("id") })
        assertEquals(12L, pg.last)
        pg.take((13..(12 + Store.PAGE_KEEP)).map { ev(it.toLong()) }, reset = false)
        assertEquals(Store.PAGE_KEEP, pg.raw.size)
        assertEquals(12L + Store.PAGE_KEEP, pg.raw.last().getLong("id"))
        pg.take(listOf(ev(500L)), reset = true)
        assertEquals(listOf(500L), pg.raw.map { it.getLong("id") })
    }

    @Test fun savesOfOneKeyNeverMix() {
        val dir = Files.createTempDirectory("store").toFile()
        val s = Store(dir)
        val texts = listOf("a".repeat(200_000), "b".repeat(100_000))
        val writers = texts.map { t -> Thread { repeat(50) { s.write(Store.CHAT, JSONObject().put("t", t).toString()) } } }
        writers.forEach { it.start() }
        writers.forEach { it.join() }
        assertTrue(s.read(Store.CHAT)!!.getString("t") in texts)
        dir.deleteRecursively()
    }

    @Test fun keepsPagesBoundedAndClears() {
        val dir = Files.createTempDirectory("store").toFile()
        val s = Store(dir)
        s.write(Store.CHAT, JSONObject().put("events", Store.tail((1..500).map { ev(it.toLong()) }, Store.CHAT_KEEP)).toString())
        assertEquals(Store.CHAT_KEEP, s.read(Store.CHAT)!!.getJSONArray("events").length())
        for (i in 0 until Store.PAGES + 5) {
            s.write(Store.pageKey("worker", "w$i"), JSONObject().put("last", i).toString())
            java.io.File(dir, "conversations/worker-w$i.json").setLastModified(1_000_000L * (i + 1))
        }
        s.write(Store.pageKey("worker", "new"), "{}")
        val pages = java.io.File(dir, "conversations").list()!!.filter { it.startsWith("worker-") }
        assertEquals(Store.PAGES, pages.size)
        assertTrue("the chat is never trimmed", s.read(Store.CHAT) != null)
        assertNull("the oldest pages go first", s.read("worker-w0"))
        s.clear()
        assertNull(s.read(Store.CHAT))
        dir.deleteRecursively()
    }
}

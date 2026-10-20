package com.hisamiowo.seimei

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle

/** The small Markdown dialect of seimei's replies, as the web page's md.js reads it. Text is never run as markup. */
sealed interface Block {
    data class Para(val text: AnnotatedString) : Block
    data class Heading(val level: Int, val text: AnnotatedString) : Block
    data class Code(val text: String) : Block
    data class Quote(val blocks: List<Block>) : Block
    data class Bullets(val ordered: Boolean, val start: Int, val items: List<List<Block>>) : Block
    data object Rule : Block
    data class Table(val head: List<AnnotatedString>, val rows: List<List<AnnotatedString>>) : Block
}

data class MdColors(val link: Color, val codeBg: Color)

private val INLINE = Regex(
    "`([^`\\n]+)`" +
        "|\\[([^\\]\\n]+)\\]\\((https?://[^\\s)]+)\\)" +
        "|(https?://[^\\s<>()]*[^\\s<>().,;:!?'\"])" +
        "|\\*\\*([^*\\n]+)\\*\\*" +
        "|~~([^~\\n]+)~~" +
        "|(?<![\\w*])\\*([^*\\n]+)\\*(?![\\w*])" +
        "|(?<!\\w)_([^_\\n]+)_(?!\\w)"
)
private val FENCE = Regex("^\\s*(```|~~~)")
private val ITEM = Regex("^(\\s*)([-*+]|\\d+[.)])\\s+(.*)$")
private val RULE = Regex("^\\s*([-*_])(\\s*\\1){2,}\\s*$")
private val TABLE_SEP = Regex("^\\s*\\|?\\s*:?-+:?\\s*(\\|\\s*:?-+:?\\s*)*\\|?\\s*$")
private val HEADING = Regex("^(#{1,6})\\s+(.*)$")

class Markdown(private val c: MdColors) {
    private fun AnnotatedString.Builder.inline(src: String) {
        var at = 0
        for (m in INLINE.findAll(src)) {
            append(src.substring(at, m.range.first))
            at = m.range.last + 1
            val g = m.groupValues
            when {
                g[1].isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = c.codeBg, fontSize = CODE_EM)) { append(g[1]) }
                g[2].isNotEmpty() -> link(g[3]) { inline(g[2]) }
                g[4].isNotEmpty() -> link(g[4]) { append(g[4]) }
                g[5].isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { inline(g[5]) }
                g[6].isNotEmpty() -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { inline(g[6]) }
                g[7].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { inline(g[7]) }
                g[8].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { inline(g[8]) }
            }
        }
        append(src.substring(at))
    }

    private fun AnnotatedString.Builder.link(url: String, body: AnnotatedString.Builder.() -> Unit) {
        val style = TextLinkStyles(SpanStyle(color = c.link, textDecoration = TextDecoration.Underline))
        withLink(LinkAnnotation.Url(url, style)) { body() }
    }

    fun inline(src: String): AnnotatedString = buildAnnotatedString { inline(src) }

    private fun lines(lines: List<String>, joined: String = "\n"): AnnotatedString = buildAnnotatedString {
        lines.forEachIndexed { i, l ->
            if (i > 0) append(joined)
            inline(l)
        }
    }

    private fun indent(l: String) = l.length - l.trimStart().length
    private fun isTable(ls: List<String>, i: Int) = '|' in ls[i] && i + 1 < ls.size && TABLE_SEP.matches(ls[i + 1])
    private fun startsBlock(ls: List<String>, i: Int) = FENCE.containsMatchIn(ls[i]) || HEADING.matches(ls[i]) ||
        ls[i].startsWith(">") || ITEM.matches(ls[i]) || RULE.matches(ls[i]) || isTable(ls, i)

    private fun cells(row: String) =
        row.trim().removePrefix("|").removeSuffix("|").split(Regex("(?<!\\\\)\\|")).map { inline(it.trim()) }

    private fun list(ls: List<String>, start: Int): Pair<Block, Int> {
        var i = start
        val base = indent(ls[i])
        fun ordered(l: String) = l.trimStart().firstOrNull()?.isDigit() == true
        val isOrdered = ordered(ls[i])
        fun same(l: String) = ITEM.matches(l) && indent(l) == base && ordered(l) == isOrdered
        val items = ArrayList<MutableList<String>>()
        while (i < ls.size) {
            val l = ls[i]
            val m = ITEM.find(l)
            if (m != null && m.groupValues[1].length == base) {
                if (!same(l)) break
                items.add(mutableListOf(m.groupValues[3]))
            } else if (l.isBlank()) {
                val next = ls.getOrNull(i + 1)
                if (next == null || !(indent(next) > base || same(next))) break
            } else if (indent(l) > base && items.isNotEmpty()) {
                items.last().add(l.replace(Regex("^\\s{0,${base + 4}}"), ""))
            } else break
            i++
        }
        val first = ITEM.find(ls[start])!!.groupValues[2].takeWhile { it.isDigit() }.toIntOrNull() ?: 1
        val blocks = items.map { item ->
            val rest = item.drop(1)
            listOf<Block>(Block.Para(inline(item[0]))) + if (rest.isEmpty()) emptyList() else parse(rest.joinToString("\n"))
        }
        return Block.Bullets(isOrdered, first, blocks) to i
    }

    fun parse(src: String): List<Block> {
        val ls = src.replace("\r\n", "\n").replace('\r', '\n').split('\n')
        val out = ArrayList<Block>()
        var i = 0
        while (i < ls.size) {
            val line = ls[i]
            val fence = FENCE.find(line)
            val heading = HEADING.matchEntire(line)
            when {
                line.isBlank() -> i++
                fence != null -> {
                    val mark = fence.groupValues[1]
                    val body = ArrayList<String>()
                    i++
                    while (i < ls.size && !ls[i].trim().startsWith(mark)) body.add(ls[i++])
                    i++
                    out.add(Block.Code(body.joinToString("\n")))
                }
                heading != null -> {
                    out.add(Block.Heading(heading.groupValues[1].length, inline(heading.groupValues[2])))
                    i++
                }
                RULE.matches(line) -> {
                    out.add(Block.Rule)
                    i++
                }
                line.startsWith(">") -> {
                    val body = ArrayList<String>()
                    while (i < ls.size && ls[i].startsWith(">")) body.add(ls[i++].replace(Regex("^>\\s?"), ""))
                    out.add(Block.Quote(parse(body.joinToString("\n"))))
                }
                isTable(ls, i) -> {
                    val head = cells(ls[i])
                    val rows = ArrayList<List<AnnotatedString>>()
                    i += 2
                    while (i < ls.size && '|' in ls[i] && ls[i].isNotBlank()) rows.add(cells(ls[i++]))
                    out.add(Block.Table(head, rows))
                }
                ITEM.matches(line) -> {
                    val (b, next) = list(ls, i)
                    out.add(b)
                    i = next
                }
                else -> {
                    val para = arrayListOf(line)
                    i++
                    while (i < ls.size && ls[i].isNotBlank() && !startsBlock(ls, i)) para.add(ls[i++])
                    out.add(Block.Para(lines(para)))
                }
            }
        }
        return out
    }

    companion object {
        val CODE_EM = androidx.compose.ui.unit.TextUnit(0.9f, androidx.compose.ui.unit.TextUnitType.Em)
    }
}

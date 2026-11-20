package com.hisamiowo.seimei

import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.wear.compose.material3.Text
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

// ---------- markdown ----------

@Composable
fun Md(src: String, color: Color = LocalT.current.text) {
    val t = LocalT.current
    val colors = MdColors(t.primary, t.containerHigh)
    val blocks = remember(src, colors) { Markdown(colors).parse(src) }
    Blocks(blocks, color)
}

@Composable
private fun Blocks(blocks: List<Block>, color: Color) {
    val t = LocalT.current
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> Text(b.text, color = color, fontSize = BODY, lineHeight = BODY_LH, fontFamily = Sans)
            is Block.Heading -> Text(b.text, color = color, fontSize = if (b.level <= 2) TITLE else BODY, lineHeight = BODY_LH,
                fontWeight = FontWeight.SemiBold, fontFamily = Sans)
            is Block.Code -> Text(b.text, color = color, fontSize = 10.5.sp, lineHeight = 14.sp, fontFamily = FontFamily.Monospace, softWrap = false,
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(t.surfaceLowest)
                    .horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 4.dp))
            is Block.Quote -> Row {
                Box(Modifier.width(2.dp).heightIn(min = 16.dp).background(t.outline))
                Box(Modifier.width(8.dp))
                Blocks(b.blocks, t.text2)
            }
            is Block.Bullets -> Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                b.items.forEachIndexed { i, it ->
                    Row {
                        Text(if (b.ordered) "${b.start + i}." else "•", color = color, fontSize = BODY, lineHeight = BODY_LH, fontFamily = Sans,
                            textAlign = TextAlign.End, modifier = Modifier.width(16.dp).padding(end = 4.dp))
                        Blocks(it, color)
                    }
                }
            }
            Block.Rule -> Box(Modifier.fillMaxWidth().padding(vertical = 3.dp).height(1.dp).background(t.containerHigh))
            is Block.Table -> Column(Modifier.clip(RoundedCornerShape(8.dp)).background(t.container).horizontalScroll(rememberScrollState())) {
                for ((i, r) in (listOf(b.head) + b.rows).withIndex()) Row {
                    for (c in r) Text(c, color = if (i == 0) t.text2 else color, fontSize = SMALL, fontFamily = Sans,
                        fontWeight = if (i == 0) FontWeight.SemiBold else null, modifier = Modifier.width(90.dp).padding(6.dp))
                }
            }
        }
    }
}

// ---------- the rows of a chat or a page ----------

/** One chip line: a tool run, a run of the trajectory, an event; a tap goes on. */
@Composable
fun Chip(text: String, icon: androidx.compose.ui.graphics.vector.ImageVector? = null, bold: String = "", onClick: (() -> Unit)? = null) {
    val t = LocalT.current
    Row(
        Modifier.heightIn(min = 30.dp).clip(RoundedCornerShape(8.dp)).background(t.container)
            .let { if (onClick != null) it.clickable(onClick = onClick) else it }.padding(start = 8.dp, end = 5.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (icon != null) Ico(icon, 13.dp, t.text2)
        if (bold.isNotEmpty()) T(bold, size = BODY, weight = FontWeight.Medium, lines = 1)
        T(text, Modifier.weight(1f, fill = false), SMALL, if (bold.isEmpty()) t.text2 else t.text3, lines = 1)
        if (onClick != null) Ico(Icons.chev, 15.dp, t.text3)
    }
}

fun iconFor(name: String) = when {
    name == "Bash" -> Icons.bash
    name == "Read" -> Icons.read
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> Icons.edit
    name in setOf("Grep", "Glob") -> Icons.search
    name.startsWith("Web") -> Icons.web
    name == "mcp__seimei__workers" -> Icons.agents
    else -> Icons.tool
}

/** What a shikigami card says on its line: "finished", "asks", the model and effort when summoned. */
private fun workerWord(w: Item.Worker): String = when (w.event) {
    "summoned" -> listOf(modelName(w.model), w.effort).filter { it.isNotEmpty() }.joinToString(" · ")
    "done" -> "finished"
    "failed" -> "failed"
    "asking" -> w.settled?.lowercase() ?: "asks"
    else -> w.event
}

/**
 * Draws one item of a chat, the trajectory or an agent's page.
 * [lane] says where it shows: "c" (the chat), "t" (the trajectory) or "p" (a page).
 */
@Composable
fun ItemRow(it: Item, vm: Model, lane: String, onFold: (String) -> Unit = {}) {
    val t = LocalT.current
    when (it) {
        is Item.Day -> T(dayLabel(it.t), Modifier.fillMaxWidth().padding(top = 4.dp), LABEL, t.text3, FontWeight.Medium, 1, align = TextAlign.Center)
        is Item.Msg -> if (it.role == "user") Bubble(it.text + if (it.images.isNotEmpty()) "  [${plural(it.images.size, "photo", "photos")}]" else "")
            else Md(it.text)
        is Item.Fold -> if (it.text.isNotEmpty()) Chip(it.text.cap(), Icons.list) { onFold(it.goto) }
        is Item.Trig -> T(it.text, size = LABEL, color = t.text3, lines = 1)
        is Item.Update -> T(it.text, size = SMALL, color = t.text2, lines = 4)
        is Item.Note -> T(it.text + if (it.n > 1) " (${it.n}x)" else "", size = SMALL, lines = 3,
            color = when (it.level) { "error" -> t.error; "warn" -> t.warn; else -> t.text3 })
        is Item.Activity -> Chip(activityLabel(it.calls), iconFor(it.calls.lastOrNull()?.name ?: ""))
        is Item.Worker -> {
            val open = { vm.push(Screen.Agent("worker", it.worker)) }
            if (it.event == "asking" && it.settled == null && it.text.isNotEmpty()) Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.warnContainer).clickable(onClick = open).padding(8.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                T(it.name, size = BODY, color = t.onWarnContainer, weight = FontWeight.Bold, lines = 1)
                Md(it.text, t.onWarnContainer)
            } else Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Chip(workerWord(it), bold = it.name, onClick = open)
                if (it.event in setOf("done", "failed") && it.text.isNotEmpty() && lane != "c") Md(it.text, t.text2)
            }
        }
        is Item.Approval -> ApprovalCard(it, vm)
        is Item.Shown -> Picture(it.file, it.caption, vm)
        is Item.Sent -> Chip(it.file.name, Icons.doc)
    }
}

private fun dayLabel(t: Double): String {
    val d = dayOf(t)
    val today = java.time.LocalDate.now()
    return when (d) {
        today -> "Today"
        today.minusDays(1) -> "Yesterday"
        else -> d.format(java.time.format.DateTimeFormatter.ofPattern("EEE d MMM"))
    }
}

/** The user's message: a bubble on the right, as on the phone. */
@Composable
fun Bubble(text: String, dim: Boolean = false, failed: Boolean = false, onClick: (() -> Unit)? = null) {
    val t = LocalT.current
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Box(
            Modifier.widthIn(max = 139.dp).alpha(if (dim) .6f else 1f)
                .clip(RoundedCornerShape(12.dp, 12.dp, 3.dp, 12.dp)).background(if (failed) t.errorContainer else t.primaryContainer)
                .let { if (onClick != null) it.clickable(onClick = onClick) else it }.padding(horizontal = 8.dp, vertical = 5.dp),
        ) { T(text, size = BODY, color = if (failed) t.onErrorContainer else t.onPrimaryContainer, lineHeight = BODY_LH) }
    }
}

/** A card the user answers: what wants to happen, the command, Deny and Allow. A settled one says how it went. */
@Composable
fun ApprovalCard(a: Item.Approval, vm: Model) {
    val t = LocalT.current
    var sent by remember(a.aid) { mutableStateOf<Boolean?>(null) }
    val open = a.outcome == null && sent == null
    val title = "${a.who.ifEmpty { "seimei" }} wants to ${a.target.ifEmpty { a.action }}"
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(t.container)
            .let { if (open) it.border(1.5.dp, t.primary, RoundedCornerShape(16.dp)) else it }.padding(8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        T(title, Modifier.padding(horizontal = 2.dp), BODY, if (open) t.text else t.text2, FontWeight.SemiBold, 4, BODY_LH)
        if (a.detail.isNotEmpty()) Text(a.detail.take(400), color = if (open) t.text else t.text2, fontSize = 10.5.sp, lineHeight = 14.sp,
            fontFamily = FontFamily.Monospace, maxLines = 6,
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(t.surface).padding(horizontal = 6.dp, vertical = 3.dp))
        if (open) Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Pill("Deny", t.tonal, t.onTonal, Modifier.weight(1f)) {
                sent = false
                vm.decide(a, false) { sent = null }
            }
            Pill("Allow", t.primary, t.onPrimary, Modifier.weight(1.4f), Icons.check) {
                sent = true
                vm.decide(a, true) { sent = null }
            }
        } else {
            val word = when (a.outcome ?: if (sent == true) "approved" else "denied") {
                "approved" -> "Allowed"
                "denied" -> "Denied"
                "expired" -> "Expired"
                else -> "Canceled"
            }
            T(word + if (a.outcome == null) "…" else "", Modifier.padding(horizontal = 2.dp), SMALL, t.text3, FontWeight.Medium, 1)
        }
    }
}

@Composable
fun Pill(label: String, bg: Color, fg: Color, modifier: Modifier = Modifier, icon: androidx.compose.ui.graphics.vector.ImageVector? = null, onClick: () -> Unit) =
    Row(modifier.height(30.dp).clip(RoundedCornerShape(15.dp)).background(bg).clickable(onClick = onClick),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
        if (icon != null) {
            Ico(icon, 15.dp, fg)
            Box(Modifier.width(5.dp))
        }
        T(label, size = SMALL1, color = fg, weight = FontWeight.Medium, lines = 1)
    }

// ---------- pictures ----------

private val pictures = LruCache<String, ImageBitmap>(24)

/** A picture seimei or an agent showed, as the server's thumbnail. */
@Composable
fun Picture(name: String, caption: String, vm: Model, width: Dp = 158.dp) {
    val img by produceState(pictures.get(name), name) {
        if (value != null) return@produceState
        value = try {
            val bytes = vm.api.bytes("api/files/$name?w=thumb")
            withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() }?.also { pictures.put(name, it) }
        } catch (e: Exception) {
            null
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        val b = img
        if (b != null) Image(b, caption, Modifier.width(width).clip(RoundedCornerShape(12.dp)), contentScale = ContentScale.FillWidth)
        else Box(Modifier.size(width, width * .6f).clip(RoundedCornerShape(12.dp)).background(LocalT.current.container))
        if (caption.isNotEmpty()) T(caption, size = SMALL, color = LocalT.current.text2, lines = 3)
    }
}

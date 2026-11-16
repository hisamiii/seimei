package com.hisamiowo.seimei

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

// ---------- icons: the web page's SVG paths, drawn as strokes ----------

fun icon(vararg d: String, stroke: Float = 2f): ImageVector = ImageVector.Builder(
    defaultWidth = 24.dp, defaultHeight = 24.dp, viewportWidth = 24f, viewportHeight = 24f,
).apply {
    for (p in d) addPath(
        pathData = PathParser().parsePathString(p).toNodes(),
        stroke = SolidColor(Color.Black), strokeLineWidth = stroke,
        strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round,
    )
}.build()

object Icons {
    val chat = icon("M6 4.5h12A2.5 2.5 0 0 1 20.5 7v8a2.5 2.5 0 0 1-2.5 2.5h-7L6.5 21v-3.5H6A2.5 2.5 0 0 1 3.5 15V7A2.5 2.5 0 0 1 6 4.5z", stroke = 1.8f)
    // The grid: four rounded tiles
    val agents = icon("M6 4h3.5A1.5 1.5 0 0 1 11 5.5V9a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 9V5.5A1.5 1.5 0 0 1 6 4zM14.5 4H18a1.5 1.5 0 0 1 " +
        "1.5 1.5V9A1.5 1.5 0 0 1 18 10.5h-3.5A1.5 1.5 0 0 1 13 9V5.5A1.5 1.5 0 0 1 14.5 4zM6 13.5h3.5A1.5 1.5 0 0 1 11 15v3.5A1.5 1.5 0 0 1 9.5 " +
        "20H6a1.5 1.5 0 0 1-1.5-1.5V15A1.5 1.5 0 0 1 6 13.5zM14.5 13.5H18a1.5 1.5 0 0 1 1.5 1.5v3.5A1.5 1.5 0 0 1 18 20h-3.5a1.5 1.5 0 0 " +
        "1-1.5-1.5V15a1.5 1.5 0 0 1 1.5-1.5z", stroke = 1.8f)
    val check = icon("M5 12.5l4.5 4.5L19 7.5")
    val stopped = icon("M7.5 7.5h9v9h-9z")
    val info = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.5v.01", stroke = 1.8f)
    val restarted = icon("M4 12a8 8 0 1 0 2.4-5.7M4 4.5v4h4")
    val chev = icon("M9 6l6 6-6 6")
    val expand = icon("M6 9l6 6 6-6")
    val send = icon("M12 19V5M5.5 11.5 12 5l6.5 6.5", stroke = 2.4f)
    val photo = icon("M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM9 8.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 1 0 0-3.2zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5", stroke = 1.8f)
    val more = icon("M12 5.5v.01M12 12v.01M12 18.5v.01")
    val close = icon("M6 6l12 12M18 6 6 18")
    val reply = icon("M9 14 4 9l5-5", "M4 9h10.5a5.5 5.5 0 0 1 0 11H11")
    val mic = icon("M12 3.5a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0v-5a3 3 0 0 0-3-3zM6 11.5a6 6 0 0 0 12 0M12 17.5v3", stroke = 1.8f)
    val attach = icon("M20 11.5l-7.8 7.8a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8", stroke = 1.8f)
    val download = icon("M12 4v11M7 10.5l5 5 5-5M5 20h14", stroke = 1.8f)
    val open = icon("M14 4h6v6M20 4l-9 9M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10", stroke = 1.8f)
    val doc = icon("M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6", stroke = 1.8f)
    val archive = icon("M4 7.5 12 3.5l8 4v9l-8 4-8-4zM4 7.5l8 4 8-4M12 11.5v9", stroke = 1.8f)
    val audio = icon("M9 18V5.5l10-2v12.5M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0zM19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0z", stroke = 1.8f)
    val video = icon("M4 6.5h11v11H4zM15 10.5l5-3v9l-5-3", stroke = 1.8f)
    val code = icon("M9 8l-4 4 4 4M15 8l4 4-4 4", stroke = 1.8f)
    val lock = icon("M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z")
    val fail = icon("M12 7v6M12 17v.01M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z")
    val fingerprint = icon("M7 20c.6-1.6 1-3.6 1-6.5a4 4 0 0 1 8 0c0 1.2-.1 2.4-.3 3.5M11.2 21c.5-2 .8-4.5.8-7.5M4.5 16.5c.3-1 .5-2 .5-3a7 7 0 0 1 12-4.9M19 11c.2.8.2 1.7.1 2.6M15 20.5c.3-.8.5-1.6.6-2.5", stroke = 1.8f)
    val bash = icon("M4 17l5-5-5-5M11 19h9")
    val read = icon("M6 3h9l4 4v14H6zM14 3v5h5")
    val edit = icon("M4 20h4L19 9l-4-4L4 16zM13 7l4 4")
    val search = icon("M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4")
    /** Archive: a box with a lid; Icons.archive is a package. */
    val shelf = icon("M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4", stroke = 1.8f)
    val web = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18")
    val tool = icon("M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z")
    val laptop = icon("M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18")
    val list = icon("M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01")
    val alert = icon("M12 7v6M12 17v.01")
    val clock = icon("M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16zM12 8v4l2.5 2")
    val back = icon("M15 5l-7 7 7 7")
    val down = icon("M12 5v14M5.5 12.5 12 19l6.5-6.5", stroke = 2.2f)
    /** A job to look over: a clipboard with a check. */
    val review = icon("M9 4.5H6.5A1.5 1.5 0 0 0 5 6v13.5A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V6a1.5 1.5 0 0 0-1.5-1.5H15M9.8 3h4.4a.8.8 0 0 1 .8.8v1.4a.8.8 0 0 1-.8.8H9.8a.8.8 0 0 1-.8-.8V3.8a.8.8 0 0 1 .8-.8zM8.5 13.5l2.5 2.5 4.5-4.5")
    val bell = icon("M6 10a6 6 0 0 1 12 0c0 4.5 1.5 6.5 2 7H4c.5-.5 2-2.5 2-7zM10 20.5a2.1 2.1 0 0 0 4 0", stroke = 1.8f)
    val palette = icon("M12 21a9 9 0 1 1 0-18c4.97 0 9 3.58 9 8 0 2.76-2.24 4-4.5 4H15a2 2 0 0 0-1.5 3.3A1.6 1.6 0 0 1 12 21z",
        "M7.5 11.5v.01M10 7.5v.01M14.5 7.5v.01M17 11v.01", stroke = 1.8f)
    val shield = icon("M12 3 4.5 6v5.5c0 4.4 3.1 8.2 7.5 9.5 4.4-1.3 7.5-5.1 7.5-9.5V6L12 3z", "M9 12h6M9 15.5h4", stroke = 1.8f)
    val logout = icon("M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3M10 17l-5-5 5-5M5 12h11", stroke = 1.8f)
    val question = icon("M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1")
    val menu = icon("M4 7h16M4 12h16M4 17h16")
    /** A new side thread: a pen on a square. */
    val compose = icon("M12 4H6.5A2.5 2.5 0 0 0 4 6.5v11A2.5 2.5 0 0 0 6.5 20h11a2.5 2.5 0 0 0 2.5-2.5V12M17.6 3.9a1.9 1.9 0 0 1 2.7 2.7L12.5 14.4 9 15l.6-3.5z", stroke = 1.8f)
    /** A side thread: two speech bubbles. */
    val thread = icon("M4.5 4.5h9A1.5 1.5 0 0 1 15 6v5.5A1.5 1.5 0 0 1 13.5 13H8.5L5 16v-3h-.5A1.5 1.5 0 0 1 3 11.5V6a1.5 1.5 0 0 1 1.5-1.5zM18 8.5h1.5A1.5 1.5 0 0 1 21 10v5.5a1.5 1.5 0 0 1-1.5 1.5H19v3l-3.5-3h-4A1.5 1.5 0 0 1 10 15.5V16", stroke = 1.8f)
    val files = icon("M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM4 16l4.5-4.5 4 4 2.5-2.5L20 18M15.5 8.5v.01", stroke = 1.8f)
    val tune = icon("M4 7h9M17 7h3M4 17h3M11 17h9M15 5a2 2 0 1 1 0 4 2 2 0 0 1 0-4zM9 15a2 2 0 1 1 0 4 2 2 0 0 1 0-4z", stroke = 1.8f)
    val copy = icon("M10 8h8.5A1.5 1.5 0 0 1 20 9.5v9a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 8 18.5V10a2 2 0 0 1 2-2zM16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8", stroke = 1.8f)
    val trash = icon("M5 7h14M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3", stroke = 1.8f)
    val phone = icon("M8 3h8a1.5 1.5 0 0 1 1.5 1.5v15A1.5 1.5 0 0 1 16 21H8a1.5 1.5 0 0 1-1.5-1.5v-15A1.5 1.5 0 0 1 8 3zM11 18h2", stroke = 1.8f)
    /** Watch rules by group (watches.rs): a timer, a command's exit, a reboot, a ping. */
    val watch = mapOf(
        "time" to icon("M10 2.5h4M12 14.5V10M12 6.5a7.5 7.5 0 1 1 0 15 7.5 7.5 0 0 1 0-15z", stroke = 1.8f),
        "exits" to icon("M4 5h16v14H4zM7.5 9.5 10 12l-2.5 2.5M12.5 15H16", stroke = 1.8f),
        "reboot" to icon("M12 3v8M7 6.5a7 7 0 1 0 10 0", stroke = 1.8f),
        "ping" to icon("M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15L6 16zM10 20.5a2 2 0 0 0 4 0", stroke = 1.8f),
    )
}

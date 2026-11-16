package com.hisamiowo.seimei

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

val WINDOWS = listOf("five_hour" to "5-hour", "seven_day" to "Weekly")

fun tokens(n: Long): String = when {
    n >= 1_000_000_000 -> "%.1fB".format(Locale.US, n / 1e9)
    n >= 1_000_000 -> "%.1fM".format(Locale.US, n / 1e6)
    n >= 1_000 -> "${Math.round(n / 1e3)}k"
    else -> "$n"
}

/** Cents only below $100: past that they are noise. */
fun usd(n: Double): String = if (n >= 100) "$" + "%,d".format(Locale.US, Math.round(n)) else "$" + "%.2f".format(Locale.US, n)

/** A model id as its product name: the vendor prefix goes, "opus-5-5" reads as "Opus 5.5". */
fun modelName(k: String): String = k.removePrefix("claude-").replace(Regex("-(\\d+)-(\\d+)$"), " $1.$2").replace(Regex("-(\\d+)$"), " $1")
    .replaceFirstChar { it.uppercase() }

fun resetText(t: Double): String {
    val mins = maxOf(0L, Math.round((t * 1000 - System.currentTimeMillis()) / 60000.0))
    val left = when {
        mins < 60 -> "$mins min"
        mins < 2880 -> "${mins / 60} h ${mins % 60} min"
        else -> "${Math.round(mins / 1440.0)} days"
    }
    val d = Instant.ofEpochMilli((t * 1000).toLong()).atZone(ZoneId.systemDefault())
    val at = if (dayOf(t) == java.time.LocalDate.now()) clock(t) else d.format(DateTimeFormatter.ofPattern("EEE")) + " " + clock(t)
    return "Resets $at, in $left"
}

fun span(sec: Double): String {
    val m = Math.round(sec / 60).toInt()
    return if (sec < 60) "${Math.round(sec)} s" else if (m < 60) "$m min" else "${m / 60} h ${"%02d".format(m % 60)} min"
}

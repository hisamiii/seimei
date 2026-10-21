package com.hisamiowo.seimei

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent

/** Sets up the channels and this build's way of getting notifications (Delivery: Firebase, or UnifiedPush). */
class SeimeiApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Push.start(this)
    }
}

object Push {
    /** One channel per kind, so each can be tuned or muted in the system settings. */
    private val CHANNELS = listOf(
        Triple("approval", "Approvals", NotificationManager.IMPORTANCE_HIGH),
        Triple("listen", "Background connection", NotificationManager.IMPORTANCE_MIN),
        Triple("quiet", "Quiet pings", NotificationManager.IMPORTANCE_LOW),
        Triple("ping", "Pings", NotificationManager.IMPORTANCE_DEFAULT),
        Triple("urgent", "Urgent", NotificationManager.IMPORTANCE_HIGH),
    )
    private val ABOUT = mapOf(
        "approval" to "A card waits for your OK: a session command, a file edit, a desktop action.",
        "quiet" to "Pings seimei sends without sound.",
        "ping" to "Things seimei tells you about: an APK is up, a run finished, a reply is ready.",
        "urgent" to "Rare emergencies seimei flags itself.",
        "listen" to "Shown while seimei keeps its own connection open, on phones without a push app.",
    )

    /** Whether the app is on screen; the laptop holds pushes then, and this catches any that slip through. */
    @Volatile
    var visible = false

    /** The chat is on screen (not another tab or an agent's page): replies show everywhere else. */
    @Volatile
    var onChat = true

    fun start(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        for ((id, name, importance) in CHANNELS) {
            nm.createNotificationChannel(NotificationChannel(id, name, importance).apply { description = ABOUT[id] })
        }
        // Questions and automatic replies no longer notify (rules of 2026-10-20): seimei decides what pings.
        nm.deleteNotificationChannel("question")
        nm.deleteNotificationChannel("reply")
        Delivery.start(ctx)
    }

    fun clear(ctx: Context) = ctx.getSystemService(NotificationManager::class.java).cancelAll()

    fun cancel(ctx: Context, tag: String) = ctx.getSystemService(NotificationManager::class.java).cancel(tag, 0)

    /** Turns one of the laptop's data messages into a notification: one per kind, replaced by later ones, withdrawn on
     * cancel. Every way of delivery ends here. */
    fun show(ctx: Context, d: Map<String, String>) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        // A kind from a newer laptop than this app knows still shows, on the Pings channel.
        val kind = (d["kind"] ?: return).let { if (nm.getNotificationChannel(it) != null && it != "listen") it else "ping" }
        val tag = d["tag"] ?: kind
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        // On screen the laptop holds pushes; "reply ready" still shows when the chat itself is not in front, a test always.
        if (visible && d["test"] != "1" && (tag != "reply" || onChat)) return
        val open = PendingIntent.getActivity(
            ctx, tag.hashCode(),
            Intent(ctx, MainActivity::class.java).putExtra("tab", d["tab"] ?: "chat").putExtra("event", d["event"])
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = d["title"].orEmpty()
        val body = d["body"].orEmpty()
        // The lock screen shows only that something came, never the command or the text.
        val public = Notification.Builder(ctx, kind).setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(PUBLIC[kind] ?: "seimei").build()
        val n = Notification.Builder(ctx, kind)
            .setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(title).setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(open).setAutoCancel(true)
            .setOnlyAlertOnce(d["alert"] != "1")
            .setCategory(if (tag == "reply") Notification.CATEGORY_MESSAGE else Notification.CATEGORY_REMINDER)
            .setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(public)
            .build()
        nm.notify(tag, 0, n)
    }

    const val MAUVE = 0xFFCBA6F7.toInt()
    val PUBLIC = mapOf("approval" to "Approval waiting", "quiet" to "Update from seimei", "ping" to "Update from seimei",
        "urgent" to "seimei: urgent")
}

package com.hisamiowo.seimei

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Bundle
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.io.IOException

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
        "approval" to "What a session wants to run, edit, or do",
        "quiet" to "Pings without sound",
        "ping" to "What seimei tells you",
        "urgent" to "Emergencies",
        "listen" to "While seimei keeps its own connection open",
    )

    /** Whether the app is on screen; the server holds pushes then, and this catches any that slip through. */
    @Volatile
    var visible = false

    /** The chat is on screen (not another tab or an agent's page): replies show everywhere else. */
    @Volatile
    var onChat = true

    /** The fingerprint-unlocked session, while the app's process lives; null once it locks or the phone unpairs. */
    @Volatile
    var live: Api? = null

    /** When the app would ask for the fingerprint again (it left the screen LOCK_AFTER ago). */
    @Volatile
    var lockAt = Long.MAX_VALUE

    /** What a notification's Allow/Deny may act through: never a stored secret, only the unlocked session (audit P2). */
    fun session(): Api? = live?.takeIf { System.currentTimeMillis() < lockAt }

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

    /** Turns one of the server's data messages into a notification: one per kind, replaced by later ones, withdrawn on
     * cancel. Every way of delivery ends here. */
    fun show(ctx: Context, d: Map<String, String>) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        // A kind from a newer server than this app knows still shows, on the Pings channel.
        val kind = (d["kind"] ?: return).let { if (nm.getNotificationChannel(it) != null && it != "listen") it else "ping" }
        val tag = d["tag"] ?: kind
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        // On screen the server holds pushes; "reply ready" still shows when the chat itself is not in front, a test always.
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
        val aid = d["aid"]
        // A lone card gets Allow/Deny: only on an unlocked phone (API 31+), only while the app's session is unlocked.
        if (aid != null && Build.VERSION.SDK_INT >= 31 && session() != null) {
            n.addExtras(Bundle().apply { putString("aid", aid) })
            val perm = d["perm"] == "1"
            for ((approve, label) in listOf(false to "Deny", true to "Allow")) {
                val pi = PendingIntent.getBroadcast(
                    ctx, (aid + approve).hashCode(),
                    Intent(ctx, Decide::class.java).putExtra("aid", aid).putExtra("approve", approve).putExtra("perm", perm),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify), label, pi)
                    .setAuthenticationRequired(true).build())
            }
        }
        nm.notify(tag, 0, n.build())
    }

    /** The approval notification, without actions: the app has to be opened (and unlocked) to answer. */
    fun openToAnswer(ctx: Context, why: String) =
        show(ctx, mapOf("kind" to "approval", "tag" to "approval", "title" to "Open seimei to answer", "body" to why, "tab" to "chat"))

    /** Withdraws the approval notification if it still shows this card (a newer card may have replaced it). */
    fun settled(ctx: Context, aid: String) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (nm.activeNotifications.any { it.tag == "approval" && it.notification.extras.getString("aid") == aid }) nm.cancel("approval", 0)
    }

    const val MAUVE = 0xFFCBA6F7.toInt()
    val PUBLIC = mapOf("approval" to "Approval waiting", "quiet" to "seimei", "ping" to "seimei",
        "urgent" to "seimei: urgent")
}

/** Allow or Deny on an approval notification: settles that one card through the unlocked session. After the app's process
 * died or its session locked, it asks to open the app instead (Android blocks starting an activity from here). */
class Decide : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val aid = intent.getStringExtra("aid") ?: return
        val approve = intent.getBooleanExtra("approve", false)
        val api = Push.session() ?: return Push.openToAnswer(ctx, "The app is locked.")
        val body = JSONObject().put("decision", if (approve) "approve" else "deny")
        if (intent.getBooleanExtra("perm", false)) body.put("option", if (approve) "once" else "deny")
        val done = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                api.post("api/approvals/$aid", body)
                Push.settled(ctx, aid)
            } catch (e: HttpError) {
                if (e.code == 409) Push.settled(ctx, aid) else Push.openToAnswer(ctx, "The server said no (${e.code})")
            } catch (e: IOException) {
                Push.openToAnswer(ctx, "Couldn't reach the server")
            } finally {
                done.finish()
            }
        }
    }
}

package com.hisamiowo.seimei

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/** Starts Firebase from the build's config (none means no notifications) and sets up the channels. */
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
        Triple("reply", "Replies", NotificationManager.IMPORTANCE_LOW),
        Triple("ping", "Pings", NotificationManager.IMPORTANCE_DEFAULT),
        Triple("urgent", "Urgent", NotificationManager.IMPORTANCE_HIGH),
    )
    private val ABOUT = mapOf(
        "approval" to "A card waits for your OK: a session command, a file edit, a desktop action.",
        "reply" to "seimei's finished turn, when the chat is not on screen. Silent; later turns update it.",
        "ping" to "Things you asked seimei to tell you about: an APK is up, a render or a run finished.",
        "urgent" to "Rare emergencies seimei flags itself.",
    )

    /** Whether the app is on screen; the laptop holds pushes then, and this catches any that slip through. */
    @Volatile
    var visible = false

    /** The chat is on screen (not another tab or an agent's page): replies show everywhere else. */
    @Volatile
    var onChat = true

    val configured get() = BuildConfig.FCM_APP.isNotEmpty()

    fun start(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        for ((id, name, importance) in CHANNELS) {
            nm.createNotificationChannel(NotificationChannel(id, name, importance).apply { description = ABOUT[id] })
        }
        // Questions no longer notify (rules of 2026-10-20).
        nm.deleteNotificationChannel("question")
        if (!configured || FirebaseApp.getApps(ctx).isNotEmpty()) return
        FirebaseApp.initializeApp(ctx, FirebaseOptions.Builder()
            .setApplicationId(BuildConfig.FCM_APP).setApiKey(BuildConfig.FCM_KEY)
            .setProjectId(BuildConfig.FCM_PROJECT).setGcmSenderId(BuildConfig.FCM_SENDER).build())
    }

    /** This phone's Firebase token, or null without Firebase. */
    suspend fun token(): String? {
        if (!configured) return null
        return suspendCancellableCoroutine { c ->
            FirebaseMessaging.getInstance().token
                .addOnSuccessListener { c.resume(it) }
                .addOnFailureListener { c.resume(null) }
        }
    }

    fun clear(ctx: Context) = ctx.getSystemService(NotificationManager::class.java).cancelAll()

    fun cancel(ctx: Context, tag: String) = ctx.getSystemService(NotificationManager::class.java).cancel(tag, 0)
}

/** Turns the laptop's data messages into notifications: one per kind, replaced by later ones, withdrawn on cancel. */
class Notify : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        // Sent to the laptop at the next login; the token alone opens nothing.
        Prefs(this).pushToken = token
    }

    override fun onMessageReceived(msg: RemoteMessage) {
        val d = msg.data
        val nm = getSystemService(NotificationManager::class.java)
        // A kind from a newer laptop than this app knows still shows, on the Pings channel.
        val kind = (d["kind"] ?: return).let { if (nm.getNotificationChannel(it) != null) it else "ping" }
        val tag = d["tag"] ?: kind
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        // On screen the laptop holds pushes; a reply still shows when the chat itself is not in front.
        if (Push.visible && (kind != "reply" || Push.onChat)) return
        val open = PendingIntent.getActivity(
            this, tag.hashCode(),
            Intent(this, MainActivity::class.java).putExtra("tab", d["tab"] ?: "chat").addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = d["title"].orEmpty()
        val body = d["body"].orEmpty()
        // The lock screen shows only that something came, never the command or the text.
        val public = Notification.Builder(this, kind).setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(PUBLIC[kind] ?: "seimei").build()
        val n = Notification.Builder(this, kind)
            .setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(title).setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(open).setAutoCancel(true)
            .setOnlyAlertOnce(d["alert"] != "1")
            .setCategory(if (kind == "reply") Notification.CATEGORY_MESSAGE else Notification.CATEGORY_REMINDER)
            .setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(public)
            .build()
        nm.notify(tag, 0, n)
    }

    companion object {
        const val MAUVE = 0xFFCBA6F7.toInt()
        val PUBLIC = mapOf("approval" to "Approval waiting", "reply" to "seimei replied", "ping" to "Update from seimei",
            "urgent" to "seimei: urgent")
    }
}

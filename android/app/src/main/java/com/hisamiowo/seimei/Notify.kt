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
        Triple("question", "Questions", NotificationManager.IMPORTANCE_HIGH),
        Triple("reply", "Replies", NotificationManager.IMPORTANCE_LOW),
        Triple("urgent", "Urgent", NotificationManager.IMPORTANCE_HIGH),
    )
    private val ABOUT = mapOf(
        "approval" to "A card waits for your OK: a session command, a file edit, a desktop action.",
        "question" to "A shikigami or one of your sessions asks something. At most one every few minutes.",
        "reply" to "seimei's reply, when you sent a message and left the app before it came. Silent.",
        "urgent" to "Rare emergencies seimei flags itself.",
    )

    /** Whether the app is on screen; the laptop holds pushes then, and this catches any that slip through. */
    @Volatile
    var visible = false

    val configured get() = BuildConfig.FCM_APP.isNotEmpty()

    fun start(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        for ((id, name, importance) in CHANNELS) {
            nm.createNotificationChannel(NotificationChannel(id, name, importance).apply { description = ABOUT[id] })
        }
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
}

/** Turns the laptop's data messages into notifications: one per kind, replaced by later ones, withdrawn on cancel. */
class Notify : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        // Sent to the laptop at the next login; the token alone opens nothing.
        Prefs(this).pushToken = token
    }

    override fun onMessageReceived(msg: RemoteMessage) {
        val d = msg.data
        val kind = d["kind"] ?: return
        val tag = d["tag"] ?: kind
        val nm = getSystemService(NotificationManager::class.java)
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        if (Push.visible) return
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
        val PUBLIC = mapOf("approval" to "Approval waiting", "question" to "A question for you", "reply" to "seimei replied",
            "urgent" to "seimei: urgent")
    }
}

package com.hisamiowo.seimei

import com.hisamiowo.seimei.wear.BuildConfig
import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import com.hisamiowo.seimei.shared.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject
import java.io.IOException
import kotlin.coroutines.resume

class WearApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Notify.start(this)
        Caffeine.restore(this)
    }
}

/**
 * Notifications through Firebase, as the phone app's fcm build gets them; Wear OS has Google Play services.
 * The watch is a paired device of its own, so the server pushes to it as to a phone.
 */
object Notify {
    private val CHANNELS = listOf(
        Triple("approval", "Approvals", NotificationManager.IMPORTANCE_HIGH),
        Triple("quiet", "Quiet pings", NotificationManager.IMPORTANCE_LOW),
        Triple("ping", "Pings", NotificationManager.IMPORTANCE_DEFAULT),
        Triple("urgent", "Urgent", NotificationManager.IMPORTANCE_HIGH),
    )

    val available get() = BuildConfig.FCM_APP.isNotEmpty()

    @Volatile
    var visible = false

    @Volatile
    var onChat = true

    fun start(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        for ((id, name, importance) in CHANNELS) nm.createNotificationChannel(NotificationChannel(id, name, importance))
        if (!available || FirebaseApp.getApps(ctx).isNotEmpty()) return
        FirebaseApp.initializeApp(ctx, FirebaseOptions.Builder()
            .setApplicationId(BuildConfig.FCM_APP).setApiKey(BuildConfig.FCM_KEY)
            .setProjectId(BuildConfig.FCM_PROJECT).setGcmSenderId(BuildConfig.FCM_SENDER).build())
    }

    /** After each login: hands the server this watch's Firebase token. */
    suspend fun register(ctx: Context, api: Api, prefs: Prefs) {
        if (!available) return
        val token = suspendCancellableCoroutine { c ->
            FirebaseMessaging.getInstance().token.addOnSuccessListener { c.resume(it) }.addOnFailureListener { c.resume(null) }
        } ?: prefs.pushToken ?: return
        prefs.pushToken = token
        api.post("api/devices/push", JSONObject().put("token", token))
    }

    fun clear(ctx: Context) = ctx.getSystemService(NotificationManager::class.java).cancelAll()

    fun settled(ctx: Context, aid: String) = ctx.getSystemService(NotificationManager::class.java).cancel("approval:$aid", 0)

    /** One of the server's data messages as a notification: one per tag, replaced by later ones, withdrawn on cancel. */
    fun show(ctx: Context, d: Map<String, String>) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        val kind = (d["kind"] ?: return).let { k -> if (CHANNELS.any { it.first == k }) k else "ping" }
        val tag = d["tag"] ?: kind
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        if (visible && d["test"] != "1" && (tag != "reply" || onChat)) return
        val open = PendingIntent.getActivity(ctx, tag.hashCode(), Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val body = d["body"].orEmpty()
        val n = Notification.Builder(ctx, kind).setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(d["title"].orEmpty()).setContentText(body).setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(open).setAutoCancel(true).setOnlyAlertOnce(d["alert"] != "1")
            .setCategory(if (tag == "reply") Notification.CATEGORY_MESSAGE else Notification.CATEGORY_REMINDER)
        d["group"]?.let { n.setGroup(it) }
        val aid = d["aid"]
        if (aid != null) {
            val perm = d["perm"] == "1"
            for ((approve, label) in listOf(false to "Deny", true to "Allow")) {
                val pi = PendingIntent.getBroadcast(ctx, (aid + label).hashCode(),
                    Intent(ctx, Decide::class.java).putExtra("aid", aid).putExtra("approve", approve)
                        .putExtra("option", if (!perm) null else if (approve) "once" else "deny"),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
                n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify), label, pi).build())
            }
        }
        nm.notify(tag, 0, n.build())
    }

    private const val MAUVE = 0xFFCBA6F7.toInt()
}

class FcmService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        Prefs(this).pushToken = token
    }

    override fun onMessageReceived(msg: RemoteMessage) {
        val d = msg.data
        if (d["kind"] == "adb") return d["id"]?.let { AdbRelay.wake(this, it) } ?: Unit
        Notify.show(this, d)
    }
}

/** Allow or Deny on a card's notification: the watch's key signs in (it works only while the watch is unlocked) and settles that one card. */
class Decide : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val aid = intent.getStringExtra("aid") ?: return
        val body = JSONObject().put("decision", if (intent.getBooleanExtra("approve", false)) "approve" else "deny")
        intent.getStringExtra("option")?.let { body.put("option", it) }
        val done = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                Session.login(ctx).post("api/approvals/$aid", body)
                Notify.settled(ctx, aid)
            } catch (e: HttpError) {
                if (e.code == 409) Notify.settled(ctx, aid)
                else again(ctx, aid)
            } catch (e: IOException) {
                again(ctx, aid)
            } catch (e: Exception) {
                // The key refuses while the watch is locked.
                again(ctx, aid)
            } finally {
                done.finish()
            }
        }
    }

    private fun again(ctx: Context, aid: String) = Notify.show(ctx, mapOf("kind" to "approval", "tag" to "approval:$aid",
        "title" to "Couldn't send your answer", "body" to "Tap to answer in seimei", "test" to "1"))
}

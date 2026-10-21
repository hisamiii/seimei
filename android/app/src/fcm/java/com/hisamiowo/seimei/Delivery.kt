package com.hisamiowo.seimei

import android.content.Context
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject
import kotlin.coroutines.resume

/** Notifications through Firebase Cloud Messaging, configured at build time (none means no notifications). */
object Delivery {
    val available get() = BuildConfig.FCM_APP.isNotEmpty()

    fun start(ctx: Context) {
        if (!available || FirebaseApp.getApps(ctx).isNotEmpty()) return
        FirebaseApp.initializeApp(ctx, FirebaseOptions.Builder()
            .setApplicationId(BuildConfig.FCM_APP).setApiKey(BuildConfig.FCM_KEY)
            .setProjectId(BuildConfig.FCM_PROJECT).setGcmSenderId(BuildConfig.FCM_SENDER).build())
    }

    /** This phone's Firebase token, or null without Firebase. */
    private suspend fun token(): String? {
        if (!available) return null
        return suspendCancellableCoroutine { c ->
            FirebaseMessaging.getInstance().token
                .addOnSuccessListener { c.resume(it) }
                .addOnFailureListener { c.resume(null) }
        }
    }

    /** After each login: hands the laptop this phone's token, which then may push to it. */
    suspend fun register(ctx: Context, api: Api, prefs: Prefs) {
        val token = token() ?: prefs.pushToken ?: return
        prefs.pushToken = token
        api.post("api/devices/push", JSONObject().put("token", token))
    }

    /** The phone was unpaired: nothing to undo, the laptop forgot the token. */
    fun forget(ctx: Context, prefs: Prefs) {}
}

class Notify : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        // Sent to the laptop at the next login; the token alone opens nothing.
        Prefs(this).pushToken = token
    }

    override fun onMessageReceived(msg: RemoteMessage) = Push.show(this, msg.data)
}

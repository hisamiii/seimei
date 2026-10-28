package com.hisamiowo.seimei

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.INSTANCE_DEFAULT
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage
import java.io.IOException
import kotlin.coroutines.resume

/**
 * Notifications without Google: the phone's UnifiedPush app (ntfy and the like) gives an endpoint, and messages come encrypted for this phone.
 * With no push app, the background connection (Listen) instead.
 */
object Delivery {
    const val available = true
    const val FOSS = true
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** The logged-in API, so an endpoint the push app changes later reaches the server at once. */
    @Volatile
    private var api: Api? = null

    fun pushApps(ctx: Context): List<String> = UnifiedPush.getDistributors(ctx)

    /** The push app seimei registers with: the one in use, else the one the next login picks. */
    fun current(ctx: Context): String? =
        UnifiedPush.getAckDistributor(ctx) ?: UnifiedPush.getSavedDistributor(ctx) ?: pushApps(ctx).firstOrNull()

    fun label(ctx: Context, pkg: String?): String {
        pkg ?: return "none"
        val pm = ctx.packageManager
        return runCatching { pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString() }.getOrDefault(pkg)
    }

    /** Another push app: the old one lets go of seimei's endpoint, and the next register goes to this one. */
    fun pick(ctx: Context, pkg: String) {
        if (pkg == current(ctx)) return
        UnifiedPush.unregister(ctx, INSTANCE_DEFAULT)
        UnifiedPush.saveDistributor(ctx, pkg)
    }

    fun route(ctx: Context, prefs: Prefs): Route {
        val app = current(ctx)
        return when {
            app != null -> label(ctx, app).let { Route("Through $it", "End-to-end encrypted", true) }
            prefs.listen -> Route("No push app on this phone",
                "seimei keeps its own connection instead. A push app saves battery.", false)
            else -> Route("No notifications",
                "Install a push app, or turn on the background connection", false)
        }
    }

    fun start(ctx: Context) {
        // A restarted process picks the background connection back up.
        if (listening(ctx)) Listen.start(ctx)
    }

    fun listening(ctx: Context): Boolean {
        val prefs = Prefs(ctx)
        return prefs.listen && prefs.listenSecret != null && prefs.device != null && pushApps(ctx).isEmpty()
    }

    /** After each login: a push app if the phone has one, else the background connection, else nothing. */
    suspend fun register(ctx: Context, api: Api, prefs: Prefs) {
        this.api = api
        if (pushApps(ctx).isNotEmpty()) {
            Listen.stop(ctx)
            prefs.listenSecret = null
            prefs.upEndpoint?.let { send(api, it) }
            val vapid = api.get("api/devices/push").optString("vapid").ifEmpty { null }
            withContext(Dispatchers.Main) {
                val ok = suspendCancellableCoroutine { c -> UnifiedPush.tryUseCurrentOrDefaultDistributor(ctx) { c.resume(it) } }
                // Several push apps and none chosen: the first, until the user picks another.
                if (!ok) UnifiedPush.saveDistributor(ctx, pushApps(ctx).first())
                // The push app answers with the endpoint (UpService.onNewEndpoint), also when it is unchanged.
                UnifiedPush.register(ctx, INSTANCE_DEFAULT, "seimei", vapid)
            }
        } else if (prefs.listen) {
            prefs.upEndpoint = null
            val r = api.post("api/devices/push", JSONObject().put("ws", true))
            prefs.listenSecret = r.getString("ws")
            Listen.start(ctx)
        } else {
            Listen.stop(ctx)
            prefs.listenSecret = null
            prefs.upEndpoint = null
            api.post("api/devices/push", JSONObject())
        }
    }

    private suspend fun send(api: Api, endpoint: String) {
        api.post("api/devices/push", JSONObject().put("up", JSONObject(endpoint)))
    }

    /** The push app gave a (new) endpoint: kept for the next login, and sent now when logged in. */
    fun endpoint(ctx: Context, endpoint: String) {
        Prefs(ctx).upEndpoint = endpoint
        val api = api ?: return
        scope.launch {
            try {
                send(api, endpoint)
            } catch (e: IOException) {
                Log.w("seimei", "could not hand the server the push endpoint: $e")
            }
        }
    }

    /** The phone was unpaired: stop listening and let the push app drop the endpoint. */
    fun forget(ctx: Context, prefs: Prefs) {
        api = null
        Listen.stop(ctx)
        prefs.listenSecret = null
        prefs.upEndpoint = null
        if (pushApps(ctx).isNotEmpty()) UnifiedPush.unregister(ctx, INSTANCE_DEFAULT)
    }
}

/** The push app's side: endpoints and messages (decrypted by the connector with this phone's Web Push keys). */
class UpService : PushService() {
    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        // Without keys the server would have to send in the clear; it refuses that, so this endpoint is no use.
        val keys = endpoint.pubKeySet ?: run {
            Log.w("seimei", "the push app gave an endpoint without keys")
            return
        }
        Delivery.endpoint(this, JSONObject().put("endpoint", endpoint.url).put("p256dh", keys.pubKey).put("auth", keys.auth).toString())
    }

    override fun onMessage(message: PushMessage, instance: String) {
        if (!message.decrypted) {
            Log.w("seimei", "dropped a push that was not encrypted for this phone")
            return
        }
        val json = JSONObject(String(message.content))
        Push.show(this, json.keys().asSequence().associateWith { json.getString(it) })
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w("seimei", "push app registration failed: $reason")
        Prefs(this).upEndpoint = null
    }

    override fun onUnregistered(instance: String) {
        Prefs(this).upEndpoint = null
    }
}

/** After a reboot or an update, the background connection comes back on its own. */
class Boot : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action in setOf(Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED) && Delivery.listening(ctx)) {
            Listen.start(ctx)
        }
    }
}

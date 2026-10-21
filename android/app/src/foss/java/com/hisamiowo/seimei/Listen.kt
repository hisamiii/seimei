package com.hisamiowo.seimei

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * The background connection, for phones without a push app: a foreground service holding one websocket to the
 * laptop, which sends it what it would push. Android requires the lasting notification that says so.
 */
class Listen : Service() {
    companion object {
        private const val ID = 7

        fun start(ctx: Context) {
            ctx.startForegroundService(Intent(ctx, Listen::class.java))
        }

        fun stop(ctx: Context) {
            ctx.stopService(Intent(ctx, Listen::class.java))
        }
    }

    // Pings every 30 s find a dead link (a network switch, a sleeping laptop); no read timeout between messages.
    private val client = OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(0, TimeUnit.SECONDS)
        .pingInterval(30, TimeUnit.SECONDS).build()
    private val main = Handler(Looper.getMainLooper())
    private var socket: WebSocket? = null
    private var open = false
    private var failures = 0
    private var stopped = false
    private val net = object : ConnectivityManager.NetworkCallback() {
        // Back on a network: try now instead of waiting out the backoff.
        override fun onAvailable(network: Network) {
            main.post { if (!open && !stopped) connect() }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(net)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForeground(ID, notice("Connecting for notifications"), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        // A new login hands out a new secret: always reconnect with the current one.
        connect()
        return START_STICKY
    }

    private fun notice(text: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, "listen").setSmallIcon(R.drawable.ic_notify).setColor(Push.MAUVE)
            .setContentTitle("seimei").setContentText(text).setContentIntent(open).setOngoing(true).build()
    }

    private fun connect() {
        main.removeCallbacksAndMessages(null)
        socket?.cancel()
        socket = null
        val prefs = Prefs(this)
        val secret = prefs.listenSecret
        val device = prefs.device
        if (secret == null || device == null) return stopSelf()
        val req = Request.Builder().url(prefs.server.trimEnd('/') + "/api/push/ws")
            .header("Authorization", "Push $device $secret").build()
        socket = client.newWebSocket(req, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                main.post {
                    if (webSocket !== socket) return@post
                    open = true
                    failures = 0
                    notify("Connected for notifications")
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                val json = runCatching { JSONObject(text) }.getOrNull() ?: return
                Push.show(this@Listen, json.keys().asSequence().associateWith { json.getString(it) })
            }

            // The laptop closed it (unpaired, or a restart): close back at once, then reconnect, which tells which.
            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
                lost(webSocket, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lost(webSocket, null)

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = lost(webSocket, response?.code)
        })
    }

    private fun lost(webSocket: WebSocket, code: Int?) {
        main.post {
            if (webSocket !== socket || stopped) return@post
            // Once per socket: closing and closed both end up here.
            socket = null
            open = false
            if (code == 401) {
                // Unpaired, or another login replaced the secret and will start this again.
                Prefs(this).listenSecret = null
                return@post stopSelf()
            }
            failures++
            notify("Reconnecting")
            // 5 s, 10 s, 20 s ... up to 5 minutes.
            main.postDelayed(::connect, minOf(300_000L, 5_000L shl minOf(failures - 1, 6)))
        }
    }

    private fun notify(text: String) {
        getSystemService(android.app.NotificationManager::class.java).notify(ID, notice(text))
    }

    override fun onDestroy() {
        stopped = true
        main.removeCallbacksAndMessages(null)
        socket?.close(1000, null)
        runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(net) }
        super.onDestroy()
    }
}

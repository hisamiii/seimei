package com.hisamiowo.seimei

import com.hisamiowo.seimei.shared.R
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString

/**
 * The adb relay (docs/watch-adb.md): adb on the server reaches this watch's own adbd, also with the watch's wifi off.
 * The server's push names a connection; this service opens it back over the app's link and copies its bytes both ways to adbd on 127.0.0.1:5555.
 * It runs in the foreground while adb works, so the system keeps the process going, and stops when the last connection closes.
 */
class AdbRelay : Service() {
    companion object {
        private const val ID = 10
        private const val ADBD = 5555
        /** Bytes queued toward the server before the copy from adbd waits: the phone's bluetooth proxy is slow. */
        private const val QUEUED = 1 shl 20

        /** The server's push for a connection: wakes the relay for it. */
        fun wake(ctx: Context, id: String) {
            try {
                ctx.startForegroundService(Intent(ctx, AdbRelay::class.java).putExtra("id", id))
            } catch (e: IllegalStateException) {
                // A push the system did not deliver as urgent may not start a foreground service.
                Log.w("seimei", "adb relay: cannot start: $e")
            }
        }

        /** After each login: from now on the server wakes this watch for adb. */
        suspend fun ready(api: Api) {
            api.post("api/adb/ready")
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    /** Open connections, counted on the main thread. */
    private var open = 0
    /** Their sockets to adbd: closing one ends its connection. */
    private val sockets = java.util.concurrent.ConcurrentHashMap.newKeySet<Socket>()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java)
            .createNotificationChannel(NotificationChannel("adb", "adb relay", NotificationManager.IMPORTANCE_MIN))
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val n = NotificationCompat.Builder(this, "adb").setSmallIcon(R.drawable.ic_notify).setContentTitle("adb relay").setOngoing(true).build()
        startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        val id = intent?.getStringExtra("id")
        if (id == null) {
            if (open == 0) stopSelf()
            return START_NOT_STICKY
        }
        open++
        scope.launch {
            try {
                relay(id)
            } finally {
                withContext(Dispatchers.Main) {
                    if (--open == 0) stopSelf()
                }
            }
        }
        return START_NOT_STICKY
    }

    /** One connection: until either side closes it. */
    private suspend fun relay(id: String) {
        val api = try {
            Session.login(this)
        } catch (e: Exception) {
            // The watch's key refuses while the watch is locked; the server gives up on its own.
            Log.w("seimei", "adb relay: no login: $e")
            return
        }
        val adbd = Socket()
        sockets.add(adbd)
        val missing = try {
            adbd.connect(InetSocketAddress("127.0.0.1", ADBD), 3000)
            adbd.tcpNoDelay = true
            null
        } catch (e: IOException) {
            // adbd listens on 5555 only after `adb tcpip 5555`, until the watch reboots.
            "adbd is not listening on $ADBD"
        }
        val done = CompletableDeferred<Unit>()
        api.socket("api/adb/ws?id=$id", object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                if (missing != null) {
                    webSocket.close(1011, missing)
                    return
                }
                Log.i("seimei", "adb relay: open")
                scope.launch { fromAdbd(adbd, webSocket) }
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                try {
                    adbd.getOutputStream().write(bytes.toByteArray())
                } catch (e: IOException) {
                    webSocket.close(1011, "adbd closed")
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                done.complete(Unit)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                Log.w("seimei", "adb relay: $t")
                done.complete(Unit)
            }
        })
        done.await()
        adbd.close()
        sockets.remove(adbd)
        Log.i("seimei", "adb relay: closed")
    }

    private fun fromAdbd(adbd: Socket, ws: WebSocket) {
        val buf = ByteArray(64 * 1024)
        try {
            val input = adbd.getInputStream()
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                while (ws.queueSize() > QUEUED) Thread.sleep(20)
                if (!ws.send(buf.toByteString(0, n))) return
            }
        } catch (e: IOException) {
            // Closed from the other side.
        }
        ws.close(1000, null)
    }

    override fun onTimeout(startId: Int, fgsType: Int) {
        // Android's daily limit for this kind of service: adb sees the watch go.
        stopSelf()
    }

    override fun onDestroy() {
        sockets.forEach { it.close() }
        scope.cancel()
        super.onDestroy()
    }
}

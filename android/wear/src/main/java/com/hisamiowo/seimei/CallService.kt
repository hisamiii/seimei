package com.hisamiowo.seimei

import com.hisamiowo.seimei.shared.R
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.compose.runtime.getValue
import androidx.core.app.NotificationCompat
import androidx.wear.ongoing.OngoingActivity
import androidx.wear.ongoing.Status
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * The voice call's foreground service: the call lives here, not in a screen, so it goes on with the screen off or another app in front.
 * Its ongoing activity has Mute and Hang up. The call itself is [call], which the call screen and the chat's call pill follow.
 */
class CallService : Service() {
    companion object : VoiceCall.Host {
        private const val ID = 9
        private const val MUTE = "mute"
        private const val HANG_UP = "hang-up"

        /** The call that runs now, if any. */
        var call by mutableStateOf<VoiceCall?>(null)
            private set
        /** What the call has to tell the user, for the app to show once. */
        var told by mutableStateOf<String?>(null)
        private var app: Context? = null

        /** Starts a call: the microphone first, then the service, which Android lets start only while the app is in front. */
        fun start(ctx: Context, api: Api, speed: Double?): Boolean {
            if (call != null) return true
            val c = VoiceCall(ctx.applicationContext, api, speed, Barge.learned(Prefs(ctx).voiceEcho), this, soft = true)
            if (!c.start()) {
                told = "Couldn't open the microphone"
                return false
            }
            app = ctx.applicationContext
            call = c
            ctx.startForegroundService(Intent(ctx, CallService::class.java))
            return true
        }

        override fun tell(text: String) {
            told = text
        }

        override fun ended(call: VoiceCall, why: String) {
            if (this.call !== call) return
            val ctx = app ?: return
            Prefs(ctx).voiceEcho = call.echo
            // The service sees it and stops: stopped from here before its startForeground, it would crash the app.
            this.call = null
            if (why.isNotEmpty()) told = why
        }
    }

    private val scope = MainScope()
    private val since = System.currentTimeMillis()
    private var foreground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel("call", "Voice call", NotificationManager.IMPORTANCE_LOW))
        scope.launch {
            snapshotFlow { call?.let { it.label to it.muted } }.collect {
                if (it != null) nm.notify(ID, notice())
                else if (foreground) stopSelf()
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Foreground first, every time: a service started for the foreground must say so even when the call is already over.
        startForeground(ID, notice(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        foreground = true
        when (intent?.action) {
            MUTE -> call?.toggleMute()
            HANG_UP -> call?.hangUp()
        }
        if (call == null) stopSelf()
        return START_NOT_STICKY
    }

    /** The ongoing notification, as an ongoing activity: Wear OS shows it on the watch face and in the recents, a tap opens the call. */
    private fun notice(): Notification {
        val c = call
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java).setAction(MainActivity.CALL), PendingIntent.FLAG_IMMUTABLE)
        fun action(label: String, what: String, code: Int) = NotificationCompat.Action(R.drawable.ic_notify, label,
            PendingIntent.getService(this, code, Intent(this, CallService::class.java).setAction(what), PendingIntent.FLAG_IMMUTABLE))
        val b = NotificationCompat.Builder(this, "call").setSmallIcon(R.drawable.ic_notify)
            .setContentTitle("Voice call").setContentText(c?.label ?: "").setContentIntent(open).setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_CALL).setWhen(since).setShowWhen(true).setUsesChronometer(true)
            .addAction(action(if (c?.muted == true) "Unmute" else "Mute", MUTE, 1))
            .addAction(action("Hang up", HANG_UP, 2))
        OngoingActivity.Builder(this, ID, b).setStaticIcon(R.drawable.ic_notify).setTouchIntent(open).setCategory(NotificationCompat.CATEGORY_CALL)
            .setStatus(Status.Builder().addTemplate("#time#").addPart("time", Status.StopwatchPart(since)).build())
            .build().apply(this)
        return b.build()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }
}

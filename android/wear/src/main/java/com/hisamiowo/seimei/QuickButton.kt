package com.hisamiowo.seimei

import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.provider.Settings
import android.util.Log

/**
 * Samsung's quick button (docs/voice.md): the watch's system UI starts this service with each of the button's key events, as it starts the voice recorder's.
 * The press the button's settings name (a short press, or a double one) opens the "seimei call" entry, which starts a call.
 * A service may open a screen from the background only with "display over other apps", which adb grants.
 */
class QuickButton : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val event = intent?.getIntExtra("ACTION_KEY_EVENT", -1) ?: -1
        val count = intent?.getIntExtra("ACTION_KEY_COUNT", 0) ?: 0
        // 0: a short press starts the action, 1: a double press.
        val double = Settings.Global.getInt(contentResolver, "setting_action_button_start_method", 0) == 1
        if (if (double) event == MULTI && count == 2 else event == PRESS) {
            try {
                startActivity(Intent().setClassName(this, "com.hisamiowo.seimei.Talk").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } catch (e: RuntimeException) {
                Log.w("seimei", "quick button: cannot open the call: $e")
            }
        }
        stopSelf(startId)
        return START_NOT_STICKY
    }

    private companion object {
        // The key events the system UI sends: 0 down, 1 up, 2 a press, 5 several presses with their count.
        const val PRESS = 2
        const val MULTI = 5
    }
}

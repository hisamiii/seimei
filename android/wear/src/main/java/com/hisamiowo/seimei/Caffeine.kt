package com.hisamiowo.seimei

import android.Manifest
import android.content.BroadcastReceiver
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.PixelFormat
import android.os.BatteryManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.view.View
import android.view.WindowManager
import androidx.wear.tiles.TileService

/**
 * Caffeine keeps the screen on for adb work: Wear OS ignores wake locks and FLAG_KEEP_SCREEN_ON, so it raises the timeout and turns off the wrist gestures.
 * A 1 px overlay keeps the process alive for the timer and the battery check; adb or the system UI's touch wakes the screen, as an app's wake gets cut after 15 s.
 * The old values are saved first and put back on off, or at the app's next start after a crash or a reboot.
 */
object Caffeine {
    const val ACTION = "com.hisamiowo.seimei.CAFFEINE"
    const val DEFAULT_MINUTES = 30
    const val LOW = 15

    /** A setting caffeine changes; [stock] is what off puts back when apps may not read the key, null for no key. */
    private class Knob(val global: Boolean, val name: String, val stock: String?)

    private val TIMEOUT = Knob(false, Settings.System.SCREEN_OFF_TIMEOUT, "30000")
    /** Wear OS's sleep when the wrist turns away from the eyes. */
    private val UNGAZE = Knob(true, "ungaze_enabled", null)
    /** Raise to wake: a raise also arms the wrist-down sleep, whatever UNGAZE says. */
    private val TILT = Knob(true, "ambient_tilt_to_wake", "1")
    private val KNOBS = listOf(TIMEOUT, UNGAZE, TILT)

    private val main = Handler(Looper.getMainLooper())
    private var view: View? = null
    private var watch: BroadcastReceiver? = null
    /** The elapsed-realtime clock's time of the auto-off, while on. */
    private var until = 0L

    val on get() = view != null

    fun minutesLeft(): Int = if (on) ((until - SystemClock.elapsedRealtime() + 59_999) / 60_000).toInt() else 0

    /** The minutes a tap on the tile turns it on for: the last ones adb asked for. */
    fun minutes(ctx: Context): Int = prefs(ctx).getInt("caffeine.minutes", DEFAULT_MINUTES)

    /** Turns it on for these minutes, or extends it; returns why it can't, or null. Main thread only. */
    fun start(ctx: Context, minutes: Int): String? {
        val app = ctx.applicationContext
        val pkg = app.packageName
        if (!Settings.canDrawOverlays(app)) return "no permission to draw over other apps (adb shell appops set $pkg SYSTEM_ALERT_WINDOW allow)"
        if (!Settings.System.canWrite(app)) return "no permission to change system settings (adb shell appops set $pkg WRITE_SETTINGS allow)"
        if (app.checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) != PackageManager.PERMISSION_GRANTED)
            return "no permission to change global settings (adb shell pm grant $pkg ${Manifest.permission.WRITE_SECURE_SETTINGS})"
        val battery = battery(app)
        if (battery?.let { low(it) } == true) return "battery at ${percent(battery)}%, under $LOW%"
        if (view == null) {
            val v = View(app)
            val lp = WindowManager.LayoutParams(1, 1, WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE, PixelFormat.TRANSPARENT)
            try {
                app.getSystemService(WindowManager::class.java).addView(v, lp)
            } catch (e: RuntimeException) {
                Log.w("seimei", "caffeine: cannot add the overlay: $e")
                return "cannot add the overlay: $e"
            }
            view = v
            val r = object : BroadcastReceiver() {
                override fun onReceive(c: Context, i: Intent) {
                    if (!low(i)) return
                    Log.i("seimei", "caffeine: off, battery at ${percent(i)}%")
                    stop(c)
                }
            }
            app.registerReceiver(r, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            watch = r
        }
        val p = prefs(app)
        val r = app.contentResolver
        val values = listOf(TIMEOUT to "${minutes * 60_000 + 60_000}", UNGAZE to "0", TILT to "0")
        val edit = p.edit().putInt("caffeine.minutes", minutes)
        for ((k, v) in values) {
            // "-" marks a key that was not set, so off removes it again.
            if (!p.contains("saved.${k.name}")) edit.putString("saved.${k.name}", (if (readable(r, k)) get(r, k) else k.stock)?.let { "=$it" } ?: "-")
            edit.putString("set.${k.name}", v)
        }
        // Saved before the change: a crash right after it still finds the old values.
        edit.commit()
        for ((k, v) in values) put(r, k, v)
        until = SystemClock.elapsedRealtime() + minutes * 60_000L
        main.removeCallbacksAndMessages(null)
        main.postDelayed({ stop(app) }, minutes * 60_000L)
        Log.i("seimei", "caffeine: on for $minutes min")
        refresh(app)
        return null
    }

    fun stop(ctx: Context) {
        val app = ctx.applicationContext
        main.removeCallbacksAndMessages(null)
        watch?.let { app.unregisterReceiver(it) }
        watch = null
        view?.let { app.getSystemService(WindowManager::class.java).removeView(it) }
        if (view != null) Log.i("seimei", "caffeine: off")
        view = null
        until = 0
        restore(app)
        refresh(app)
    }

    /** Puts the saved values back; at each app start too, as caffeine never outlives its process. */
    fun restore(ctx: Context) {
        val p = prefs(ctx)
        val r = ctx.contentResolver
        for (k in KNOBS) {
            val saved = p.getString("saved.${k.name}", null) ?: continue
            // Someone else's newer value stays.
            if (!readable(r, k) || get(r, k) == p.getString("set.${k.name}", null)) {
                try {
                    put(r, k, if (saved == "-") null else saved.substring(1))
                } catch (e: SecurityException) {
                    Log.w("seimei", "caffeine: cannot restore ${k.name}: $e")
                    continue
                }
            }
            p.edit().remove("saved.${k.name}").remove("set.${k.name}").commit()
        }
    }

    /** Android hides some settings keys from apps' reads, not from their writes. */
    private fun readable(r: ContentResolver, k: Knob): Boolean = try {
        get(r, k)
        true
    } catch (e: SecurityException) {
        false
    }

    private fun get(r: ContentResolver, k: Knob): String? =
        if (k.global) Settings.Global.getString(r, k.name) else Settings.System.getString(r, k.name)

    private fun put(r: ContentResolver, k: Knob, v: String?) {
        if (k.global) Settings.Global.putString(r, k.name, v) else Settings.System.putString(r, k.name, v)
    }

    fun status(ctx: Context): String {
        val b = battery(ctx.applicationContext)?.let { ", battery ${percent(it)}%" + if (charging(it)) " charging" else "" } ?: ""
        return if (on) "on, ${minutesLeft()} min left$b" else "off$b"
    }

    private fun refresh(ctx: Context) {
        TileService.getUpdater(ctx).requestUpdate(CaffeineTile::class.java)
    }

    private fun prefs(ctx: Context) = ctx.getSharedPreferences("caffeine", Context.MODE_PRIVATE)

    private fun battery(ctx: Context): Intent? = ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))

    private fun percent(i: Intent): Int = i.getIntExtra(BatteryManager.EXTRA_LEVEL, 0) * 100 / i.getIntExtra(BatteryManager.EXTRA_SCALE, 100)

    private fun charging(i: Intent): Boolean = i.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0

    /** On the charger the battery never counts as low. */
    private fun low(i: Intent): Boolean = !charging(i) && i.hasExtra(BatteryManager.EXTRA_LEVEL) && percent(i) < LOW
}

/**
 * Caffeine over adb: `am broadcast -n <app>/com.hisamiowo.seimei.CaffeineReceiver -a com.hisamiowo.seimei.CAFFEINE --es mode on|off|status [--ei minutes N]`.
 * The manifest guards it with DUMP, which adb's shell holds and apps can't get, so other apps on the watch can't send it.
 * The broadcast's result data is the status line.
 */
class CaffeineReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != Caffeine.ACTION) return
        val error = when (intent.getStringExtra("mode") ?: "status") {
            "on" -> Caffeine.start(ctx, intent.getIntExtra("minutes", Caffeine.minutes(ctx)).coerceIn(1, 24 * 60))
            "off" -> Caffeine.stop(ctx).let { null }
            "status" -> null
            else -> "mode is on, off or status"
        }
        resultCode = if (error == null) 0 else 1
        resultData = error ?: Caffeine.status(ctx)
    }
}

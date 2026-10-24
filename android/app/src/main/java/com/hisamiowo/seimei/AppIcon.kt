package com.hisamiowo.seimei

import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager

// ---------- the app icon: two launcher activity-aliases in the manifest, one enabled at a time ----------

val APP_ICONS = listOf("default" to "Default", "arcticon" to "Arcticon")

private fun alias(ctx: Context, icon: String) =
    ComponentName(ctx, "com.hisamiowo.seimei.Icon" + icon.replaceFirstChar { it.uppercase() })

/** The icon the launcher shows: the PackageManager state is the only record of it. */
fun appIcon(ctx: Context): String =
    if (ctx.packageManager.getComponentEnabledSetting(alias(ctx, "arcticon")) == PackageManager.COMPONENT_ENABLED_STATE_ENABLED) "arcticon"
    else "default"

fun setAppIcon(ctx: Context, icon: String) {
    val pm = ctx.packageManager
    // Enable the new entry before the old one goes, so the launcher never sees the app without one.
    pm.setComponentEnabledSetting(alias(ctx, icon), PackageManager.COMPONENT_ENABLED_STATE_ENABLED, PackageManager.DONT_KILL_APP)
    for ((other, _) in APP_ICONS) if (other != icon)
        pm.setComponentEnabledSetting(alias(ctx, other), PackageManager.COMPONENT_ENABLED_STATE_DISABLED, PackageManager.DONT_KILL_APP)
}

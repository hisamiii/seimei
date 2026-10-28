package com.hisamiowo.seimei

import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.remember
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.text.font.FontFamily
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/** A newer signed APK on the server (app.json from publish.sh). */
data class Release(val version: String, val code: Int, val size: Long)

/** Where an update stands; the sheet shows it. */
sealed interface Updating {
    data object Idle : Updating
    data class Downloading(val done: Long, val total: Long) : Updating
    /** Handed to the system installer; the user confirms there. */
    data object Confirming : Updating
    data class Failed(val why: String) : Updating
}

/**
 * Updates through the system package installer, never silently: the session asks the user to confirm, and Android
 * checks that the APK carries the same signing key before it installs over this one.
 */
object Updater {
    var state by mutableStateOf<Updating>(Updating.Idle)

    /** Own release builds update themselves; debug and store builds do not (build.gradle.kts, UPDATER). */
    val enabled get() = BuildConfig.UPDATER

    fun canInstall(ctx: Context) = ctx.packageManager.canRequestPackageInstalls()

    fun allowInstalls(ctx: Context) = ctx.startActivity(
        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${ctx.packageName}")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))

    suspend fun install(ctx: Context, api: Api, r: Release) {
        val apk = File(ctx.cacheDir, "update.apk")
        try {
            state = Updating.Downloading(0, r.size)
            apk.outputStream().use { out -> api.download("api/app/seimei.apk", out) { state = Updating.Downloading(it, r.size) } }
            state = Updating.Confirming
            withContext(Dispatchers.IO) {
                val pi = ctx.packageManager.packageInstaller
                val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
                    setAppPackageName(ctx.packageName)
                    if (android.os.Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
                }
                val id = pi.createSession(params)
                pi.openSession(id).use { s ->
                    s.openWrite("seimei.apk", 0, apk.length()).use { out ->
                        apk.inputStream().use { it.copyTo(out) }
                        s.fsync(out)
                    }
                    val back = PendingIntent.getBroadcast(ctx, id, Intent(ctx, InstallResult::class.java).setPackage(ctx.packageName),
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE)
                    s.commit(back.intentSender)
                }
            }
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            state = Updating.Failed(if (e is HttpError) "The server has no app to give (${e.code})" else "Couldn't download the update")
        } finally {
            if (state !is Updating.Confirming) apk.delete()
        }
    }
}

/** The installer's answers: show the confirmation it asks for, or say why it failed. */
class InstallResult : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
                ctx.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            // On success Android restarts the app as the new version; this process ends.
            PackageInstaller.STATUS_SUCCESS -> Updater.state = Updating.Idle
            PackageInstaller.STATUS_FAILURE_ABORTED -> Updater.state = Updating.Idle
            else -> Updater.state = Updating.Failed(
                intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)?.let { "The install failed: $it" } ?: "The install failed")
        }
        if (Updater.state !is Updating.Confirming) File(ctx.cacheDir, "update.apk").delete()
    }
}

fun sizeMb(n: Long) = "%.1f MB".format(n / 1048576.0)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AboutSheet(vm: Seimei, onClose: () -> Unit) {
    val ctx = LocalContext.current
    val r = vm.release
    val st = Updater.state
    ModalBottomSheet(onClose, containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("seimei for Android", fontSize = 20.sp)
            Text("Version ${BuildConfig.VERSION_NAME}", fontSize = 14.sp, color = LocalExtra.current.text3)
            Spacer(Modifier.height(10.dp))
            when {
                !Updater.enabled -> Text("Debug builds don't update themselves", fontSize = 14.sp, color = LocalExtra.current.text2)
                r == null -> {
                    Text(if (vm.checking) "Checking for updates…" else "Up to date", fontSize = 15.sp)
                    TextButton(vm::checkUpdate, enabled = !vm.checking) { Text("Check again") }
                }
                else -> {
                    Text("Update available: ${r.version}", fontSize = 16.sp, color = MaterialTheme.colorScheme.primary)
                    Text(sizeMb(r.size),
                        fontSize = 13.sp, color = LocalExtra.current.text3)
                    Spacer(Modifier.height(8.dp))
                    when (st) {
                        is Updating.Downloading -> {
                            LinearProgressIndicator({ if (st.total > 0) st.done.toFloat() / st.total else 0f }, Modifier.fillMaxWidth())
                            Text("Downloading…", fontSize = 13.sp, color = LocalExtra.current.text3)
                        }
                        Updating.Confirming -> Text("Waiting for you to confirm the install", fontSize = 14.sp)
                        else -> {
                            if (st is Updating.Failed) Text(st.why, fontSize = 13.sp, color = MaterialTheme.colorScheme.error)
                            if (vm.installAllowed) {
                                Button(vm::installUpdate, Modifier.fillMaxWidth()) { Text("Download and install") }
                            } else {
                                Text("Allow seimei to install updates first",
                                    fontSize = 13.sp, color = LocalExtra.current.text2)
                                Button({ Updater.allowInstalls(ctx) }, Modifier.fillMaxWidth()) { Text("Allow updates from seimei") }
                            }
                        }
                    }
                }
            }
            // The bundled font's license travels with the app (assets/licenses).
            var license by remember { mutableStateOf<String?>(null) }
            TextButton({ license = if (license == null) ctx.assets.open("licenses/OFL-GoogleSansFlex.txt").bufferedReader().use { it.readText() } else null },
                Modifier.heightIn(min = 48.dp)) {
                Text(if (license == null) "Font: Google Sans Flex, SIL Open Font License" else "Hide the font license", fontSize = 13.sp)
            }
            license?.let { Text(it, fontSize = 11.sp, lineHeight = 15.sp, color = LocalExtra.current.text3, fontFamily = FontFamily.Monospace) }
        }
    }
}

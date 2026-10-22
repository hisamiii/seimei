package com.hisamiowo.seimei

import kotlinx.coroutines.launch
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import android.Manifest
import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.core.content.IntentCompat
import android.content.pm.PackageManager
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.material3.Typography
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner

// ---------- theme: the picked theme's tokens (Theme.kt) arranged as Material 3 tonal roles, as on the web page ----------

/** Colors the Material roles do not name: good, warning, and the dimmer text tones. */
data class Extra(val ok: Color, val warn: Color, val text2: Color, val text3: Color)

val LocalExtra = androidx.compose.runtime.staticCompositionLocalOf { Extra(MOCHA.ok, MOCHA.warn, MOCHA.text2, MOCHA.text3) }

/** Google Sans Flex (OFL), bundled: Latin only, pinned to wght 400-700. Cyrillic falls back to the system Roboto. */
@OptIn(ExperimentalTextApi::class)
val Sans = FontFamily(listOf(400, 500, 600, 700).map { w ->
    Font(R.font.google_sans_flex, FontWeight(w), variationSettings = FontVariation.Settings(FontVariation.weight(w)))
})

private val Base = Typography()
val SeimeiType = Typography(
    displayLarge = Base.displayLarge.copy(fontFamily = Sans), displayMedium = Base.displayMedium.copy(fontFamily = Sans),
    displaySmall = Base.displaySmall.copy(fontFamily = Sans), headlineLarge = Base.headlineLarge.copy(fontFamily = Sans),
    headlineMedium = Base.headlineMedium.copy(fontFamily = Sans), headlineSmall = Base.headlineSmall.copy(fontFamily = Sans),
    titleLarge = Base.titleLarge.copy(fontFamily = Sans), titleMedium = Base.titleMedium.copy(fontFamily = Sans),
    titleSmall = Base.titleSmall.copy(fontFamily = Sans), bodyLarge = Base.bodyLarge.copy(fontFamily = Sans),
    bodyMedium = Base.bodyMedium.copy(fontFamily = Sans), bodySmall = Base.bodySmall.copy(fontFamily = Sans),
    labelLarge = Base.labelLarge.copy(fontFamily = Sans), labelMedium = Base.labelMedium.copy(fontFamily = Sans),
    labelSmall = Base.labelSmall.copy(fontFamily = Sans),
)

/** The current text style with tabular figures, for numbers that tick or line up (usage, timers). */
val TNUM: TextStyle
    @Composable get() = androidx.compose.material3.LocalTextStyle.current.copy(fontFeatureSettings = "tnum")

/** Light or dark: the system's unless pinned ("light", "dark"). */
@Composable
fun isDark(mode: String) = when (mode) { "dark" -> true; "light" -> false; else -> androidx.compose.foundation.isSystemInDarkTheme() }

@Composable
fun SeimeiTheme(theme: String, mode: String, accent: String?, content: @Composable () -> Unit) {
    val ctx = LocalContext.current
    val dark = isDark(mode)
    val t = androidx.compose.runtime.remember(theme, dark, accent) { themeTokens(ctx, theme, dark, accent) }
    val scheme = androidx.compose.runtime.remember(t, dark) { schemeOf(t, dark) }
    androidx.compose.runtime.CompositionLocalProvider(LocalTokens provides t, LocalExtra provides Extra(t.ok, t.warn, t.text2, t.text3)) {
        MaterialTheme(colorScheme = scheme, typography = SeimeiType, content = content)
    }
}

// ---------- icons: the web page's SVG paths, drawn as strokes ----------

fun icon(vararg d: String, stroke: Float = 2f): ImageVector = ImageVector.Builder(
    defaultWidth = 24.dp, defaultHeight = 24.dp, viewportWidth = 24f, viewportHeight = 24f,
).apply {
    for (p in d) addPath(
        pathData = PathParser().parsePathString(p).toNodes(),
        stroke = SolidColor(Color.Black), strokeLineWidth = stroke,
        strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round,
    )
}.build()

object Icons {
    val chat = icon("M6 4.5h12A2.5 2.5 0 0 1 20.5 7v8a2.5 2.5 0 0 1-2.5 2.5h-7L6.5 21v-3.5H6A2.5 2.5 0 0 1 3.5 15V7A2.5 2.5 0 0 1 6 4.5z", stroke = 1.8f)
    val agents = icon("M12 3 13.9 8.6 19.5 10.5 13.9 12.4 12 18 10.1 12.4 4.5 10.5 10.1 8.6ZM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z", stroke = 1.8f)
    val usage = icon("M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 1 0 0-17z", stroke = 1.8f)
    val star = icon("M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z", stroke = 1.6f)
    val check = icon("M5 12.5l4.5 4.5L19 7.5")
    val stopped = icon("M7.5 7.5h9v9h-9z")
    val info = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.5v.01", stroke = 1.8f)
    val restarted = icon("M4 12a8 8 0 1 0 2.4-5.7M4 4.5v4h4")
    val chev = icon("M9 6l6 6-6 6")
    val send = icon("M12 19V5M5.5 11.5 12 5l6.5 6.5", stroke = 2.4f)
    val photo = icon("M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM9 8.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 1 0 0-3.2zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5", stroke = 1.8f)
    val more = icon("M12 5.5v.01M12 12v.01M12 18.5v.01")
    val close = icon("M6 6l12 12M18 6 6 18")
    val reply = icon("M9 14 4 9l5-5", "M4 9h10.5a5.5 5.5 0 0 1 0 11H11")
    val attach = icon("M20 11.5l-7.8 7.8a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8", stroke = 1.8f)
    val download = icon("M12 4v11M7 10.5l5 5 5-5M5 20h14", stroke = 1.8f)
    val open = icon("M14 4h6v6M20 4l-9 9M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10", stroke = 1.8f)
    val doc = icon("M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6", stroke = 1.8f)
    val archive = icon("M4 7.5 12 3.5l8 4v9l-8 4-8-4zM4 7.5l8 4 8-4M12 11.5v9", stroke = 1.8f)
    val audio = icon("M9 18V5.5l10-2v12.5M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0zM19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0z", stroke = 1.8f)
    val video = icon("M4 6.5h11v11H4zM15 10.5l5-3v9l-5-3", stroke = 1.8f)
    val code = icon("M9 8l-4 4 4 4M15 8l4 4-4 4", stroke = 1.8f)
    val lock = icon("M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z")
    val fail = icon("M12 7v6M12 17v.01M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z")
    val ask = icon("M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17v.01")
    val fingerprint = icon("M7 20c.6-1.6 1-3.6 1-6.5a4 4 0 0 1 8 0c0 1.2-.1 2.4-.3 3.5M11.2 21c.5-2 .8-4.5.8-7.5M4.5 16.5c.3-1 .5-2 .5-3a7 7 0 0 1 12-4.9M19 11c.2.8.2 1.7.1 2.6M15 20.5c.3-.8.5-1.6.6-2.5", stroke = 1.8f)
    val bash = icon("M4 17l5-5-5-5M11 19h9")
    val read = icon("M6 3h9l4 4v14H6zM14 3v5h5")
    val edit = icon("M4 20h4L19 9l-4-4L4 16zM13 7l4 4")
    val search = icon("M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4")
    /** Archive (#43): a box with a lid; Icons.archive is a package. */
    val shelf = icon("M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4", stroke = 1.8f)
    val web = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18")
    val tool = icon("M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z")
    val laptop = icon("M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18")
    val list = icon("M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01")
    val alert = icon("M12 7v6M12 17v.01")
    val dash = icon("M7 12h10")
    val back = icon("M15 5l-7 7 7 7")
    val down = icon("M12 5v14M5.5 12.5 12 19l6.5-6.5", stroke = 2.2f)
    val question = icon("M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1")
}

// ---------- activity ----------

class MainActivity : ComponentActivity() {
    private val vm: Seimei by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = vm.onStart()
            override fun onStop(owner: LifecycleOwner) = vm.onStop()
        })
        openTab(intent)
        if (savedInstanceState == null) takeShare(intent)
        setContent {
            SeimeiTheme(vm.theme, vm.themeMode, vm.accent) {
                // Status and navigation bar icons follow the theme's side, not only the system's.
                val dark = isDark(vm.themeMode)
                androidx.compose.runtime.LaunchedEffect(dark) {
                    val clear = android.graphics.Color.TRANSPARENT
                    enableEdgeToEdge(
                        if (dark) androidx.activity.SystemBarStyle.dark(clear) else androidx.activity.SystemBarStyle.light(clear, clear),
                        if (dark) androidx.activity.SystemBarStyle.dark(clear) else androidx.activity.SystemBarStyle.light(clear, clear),
                    )
                }
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
                    when (vm.phase) {
                        Phase.Pair -> if (vm.pairServer == null) ServerScreen(vm) else PairScreen(vm, this)
                        Phase.Locked -> LockScreen(vm, this)
                        Phase.Ready -> AppScreen(vm)
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        openTab(intent)
        takeShare(intent)
    }

    /** The share sheet: a link or text from a browser, photos, any files. */
    private fun takeShare(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND && intent?.action != Intent.ACTION_SEND_MULTIPLE) return
        val uris = if (intent.action == Intent.ACTION_SEND)
            listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
        else IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString().orEmpty()
        val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT).orEmpty()
        // A browser shares the page title as the subject and the link as the text.
        val body = if (subject.isNotBlank() && !text.contains(subject)) "$subject\n$text".trim() else text
        intent.action = null
        if (body.isNotBlank() || uris.isNotEmpty()) vm.share(Shared(body, uris))
    }

    /** A tapped notification names the tab it is about. */
    private fun openTab(intent: Intent?) {
        intent?.getStringExtra("tab")?.takeIf { it in TABS }?.let { vm.show(it) }
        // "Reply ready": the chat opens at that message.
        intent?.getStringExtra("event")?.toLongOrNull()?.let {
            if (vm.lane == "k") vm.showLane("c")
            vm.focus = it
        }
        intent?.removeExtra("tab")
        intent?.removeExtra("event")
    }
}

// ---------- pairing and unlocking ----------

/** The seiman (the launcher icon's glyph), in onPrimaryContainer. */
@Composable
fun Glyph(size: Dp) {
    // The drawable is the adaptive icon's 108-unit layer and the glyph spans its middle 60: drawn 1.8x, the glyph
    // fills size (the rest is transparent).
    Box(Modifier.size(size), contentAlignment = Alignment.Center) {
        Icon(painterResource(R.drawable.ic_seimei_monochrome), null, Modifier.requiredSize(size * 1.8f),
            tint = MaterialTheme.colorScheme.onPrimaryContainer)
    }
}

@Composable
fun Mark(size: Int = 72) {
    Box(
        Modifier.size(size.dp).clip(RoundedCornerShape((size * 0.28f).dp)).background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Glyph((size * 0.6f).dp)
    }
}

/** The first-run screens' frame: the mark, a title and a line under it. */
@Composable
fun FirstRun(title: String, line: String, content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(56.dp))
        Mark()
        Spacer(Modifier.height(20.dp))
        Text(title, style = MaterialTheme.typography.headlineSmall)
        Spacer(Modifier.height(8.dp))
        Text(line, color = LocalExtra.current.text3, textAlign = TextAlign.Center, fontSize = 14.sp)
        Spacer(Modifier.height(28.dp))
        content()
        Spacer(Modifier.height(24.dp))
    }
}

/** Step 1 when no server is built in: the address, checked before going on. */
@Composable
fun ServerScreen(vm: Seimei) {
    var typed by remember { mutableStateOf(vm.lastServer.removePrefix("https://")) }
    FirstRun("Connect to your seimei", "The address of the computer that runs seimei. Its page shows it under Phone app.") {
        OutlinedTextField(
            typed,
            { typed = it.trim(); vm.serverError = null },
            label = { Text("Server address") },
            singleLine = true,
            isError = vm.serverError != null,
            supportingText = vm.serverError?.let { { Text(it) } },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { vm.checkServer(typed) }),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(16.dp))
        Button(
            onClick = { vm.checkServer(typed) },
            enabled = !vm.authBusy && typed.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(56.dp),
        ) { Text(if (vm.authBusy) "Checking" else "Continue", fontSize = 16.sp) }
    }
}

/** Step 2: pair with a code from the web page, or with the password. */
@Composable
fun PairScreen(vm: Seimei, activity: Activity) {
    var code by remember { mutableStateOf("") }
    // Not rememberSaveable: the password never goes into saved state.
    var password by remember { mutableStateOf("") }
    var withPassword by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf(Build.MODEL ?: "phone") }
    val line = if (withPassword) "Type seimei's password. The phone pairs itself and keeps only its own key; the password is not stored."
        else "On the seimei page, open the menu, then Phone app, then Pair a phone. Type the code it shows."
    FirstRun("Pair this phone", line) {
        ServerChip(vm)
        Spacer(Modifier.height(20.dp))
        if (withPassword) {
            OutlinedTextField(
                password,
                { password = it },
                label = { Text("Password") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
        } else {
            OutlinedTextField(
                value = code,
                // Kept raw; capitals and the dash are display only, so the keyboard's composing text stays in sync.
                onValueChange = { v -> code = v.filter { it.isLetterOrDigit() }.take(8) },
                visualTransformation = CodeFormat,
                label = { Text("Code") },
                singleLine = true,
                textStyle = MaterialTheme.typography.headlineSmall.copy(fontFamily = FontFamily.Monospace, letterSpacing = 3.sp),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(name, { name = it.take(60) }, label = { Text("Name for this phone") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        vm.authError?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center, fontSize = 14.sp)
        }
        Spacer(Modifier.height(24.dp))
        Button(
            onClick = { if (withPassword) vm.pair(activity, name, password = password) else vm.pair(activity, name, code = code) },
            enabled = !vm.authBusy && (if (withPassword) password.isNotEmpty() else code.length == 8),
            modifier = Modifier.fillMaxWidth().height(56.dp),
        ) { Text(if (vm.authBusy) "Pairing" else "Pair", fontSize = 16.sp) }
        TextButton({
            withPassword = !withPassword
            password = ""
            vm.authError = null
        }) { Text(if (withPassword) "Use a code instead" else "Use the password instead") }
    }
}

/** The server being paired with; Change goes back to the server step. */
@Composable
fun ServerChip(vm: Seimei) {
    Row(
        Modifier.clip(CircleShape).background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(start = 14.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.web, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(8.dp))
        Text(vm.pairServer.orEmpty().substringAfter("://"), fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false))
        TextButton({ vm.changeServer() }, enabled = !vm.authBusy) { Text("Change") }
    }
}

/** Shows a typed code as ABCD-EFGH. */
object CodeFormat : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val raw = text.text.uppercase()
        val out = if (raw.length > 4) raw.take(4) + "-" + raw.drop(4) else raw
        return TransformedText(AnnotatedString(out), object : OffsetMapping {
            override fun originalToTransformed(offset: Int) = if (offset > 4) offset + 1 else offset
            override fun transformedToOriginal(offset: Int) = if (offset > 4) offset - 1 else offset
        })
    }
}

@Composable
fun LockScreen(vm: Seimei, activity: Activity) {
    LaunchedEffect(Unit) { vm.unlock(activity) }
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Mark()
        Spacer(Modifier.height(20.dp))
        Text("seimei", style = MaterialTheme.typography.headlineSmall)
        vm.authError?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center, fontSize = 14.sp)
        }
        Spacer(Modifier.height(32.dp))
        Button({ vm.unlock(activity) }, enabled = !vm.authBusy, modifier = Modifier.height(52.dp)) {
            Icon(Icons.fingerprint, null, Modifier.size(20.dp))
            Spacer(Modifier.width(10.dp))
            Text("Unlock", fontSize = 16.sp)
        }
    }
}

// ---------- the app: header, tabs, navigation bar ----------

data class Status(val s: String, val text: String)

fun status(vm: Seimei): Status = when {
    !vm.online && vm.awaySince != null -> Status("away", "away since ${clock(vm.awaySince!! / 1000.0)}")
    !vm.online -> Status("connecting", "connecting")
    !vm.connected -> Status("starting", "session starting")
    vm.waiting > 0 -> Status("waiting", "waiting for your OK")
    vm.busy -> Status("working", "working")
    else -> Status("idle", "idle")
}

val TABS = listOf("chat", "agents", "usage")
val TITLES = mapOf("chat" to "seimei", "agents" to "Agents", "usage" to "Usage")

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun AppScreen(vm: Seimei) {
    BackHandler(vm.tab != "chat") { vm.show("chat") }
    // Notifications need a yes on Android 13 and later; asked once, after the first login.
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    val ctx = LocalContext.current
    LaunchedEffect(Unit) {
        if (Delivery.available && !vm.prefs.askedNotifications &&
            ctx.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            vm.prefs.askedNotifications = true
            ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    val snacks = remember { SnackbarHostState() }
    val snackScope = rememberCoroutineScope()
    // Shown from its own scope: clearing vm.snack restarts this effect, which would cancel a snackbar shown inside it.
    LaunchedEffect(vm.snack) {
        val t = vm.snack ?: return@LaunchedEffect
        val undo = vm.undo
        vm.snack = null
        vm.undo = null
        snackScope.launch {
            snacks.currentSnackbarData?.dismiss()
            // With an action, a snackbar stays up until tapped unless given a duration.
            val shown = snacks.showSnackbar(t, actionLabel = if (undo != null) "Undo" else null, duration = SnackbarDuration.Long)
            if (shown == SnackbarResult.ActionPerformed) undo?.invoke()
        }
    }
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().imePadding()) {
        Header(vm)
        Box(Modifier.weight(1f)) {
            when (vm.tab) {
                "chat" -> if (vm.lane == "k") TasksTab(vm) else ChatTab(vm)
                "agents" -> AgentsTab(vm)
                "usage" -> UsageTab(vm)
            }
        }
        if (vm.tab == "chat" && vm.lane != "k") Composer(vm)
        // The keyboard covers the bottom of the screen: while typing, the navigation bar steps aside.
        if (!WindowInsets.isImeVisible) NavBar(vm)
    }
    // Above the composer (chat) and the navigation bar (hidden while typing).
    val above = (if (vm.tab == "chat" && vm.lane != "k") 76 else 0) + (if (WindowInsets.isImeVisible) 0 else 80) + 8
    SnackbarHost(snacks, Modifier.align(Alignment.BottomCenter).imePadding().padding(bottom = above.dp))
    }
    vm.page?.let { AgentPage(vm, it) }
    if (vm.about) AboutSheet(vm) { vm.about = false }
}

@Composable
fun Header(vm: Seimei) {
    val st = status(vm)
    val (working, asks) = agentCounts(vm)
    val (dot, text) = when (vm.tab) {
        "chat" -> st.s to st.text
        "agents" -> (if (asks > 0) "waiting" else if (working > 0) "working" else "idle") to
            (listOfNotNull(if (working > 0) "$working working" else null, if (asks > 0) "$asks ${if (asks == 1) "needs" else "need"} you" else null)
                .joinToString(" · ").ifEmpty { "nothing running" })
        else -> "idle" to ""
    }.let { (d, t) ->
        // Away: the other tabs show the last known state, and say so.
        if (vm.tab != "chat" && !vm.online && vm.awaySince != null) "away" to "offline since ${clock(vm.awaySince!! / 1000.0)} · last known state"
        else d to t
    }
    // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
    val showStatus = text.isNotEmpty() && !(vm.tab == "chat" && (dot == "idle" || dot == "working"))
    var menu by remember { mutableStateOf(false) }
    var themes by remember { mutableStateOf(false) }
    if (themes) ThemeSheet(vm) { themes = false }
    var notifications by remember { mutableStateOf(false) }
    if (notifications) NotificationsSheet(vm) { notifications = false }
    Row(Modifier.statusBarsPadding().fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(40.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer), contentAlignment = Alignment.Center) {
            Glyph(24.dp)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(TITLES[vm.tab]!!, fontSize = 20.sp)
            if (showStatus) Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(dotColor(dot)))
                Spacer(Modifier.width(6.dp))
                Text(text, fontSize = 13.sp, color = LocalExtra.current.text3)
            }
        }
        Box {
            IconButton({ menu = true }) {
                Box {
                    Icon(Icons.more, if (vm.release != null) "Menu, update available" else "Menu")
                    if (vm.release != null) Box(Modifier.align(Alignment.TopEnd).size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
                }
            }
            DropdownMenu(menu, { menu = false }) {
                val ctx = LocalContext.current
                val r = vm.release
                DropdownMenuItem(
                    text = { Text(if (r != null) "Update to ${r.version}" else "About seimei",
                        color = if (r != null) MaterialTheme.colorScheme.primary else Color.Unspecified) },
                    trailingIcon = if (r != null) ({ Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary)) }) else null,
                    onClick = {
                        menu = false
                        vm.about = true
                        vm.checkUpdate()
                    },
                )
                if (Delivery.available) DropdownMenuItem(
                    text = { Text("Notifications") },
                    onClick = {
                        menu = false
                        vm.pushTest = null
                        notifications = true
                    },
                )
                DropdownMenuItem(
                    text = { Text("Theme") },
                    onClick = {
                        menu = false
                        themes = true
                    },
                )
            }
        }
    }
}

val ACTIVE = setOf("running", "starting", "asking")

@Composable
fun dotColor(s: String): Color = when (s) {
    "away" -> MaterialTheme.colorScheme.error
    "waiting" -> LocalExtra.current.warn
    "working", "starting", "connecting" -> MaterialTheme.colorScheme.primary
    else -> LocalExtra.current.ok
}

@Composable
fun NavBar(vm: Seimei) {
    val (working, asks) = agentCounts(vm)
    NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        for (tab in TABS) {
            NavigationBarItem(
                selected = vm.tab == tab,
                onClick = { vm.show(tab) },
                label = { Text(TITLES[tab]!!.replaceFirstChar { it.uppercase() }.let { if (tab == "chat") "Chat" else it }) },
                icon = {
                    val ic = mapOf("chat" to Icons.chat, "agents" to Icons.agents, "usage" to Icons.usage)[tab]!!
                    BadgedBox(badge = {
                        when {
                            // What needs the user counts first, in orange; otherwise how many work.
                            tab == "agents" && (asks > 0 || working > 0) -> {
                                val n = if (asks > 0) asks else working
                                Badge(containerColor = if (asks > 0) LocalExtra.current.warn else MaterialTheme.colorScheme.primary,
                                    contentColor = MaterialTheme.colorScheme.onPrimary) { Text(if (n > 99) "99+" else "$n") }
                            }
                            tab == "chat" && vm.tab != "chat" && (vm.unread || vm.waiting > 0) -> Badge(containerColor = MaterialTheme.colorScheme.primary)
                        }
                    }) { if (tab == "usage") UsageRing(vm) else Icon(ic, null) }
                },
            )
        }
    }
}

package com.hisamiowo.seimei

import com.hisamiowo.seimei.shared.R
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.animation.core.Animatable
import androidx.activity.BackEventCompat
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.draw.alpha
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FastOutLinearInEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.togetherWith
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.fadeOut
import androidx.compose.animation.fadeIn
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.layout.onSizeChanged
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
import androidx.activity.compose.LocalActivity
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
import androidx.compose.material3.LocalContentColor
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
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.selectable
import androidx.compose.ui.semantics.Role
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.indication
import androidx.compose.material3.ripple
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.material3.ButtonDefaults
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.unit.DpOffset
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.ui.text.rememberTextMeasurer

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
// The web's letter spacing: none (M3's 0.1 to 0.5 sp widened every line and broke the design's wraps)
val SeimeiType = Typography(
    displayLarge = Base.displayLarge.copy(fontFamily = Sans, letterSpacing = 0.sp), displayMedium = Base.displayMedium.copy(fontFamily = Sans, letterSpacing = 0.sp),
    displaySmall = Base.displaySmall.copy(fontFamily = Sans, letterSpacing = 0.sp), headlineLarge = Base.headlineLarge.copy(fontFamily = Sans, letterSpacing = 0.sp),
    headlineMedium = Base.headlineMedium.copy(fontFamily = Sans, letterSpacing = 0.sp), headlineSmall = Base.headlineSmall.copy(fontFamily = Sans, letterSpacing = 0.sp),
    titleLarge = Base.titleLarge.copy(fontFamily = Sans, letterSpacing = 0.sp), titleMedium = Base.titleMedium.copy(fontFamily = Sans, letterSpacing = 0.sp),
    titleSmall = Base.titleSmall.copy(fontFamily = Sans, letterSpacing = 0.sp), bodyLarge = Base.bodyLarge.copy(fontFamily = Sans, letterSpacing = 0.sp),
    bodyMedium = Base.bodyMedium.copy(fontFamily = Sans, letterSpacing = 0.sp), bodySmall = Base.bodySmall.copy(fontFamily = Sans, letterSpacing = 0.sp),
    labelLarge = Base.labelLarge.copy(fontFamily = Sans, letterSpacing = 0.sp), labelMedium = Base.labelMedium.copy(fontFamily = Sans, letterSpacing = 0.sp),
    labelSmall = Base.labelSmall.copy(fontFamily = Sans, letterSpacing = 0.sp),
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
        // A side thread's ping opens its page.
        intent?.getStringExtra("thread")?.takeIf { it.isNotEmpty() }?.let { vm.openThread(it) }
        intent?.removeExtra("thread")
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
    // The drawable is the adaptive icon's 108-unit layer and the glyph spans its middle 60, so drawn 1.8x the glyph fills size.
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

/** A button's label that stays while it works: the indicator sits over the hidden label, so the button keeps its size. */
@Composable
fun BusyLabel(text: String, busy: Boolean, fontSize: TextUnit = TextUnit.Unspecified) {
    Box(contentAlignment = Alignment.Center) {
        Text(text, Modifier.alpha(if (busy) 0f else 1f), fontSize = fontSize)
        if (busy) Loader(24.dp, contained = false, color = LocalContentColor.current)
    }
}

/** Step 1 when no server is built in: the address, checked before going on. */
@Composable
fun ServerScreen(vm: Seimei) {
    var typed by remember { mutableStateOf(vm.lastServer.removePrefix("https://")) }
    FirstRun("Server address", "The seimei page shows it under Phone app") {
        OutlinedTextField(
            typed,
            { typed = it.trim(); vm.serverError = null },
            label = { Text("Address") },
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
        ) { BusyLabel("Continue", vm.authBusy, fontSize = 16.sp) }
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
    val line = if (withPassword) "The password isn't stored"
        else "On the seimei page: Settings, Phone app, Pair a phone"
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
        ) { BusyLabel("Pair", vm.authBusy, fontSize = 16.sp) }
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

/**
 * The lock screen, bottom-aligned: the seiman and "seimei", Use fingerprint, "or", then the password and Log in.
 * The password logs in as the web page does; the phone's key stays for the next time.
 */
@Composable
fun LockScreen(vm: Seimei, activity: Activity) {
    LaunchedEffect(Unit) { vm.unlock(activity) }
    // Not rememberSaveable: the password never goes into saved state.
    var password by remember { mutableStateOf("") }
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    val login = { vm.passwordLogin(password) }
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(start = 26.dp, end = 26.dp, bottom = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp, Alignment.Bottom),
    ) {
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            Icon(painterResource(R.drawable.ic_seimei_monochrome), null, Modifier.requiredSize(72.dp), tint = cs.primary)
        }
        Text("seimei", fontSize = 28.sp, lineHeight = 36.sp, fontWeight = FontWeight.Medium, letterSpacing = (-.56).sp)
        Button({ vm.unlock(activity) }, enabled = !vm.authBusy, modifier = Modifier.fillMaxWidth().height(56.dp), shape = CircleShape) {
            Icon(Icons.fingerprint, null, Modifier.size(20.dp))
            Spacer(Modifier.width(8.dp))
            Text("Use fingerprint", fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
        }
        Text("or", fontSize = 12.sp, lineHeight = 16.sp, color = x.text3)
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            BasicTextField(
                password, { password = it; vm.authError = null },
                singleLine = true,
                textStyle = TextStyle(fontSize = 16.sp, color = x.text),
                cursorBrush = SolidColor(cs.primary),
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false, imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { login() }),
                modifier = Modifier.fillMaxWidth().height(56.dp).semantics { contentDescription = "Password" },
                decorationBox = { field ->
                    Box(Modifier.clip(RoundedCornerShape(18.dp)).background(cs.surfaceContainerHigh).padding(horizontal = 20.dp),
                        contentAlignment = Alignment.CenterStart) {
                        if (password.isEmpty()) Text("Password", fontSize = 16.sp, color = x.text3)
                        field()
                    }
                },
            )
            Button(login, enabled = !vm.authBusy, modifier = Modifier.fillMaxWidth().height(56.dp), shape = CircleShape,
                colors = ButtonDefaults.buttonColors(containerColor = x.tonal, contentColor = x.onTonal)) {
                Text("Log in", fontSize = 16.sp, fontWeight = FontWeight.Medium)
            }
        }
        vm.authError?.let { Text(it, color = cs.error, textAlign = TextAlign.Center, fontSize = 14.sp, lineHeight = 21.7.sp) }
    }
}

// ---------- the app: header, pages, navigation drawer ----------

data class Status(val s: String, val text: String)

fun status(vm: Seimei): Status = when {
    // Offline and an approval have their own cards; the header doesn't repeat them
    !vm.online && vm.awaySince != null -> Status("away", "")
    !vm.online -> Status("connecting", "Connecting…")
    !vm.connected -> Status("starting", "Session starting…")
    vm.waiting > 0 -> Status("waiting", "")
    vm.busy -> Status("working", "Working")
    else -> Status("idle", "")
}

val TABS = listOf("chat", "threads", "agents", "watches", "files", "usage", "settings")
val TITLES = mapOf("chat" to "seimei", "threads" to "Side threads", "agents" to "Agents", "watches" to "Watches", "files" to "Files",
    "usage" to "Usage", "settings" to "Settings")

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun AppScreen(vm: Seimei) {
    val drawer = rememberSideDrawer()
    val activity = LocalActivity.current
    // Android's back gesture owns the screen's edges: on the top screens one from the left edge opens the drawer, which follows it.
    // The right edge and the back button keep their meaning: to the chat, then out of the app.
    PredictiveBackHandler(vm.page == null && !drawer.isOpen) { events ->
        var left = false
        try {
            events.collect { e ->
                left = e.swipeEdge == BackEventCompat.EDGE_LEFT
                if (left) drawer.follow(e.touchX)
            }
            when {
                left -> drawer.open()
                vm.tab != "chat" -> vm.show("chat")
                else -> activity?.moveTaskToBack(true)
            }
        } catch (e: kotlin.coroutines.cancellation.CancellationException) {
            if (left) drawer.close()
            throw e
        }
    }
    BackHandler(drawer.isOpen) { drawer.close() }
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
    // What a voice call has to say (it could not start, the partner's error) shows as a snack.
    LaunchedEffect(CallService.told) {
        CallService.told?.let { vm.tell(it, null) }
        CallService.told = null
    }
    LaunchedEffect(vm.snack) {
        val t = vm.snack ?: return@LaunchedEffect
        val undo = vm.undo
        val label = vm.snackAction
        vm.snack = null
        vm.undo = null
        vm.snackAction = "Undo"
        snackScope.launch {
            snacks.currentSnackbarData?.dismiss()
            // With an action it stays until it's used, replaced, or the user moves on (M3); a plain note goes by itself
            val shown = snacks.showSnackbar(t, actionLabel = if (undo != null) label else null,
                duration = if (undo != null) SnackbarDuration.Indefinite else SnackbarDuration.Short)
            if (shown == SnackbarResult.ActionPerformed) undo?.invoke()
        }
    }
    LaunchedEffect(vm.tab, vm.page == null) { snacks.currentSnackbarData?.dismiss() }
    val still = reducedMotion()
    // A pick in the drawer closes it, and the page it was opened over
    val pick: (() -> Unit) -> Unit = { run ->
        drawer.close()
        if (vm.page != null) vm.closePage()
        run()
    }
    LaunchedEffect(drawer.isOpen) { if (drawer.isOpen) vm.loadWatches() }
    SwipeDrawer(drawer, { DrawerSheet(vm, pick) }) {
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().imePadding()) {
        // Under a page the header can still take a key press: the drawer opens only on the pages themselves
        Header(vm) { if (vm.page == null) drawer.open() }
        // The page sheet: surface-low under the header, 28 corners on top
        Box(Modifier.weight(1f).clip(RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)).background(MaterialTheme.colorScheme.surfaceContainerLow)) {
            // Top level between nav destinations: the old one fades out, then the new one fades in
            AnimatedContent(vm.tab, transitionSpec = {
                if (still) EnterTransition.None togetherWith ExitTransition.None
                else fadeIn(tween(200, delayMillis = 100, easing = LinearOutSlowInEasing)) togetherWith fadeOut(tween(100, easing = FastOutLinearInEasing))
            }, label = "tab") { tab ->
                when (tab) {
                    "chat" -> Lanes(vm, still)
                    "threads" -> ThreadsTab(vm)
                    "agents" -> AgentsTab(vm)
                    "watches" -> WatchesTab(vm)
                    "files" -> FilesTab(vm)
                    "usage" -> UsageTab(vm)
                    "settings" -> SettingsTab(vm)
                }
            }
        }
        // The bottom container: the chat's composer on surface-container, 24 corners on top (the sheet shows in its corners)
        // A voice call's bar takes the composer's place, and stays on every tab while the call runs.
        val call = CallService.call
        val composing = vm.tab == "chat" && vm.lane != "k" || call != null
        Column(Modifier.onSizeChanged { vm.barPx = it.height }.background(MaterialTheme.colorScheme.surfaceContainerLow)
            .then(if (composing) Modifier.clip(RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp)).background(MaterialTheme.colorScheme.surfaceContainer) else Modifier)) {
            if (call != null) CallBar(vm, call)
            else if (composing) Composer(vm)
            if (!WindowInsets.isImeVisible) Spacer(Modifier.navigationBarsPadding())
        }
    }
    // Forward and backward: a page comes in from the right and goes back out to it.
    // Predictive back has already taken the page away when it commits, so that exit only finishes it.
    AnimatedContent(vm.page, transitionSpec = {
        if (still) fadeIn(tween(150)) togetherWith fadeOut(tween(150))
        else if (targetState != null) (slideInHorizontally(tween(300, easing = EMPHASIZED_DECEL)) { it / 8 } + fadeIn(tween(300))) togetherWith
            fadeOut(tween(90))
        else EnterTransition.None togetherWith (slideOutHorizontally(tween(200, easing = EMPHASIZED_ACCEL)) { it / 8 } + fadeOut(tween(200)))
    }, contentKey = { it?.let { p -> p.kind + p.id } }, label = "page") { p -> if (p != null) AgentPage(vm, p) }
    // The one polite live region TalkBack reads: a card that arrives, a reply, a card that settles
    Text(vm.said, Modifier.size(1.dp).alpha(0f).semantics { liveRegion = LiveRegionMode.Polite })
    // Above whatever holds the bottom edge, measured: never over the field (44 over a composer, 32 over the bar)
    val composer = vm.page != null || (vm.tab == "chat" && vm.lane != "k") || CallService.call != null
    val above = with(LocalDensity.current) { (if (vm.page != null) vm.pageBarPx else vm.barPx).toDp() } + if (composer) 44.dp else 32.dp
    SnackbarHost(snacks, Modifier.align(Alignment.BottomCenter).then(if (vm.page != null) Modifier.navigationBarsPadding() else Modifier)
        .imePadding().padding(bottom = above)) { d ->
        // The web's snackbar: as wide as its text, 344 at the least, 8 corners, the action at 14 semibold
        val cs = MaterialTheme.colorScheme
        val measure = rememberTextMeasurer()
        val density = LocalDensity.current
        val label = d.visuals.actionLabel
        val wide = with(density) {
            val text = measure.measure(d.visuals.message, TextStyle(fontSize = 14.sp)).size.width.toDp()
            val action = label?.let { measure.measure(it, TextStyle(fontSize = 14.sp, fontWeight = FontWeight.SemiBold)).size.width.toDp() + 40.dp } ?: 0.dp
            text + action + 32.dp
        }
        Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
            // A plain row: M3's Snackbar drew a fixed-width action over the text
            Surface(Modifier.width(wide.coerceIn(344.dp, 560.dp)), shape = RoundedCornerShape(8.dp), color = cs.inverseSurface,
                contentColor = cs.inverseOnSurface, shadowElevation = 6.dp) {
                Row(Modifier.heightIn(min = 48.dp).padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(d.visuals.message, Modifier.weight(1f).padding(vertical = 14.dp), fontSize = 14.sp, lineHeight = 20.sp)
                    if (label != null) TextButton(d::performAction, Modifier.heightIn(min = 48.dp),
                        colors = ButtonDefaults.textButtonColors(contentColor = cs.inversePrimary)) {
                        Text(label, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
        }
    }
    }
    }
    if (vm.about) AboutSheet(vm) { vm.about = false }
}

/** Chat, Trajectory and Tasks: Lateral, the incoming lane slides in from the side of its tab (emphasized decelerate). */
@Composable
private fun Lanes(vm: Seimei, still: Boolean) {
    val order = "ctk"
    Column(Modifier.fillMaxSize()) {
    LaneTabs(vm)
    AnimatedContent(vm.lane, transitionSpec = {
        val dir = if (order.indexOf(targetState) > order.indexOf(initialState)) 1 else -1
        if (still) fadeIn(tween(150)) togetherWith fadeOut(tween(150))
        else slideInHorizontally(tween(300, easing = EMPHASIZED_DECEL)) { it * dir } togetherWith
            slideOutHorizontally(tween(300, easing = EMPHASIZED_DECEL)) { -it * dir }
    }, label = "lane") { l -> if (l == "k") TasksTab(vm) else ChatTab(vm, l) }
    }
}

/** Predictive back for a screen: during the gesture it shrinks toward 90% and leans to the swipe edge (8dp margin, rounded corners).
 * On commit it leaves on emphasized accelerate and onBack runs; on cancel it comes back. */
@Composable
fun predictiveBack(enabled: Boolean = true, onBack: () -> Unit): Modifier {
    val back = remember { Animatable(0f) }
    var edge by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    PredictiveBackHandler(enabled) { events ->
        try {
            events.collect { e ->
                edge = e.swipeEdge
                back.snapTo(e.progress)
            }
            back.animateTo(1f, tween(150, easing = EMPHASIZED_ACCEL))
            onBack()
        } catch (e: kotlin.coroutines.cancellation.CancellationException) {
            scope.launch { back.animateTo(0f, tween(200, easing = EMPHASIZED_DECEL)) }
            throw e
        }
    }
    return Modifier.graphicsLayer {
        val p = back.value
        val s = 1f - .1f * p
        scaleX = s
        scaleY = s
        translationX = (if (edge == BackEventCompat.EDGE_LEFT) 1 else -1) * 8.dp.toPx() * p
        shape = RoundedCornerShape(28.dp * p)
        clip = p > 0f
        alpha = if (p >= 1f) 0f else 1f
    }
}

val EMPHASIZED_DECEL = CubicBezierEasing(.05f, .7f, .1f, 1f)
val EMPHASIZED_ACCEL = CubicBezierEasing(.3f, 0f, .8f, .15f)

/** The top bar's fill: the background at rest, surface-container once the screen has moved off it. */
@Composable
fun barColor(vm: Seimei): Color {
    val cs = MaterialTheme.colorScheme
    return animateColorAsState(if (vm.lifted && vm.page == null) cs.surfaceContainer else cs.surface, tween(225), label = "bar").value
}

/** The header's note on what the dispatcher runs on: an ACP agent's or Codex's name; the default harness goes without saying. */
fun harnessLabel(harness: String): String? = if (harness.startsWith("acp:")) "on ${harness.removePrefix("acp:")}" else null

@Composable
fun Header(vm: Seimei, openDrawer: () -> Unit) {
    val st = status(vm)
    val (dot, text) = (if (vm.tab == "chat") st.s to st.text else "idle" to "").let { (d, t) ->
        // Away: the other pages show the last known state, and say so.
        if (vm.tab != "chat" && !vm.online && vm.awaySince != null) "away" to "Offline since ${clock(vm.awaySince!! / 1000.0)}"
        else d to t
    }
    // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
    val showStatus = text.isNotEmpty() && !(vm.tab == "chat" && (dot == "idle" || dot == "working"))
    val cs = MaterialTheme.colorScheme
    Row(Modifier.background(barColor(vm)).statusBarsPadding().fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically) {
        val news = drawerNews(vm) || vm.release != null
        IconButton(openDrawer, Modifier.size(48.dp)) {
            Box {
                Icon(Icons.menu, if (news) "Open the menu, something new" else "Open the menu")
                if (news) Box(Modifier.align(Alignment.TopEnd).offset(x = 2.dp, y = (-2).dp).size(8.dp).clip(CircleShape).background(cs.primary))
            }
        }
        Spacer(Modifier.width(4.dp))
        Column(Modifier.weight(1f)) {
            Row {
                Text(TITLES[vm.tab]!!, fontSize = 20.sp, modifier = Modifier.alignByBaseline())
                harnessLabel(vm.harness)?.takeIf { vm.tab == "chat" }?.let {
                    Spacer(Modifier.width(8.dp))
                    Text(it, fontSize = 12.sp, color = LocalExtra.current.text3, modifier = Modifier.alignByBaseline())
                }
            }
            if (showStatus) Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(dotColor(dot)))
                Spacer(Modifier.width(6.dp))
                Text(text, fontSize = 13.sp, color = LocalExtra.current.text3)
            }
        }
        if (vm.tab == "chat" || vm.tab == "threads") IconButton({ vm.openDraft() }, Modifier.size(48.dp)) {
            Icon(Icons.compose, "New side thread", Modifier.size(22.dp))
        }
    }
}

@Composable
fun dotColor(s: String): Color = when (s) {
    "away" -> MaterialTheme.colorScheme.error
    "waiting" -> LocalExtra.current.warn
    "working", "starting", "connecting" -> MaterialTheme.colorScheme.primary
    else -> LocalExtra.current.ok
}

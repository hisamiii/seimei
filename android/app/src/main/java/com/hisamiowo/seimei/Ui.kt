package com.hisamiowo.seimei

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
// The web's letter spacing: none (M3's 0.1 to 0.5 sp widened every line and broke the wraps of final/)
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
    // The grid of final/ (the showcase's Agents tab): four rounded tiles
    val agents = icon("M6 4h3.5A1.5 1.5 0 0 1 11 5.5V9a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 9V5.5A1.5 1.5 0 0 1 6 4zM14.5 4H18a1.5 1.5 0 0 1 " +
        "1.5 1.5V9A1.5 1.5 0 0 1 18 10.5h-3.5A1.5 1.5 0 0 1 13 9V5.5A1.5 1.5 0 0 1 14.5 4zM6 13.5h3.5A1.5 1.5 0 0 1 11 15v3.5A1.5 1.5 0 0 1 9.5 " +
        "20H6a1.5 1.5 0 0 1-1.5-1.5V15A1.5 1.5 0 0 1 6 13.5zM14.5 13.5H18a1.5 1.5 0 0 1 1.5 1.5v3.5A1.5 1.5 0 0 1 18 20h-3.5a1.5 1.5 0 0 " +
        "1-1.5-1.5V15a1.5 1.5 0 0 1 1.5-1.5z", stroke = 1.8f)
    val usage = icon("M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 1 0 0-17z", stroke = 1.8f)
    val star = icon("M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z", stroke = 1.6f)
    val check = icon("M5 12.5l4.5 4.5L19 7.5")
    val stopped = icon("M7.5 7.5h9v9h-9z")
    val info = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.5v.01", stroke = 1.8f)
    val restarted = icon("M4 12a8 8 0 1 0 2.4-5.7M4 4.5v4h4")
    val chev = icon("M9 6l6 6-6 6")
    val expand = icon("M6 9l6 6 6-6")
    val send = icon("M12 19V5M5.5 11.5 12 5l6.5 6.5", stroke = 2.4f)
    val photo = icon("M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM9 8.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 1 0 0-3.2zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5", stroke = 1.8f)
    val more = icon("M12 5.5v.01M12 12v.01M12 18.5v.01")
    val close = icon("M6 6l12 12M18 6 6 18")
    val reply = icon("M9 14 4 9l5-5", "M4 9h10.5a5.5 5.5 0 0 1 0 11H11")
    val mic = icon("M12 3.5a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0v-5a3 3 0 0 0-3-3zM6 11.5a6 6 0 0 0 12 0M12 17.5v3", stroke = 1.8f)
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
    /** Archive: a box with a lid; Icons.archive is a package. */
    val shelf = icon("M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4", stroke = 1.8f)
    val web = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18")
    val tool = icon("M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z")
    val laptop = icon("M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18")
    val list = icon("M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01")
    val alert = icon("M12 7v6M12 17v.01")
    val clock = icon("M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16zM12 8v4l2.5 2")
    val dash = icon("M7 12h10")
    val back = icon("M15 5l-7 7 7 7")
    val down = icon("M12 5v14M5.5 12.5 12 19l6.5-6.5", stroke = 2.2f)
    /** A job to look over: a clipboard with a check. */
    val review = icon("M9 4.5H6.5A1.5 1.5 0 0 0 5 6v13.5A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V6a1.5 1.5 0 0 0-1.5-1.5H15M9.8 3h4.4a.8.8 0 0 1 .8.8v1.4a.8.8 0 0 1-.8.8H9.8a.8.8 0 0 1-.8-.8V3.8a.8.8 0 0 1 .8-.8zM8.5 13.5l2.5 2.5 4.5-4.5")
    val bell = icon("M6 10a6 6 0 0 1 12 0c0 4.5 1.5 6.5 2 7H4c.5-.5 2-2.5 2-7zM10 20.5a2.1 2.1 0 0 0 4 0", stroke = 1.8f)
    val palette = icon("M12 21a9 9 0 1 1 0-18c4.97 0 9 3.58 9 8 0 2.76-2.24 4-4.5 4H15a2 2 0 0 0-1.5 3.3A1.6 1.6 0 0 1 12 21z",
        "M7.5 11.5v.01M10 7.5v.01M14.5 7.5v.01M17 11v.01", stroke = 1.8f)
    val shield = icon("M12 3 4.5 6v5.5c0 4.4 3.1 8.2 7.5 9.5 4.4-1.3 7.5-5.1 7.5-9.5V6L12 3z", "M9 12h6M9 15.5h4", stroke = 1.8f)
    val logout = icon("M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3M10 17l-5-5 5-5M5 12h11", stroke = 1.8f)
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
        else "On the seimei page: menu, Phone app, Pair a phone"
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
 * The lock screen of final/ 01, bottom-aligned: the seiman and "seimei", Use fingerprint, "or", then the password and
 * Log in (as the web page logs in; the phone's key stays for the next time).
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

// ---------- the app: header, tabs, navigation bar ----------

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
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().imePadding()) {
        Header(vm)
        // The page sheet of final/: surface-low under the header, 28 corners on top
        Box(Modifier.weight(1f).clip(RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)).background(MaterialTheme.colorScheme.surfaceContainerLow)) {
            // Top level between nav destinations: the old one fades out, then the new one fades in (BEHAVIOR.md 1)
            AnimatedContent(vm.tab, transitionSpec = {
                if (still) EnterTransition.None togetherWith ExitTransition.None
                else fadeIn(tween(200, delayMillis = 100, easing = LinearOutSlowInEasing)) togetherWith fadeOut(tween(100, easing = FastOutLinearInEasing))
            }, label = "tab") { tab ->
                when (tab) {
                    "chat" -> Lanes(vm, still)
                    "agents" -> AgentsTab(vm)
                    "usage" -> UsageTab(vm)
                }
            }
        }
        // The bottom container of final/: composer and navigation bar on surface-container, 24 corners on top
        // (the sheet shows in its corners)
        Column(Modifier.onSizeChanged { vm.barPx = it.height }.background(MaterialTheme.colorScheme.surfaceContainerLow)
            .clip(RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
            .background(MaterialTheme.colorScheme.surfaceContainer)) {
            if (vm.tab == "chat" && vm.lane != "k") Composer(vm)
            // The keyboard covers the bottom of the screen: while typing, the navigation bar steps aside.
            if (!WindowInsets.isImeVisible) NavBar(vm)
        }
    }
    // Forward and backward (BEHAVIOR.md 1): a page comes in from the right and goes back out to it. Predictive back
    // has already taken the page away when it commits, so that exit only finishes it.
    AnimatedContent(vm.page, transitionSpec = {
        if (still) fadeIn(tween(150)) togetherWith fadeOut(tween(150))
        else if (targetState != null) (slideInHorizontally(tween(300, easing = EMPHASIZED_DECEL)) { it / 8 } + fadeIn(tween(300))) togetherWith
            fadeOut(tween(90))
        else EnterTransition.None togetherWith (slideOutHorizontally(tween(200, easing = EMPHASIZED_ACCEL)) { it / 8 } + fadeOut(tween(200)))
    }, contentKey = { it?.let { p -> p.kind + p.id } }, label = "page") { p -> if (p != null) AgentPage(vm, p) }
    // The one polite live region TalkBack reads: a card that arrives, a reply, a card that settles (BEHAVIOR.md 6)
    Text(vm.said, Modifier.size(1.dp).alpha(0f).semantics { liveRegion = LiveRegionMode.Polite })
    // Above whatever holds the bottom edge, measured: never over the field (final/ 19 and 20: 44 over a composer, 32 over the bar)
    val composer = vm.page != null || (vm.tab == "chat" && vm.lane != "k")
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

/** Predictive back (BEHAVIOR.md 2) for a screen: while the gesture runs it shrinks toward 90% and leans to the swipe
 *  edge with the 8dp margin and rounded corners; on commit it leaves on emphasized accelerate, on cancel it comes back.
 *  The returned modifier draws that; onBack runs once the gesture commits. */
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

@Composable
fun Header(vm: Seimei) {
    val st = status(vm)
    val (working, asks) = agentCounts(vm)
    val (dot, text) = when (vm.tab) {
        "chat" -> st.s to st.text
        // The sections and the nav badge say how many work and need you
        "agents" -> "idle" to ""
        else -> "idle" to ""
    }.let { (d, t) ->
        // Away: the other tabs show the last known state, and say so.
        if (vm.tab != "chat" && !vm.online && vm.awaySince != null) "away" to "Offline since ${clock(vm.awaySince!! / 1000.0)}"
        else d to t
    }
    // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
    val showStatus = text.isNotEmpty() && !(vm.tab == "chat" && (dot == "idle" || dot == "working"))
    var menu by remember { mutableStateOf(false) }
    var themes by remember { mutableStateOf(false) }
    if (themes) ThemeSheet(vm) { themes = false }
    var audit by remember { mutableStateOf(false) }
    if (audit) ActivitySheet(vm) { audit = false }
    var notifications by remember { mutableStateOf(false) }
    if (notifications) NotificationsSheet(vm) { notifications = false }
    Row(Modifier.background(barColor(vm)).statusBarsPadding().fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
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
            // final/ 13: 220 wide, 16 corners on container-high, 48 rows with a 20 icon in text-3, 14sp
            val x = LocalTokens.current
            @Composable
            fun Item(text: String, icon: ImageVector, color: Color = x.text, dot: Boolean = false, onClick: () -> Unit) = DropdownMenuItem(
                text = { Text(text, fontSize = 14.sp, lineHeight = 20.sp, color = color) },
                leadingIcon = { Icon(icon, null, Modifier.size(20.dp), tint = if (color == x.text) x.text3 else color) },
                trailingIcon = if (dot) ({ Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary)) }) else null,
                onClick = { menu = false; onClick() },
                modifier = Modifier.height(48.dp),
                contentPadding = PaddingValues(horizontal = 16.dp),
            )
            DropdownMenu(menu, { menu = false }, Modifier.width(220.dp), offset = DpOffset((-8).dp, 0.dp),
                shape = RoundedCornerShape(16.dp), containerColor = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 6.dp) {
                val r = vm.release
                Item(if (r != null) "Update to ${r.version}" else "About seimei", Icons.info,
                    if (r != null) MaterialTheme.colorScheme.primary else x.text, dot = r != null) {
                    vm.about = true
                    vm.checkUpdate()
                }
                if (Delivery.available) Item("Notifications", Icons.bell) {
                    vm.pushTest = null
                    notifications = true
                }
                Item("Theme", Icons.palette) { themes = true }
                Item("Activity log", Icons.shield) { audit = true }
                Item("Log out", Icons.logout) { vm.logOut() }
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

/**
 * The navigation bar of final/ (06, 08): 64 tall on the bottom container, a 56 x 32 primary pill under the selected
 * icon (on-primary), the label in text, the others in text-3; what needs the user is a warn badge on the pill's corner.
 */
@Composable
fun NavBar(vm: Seimei) {
    val (working, asks) = agentCounts(vm)
    val cs = MaterialTheme.colorScheme
    val x = LocalTokens.current
    Row(Modifier.fillMaxWidth().navigationBarsPadding().height(64.dp).padding(horizontal = 8.dp).selectableGroup()) {
        for (tab in TABS) {
            val on = vm.tab == tab
            val press = remember { MutableInteractionSource() }
            val pressed by press.collectIsPressedAsState()
            // Under the finger the pill widens inside its item (final/ 8)
            val pill by animateDpAsState(if (pressed) 64.dp else 56.dp, spring(dampingRatio = .9f, stiffness = 1400f), label = "pill")
            val label = if (tab == "chat") "seimei" else TITLES[tab]!!
            Column(
                Modifier.weight(1f).fillMaxHeight()
                    .selectable(on, interactionSource = press, indication = null, role = Role.Tab) { vm.show(tab) }
                    .padding(vertical = 6.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Box(Modifier.size(pill, 32.dp).clip(RoundedCornerShape(16.dp)).background(if (on) cs.primary else Color.Transparent)
                    .indication(press, ripple()), contentAlignment = Alignment.Center) {
                    val tint = if (on) cs.onPrimary else x.text3
                    if (tab == "usage") UsageRing(vm, on) else Icon(if (tab == "chat") Icons.chat else Icons.agents, null, Modifier.size(24.dp), tint = tint)
                    val badge = when {
                        // What needs the user counts first, in orange; otherwise how many work.
                        tab == "agents" && asks > 0 -> "$asks" to x.warn
                        tab == "agents" && working > 0 -> "$working" to cs.primary
                        tab == "chat" && vm.tab != "chat" && (vm.unread || vm.waiting > 0) -> "" to cs.primary
                        else -> null
                    }
                    badge?.let { (n, c) ->
                        val big = n.isNotEmpty()
                        Box(Modifier.align(Alignment.TopStart).offset(x = (pill - 56.dp) / 2 + 34.dp, y = if (big) (-1).dp else 4.dp)
                            .size(if (big) 16.dp else 8.dp).clip(CircleShape).background(c), contentAlignment = Alignment.Center) {
                            if (big) Text(if (n.length > 2) "99+" else n, color = if (c == x.warn) x.onWarn else cs.onPrimary,
                                fontSize = 11.sp, lineHeight = 16.sp, fontWeight = FontWeight.SemiBold)
                        }
                    }
                }
                Text(label, color = if (on) x.text else x.text3, fontSize = 12.sp, lineHeight = 16.sp,
                    fontWeight = if (on) FontWeight.Bold else FontWeight.Medium,
                    letterSpacing = .24.sp, maxLines = 1)
            }
        }
    }
}

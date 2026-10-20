package org.example.seimei

import android.app.Activity
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner

// ---------- theme: Catppuccin Mocha arranged as Material 3 tonal roles, as on the web page ----------

object Mocha {
    val surface = Color(0xFF1E1E2E)
    val surfaceLow = Color(0xFF181825)
    val surfaceLowest = Color(0xFF11111B)
    val container = Color(0xFF26263A)
    val containerHigh = Color(0xFF313244)
    val containerHighest = Color(0xFF3A3B50)
    val primary = Color(0xFFCBA6F7)
    val onPrimary = Color(0xFF2D1B47)
    val primaryContainer = Color(0xFF3D3552)
    val onPrimaryContainer = Color(0xFFECDCFF)
    val tonal = Color(0xFF35384D)
    val onTonal = Color(0xFFDFE3FF)
    val ok = Color(0xFFA6E3A1)
    val warn = Color(0xFFFAB387)
    val error = Color(0xFFF38BA8)
    val text = Color(0xFFCDD6F4)
    val text2 = Color(0xFFBAC2DE)
    val text3 = Color(0xFFA6ADC8)
    val outline = Color(0xFF6C7086)
}

val MochaScheme = darkColorScheme(
    primary = Mocha.primary, onPrimary = Mocha.onPrimary,
    primaryContainer = Mocha.primaryContainer, onPrimaryContainer = Mocha.onPrimaryContainer,
    secondary = Mocha.primary, onSecondary = Mocha.onPrimary,
    secondaryContainer = Mocha.tonal, onSecondaryContainer = Mocha.onTonal,
    tertiary = Mocha.warn,
    background = Mocha.surface, onBackground = Mocha.text,
    surface = Mocha.surface, onSurface = Mocha.text, onSurfaceVariant = Mocha.text3,
    surfaceVariant = Mocha.containerHigh,
    surfaceContainerLowest = Mocha.surfaceLowest, surfaceContainerLow = Mocha.surfaceLow,
    surfaceContainer = Mocha.container, surfaceContainerHigh = Mocha.containerHigh,
    surfaceContainerHighest = Mocha.containerHighest,
    outline = Mocha.outline, outlineVariant = Mocha.containerHighest,
    error = Mocha.error,
)

/** Colors the Material roles do not name: good, warning, and the dimmer text tones. */
data class Extra(val ok: Color, val warn: Color, val text2: Color, val text3: Color)

val LocalExtra = androidx.compose.runtime.staticCompositionLocalOf { Extra(Mocha.ok, Mocha.warn, Mocha.text2, Mocha.text3) }

@Composable
fun SeimeiTheme(dynamic: Boolean, content: @Composable () -> Unit) {
    val ctx = LocalContext.current
    val scheme: ColorScheme = if (dynamic && Build.VERSION.SDK_INT >= 31) dynamicDarkColorScheme(ctx) else MochaScheme
    val extra = if (scheme === MochaScheme) Extra(Mocha.ok, Mocha.warn, Mocha.text2, Mocha.text3)
    else Extra(Mocha.ok, Mocha.warn, scheme.onSurface.copy(alpha = .85f), scheme.onSurfaceVariant)
    androidx.compose.runtime.CompositionLocalProvider(LocalExtra provides extra) {
        MaterialTheme(colorScheme = scheme, content = content)
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
    val screen = icon("M5.5 4h13A2.5 2.5 0 0 1 21 6.5v7.5a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 14V6.5A2.5 2.5 0 0 1 5.5 4zM9 20.5h6M12 16.5v4", stroke = 1.8f)
    val usage = icon("M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 1 0 0-17z", stroke = 1.8f)
    val star = icon("M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z", stroke = 1.6f)
    val check = icon("M5 12.5l4.5 4.5L19 7.5")
    val chev = icon("M9 6l6 6-6 6")
    val send = icon("M12 19V5M5.5 11.5 12 5l6.5 6.5", stroke = 2.4f)
    val photo = icon("M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM9 8.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 1 0 0-3.2zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5", stroke = 1.8f)
    val more = icon("M12 5.5v.01M12 12v.01M12 18.5v.01")
    val close = icon("M6 6l12 12M18 6 6 18")
    val lock = icon("M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z")
    val fail = icon("M12 7v6M12 17v.01M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z")
    val ask = icon("M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17v.01")
    val fingerprint = icon("M7 20c.6-1.6 1-3.6 1-6.5a4 4 0 0 1 8 0c0 1.2-.1 2.4-.3 3.5M11.2 21c.5-2 .8-4.5.8-7.5M4.5 16.5c.3-1 .5-2 .5-3a7 7 0 0 1 12-4.9M19 11c.2.8.2 1.7.1 2.6M15 20.5c.3-.8.5-1.6.6-2.5", stroke = 1.8f)
    val bash = icon("M4 17l5-5-5-5M11 19h9")
    val read = icon("M6 3h9l4 4v14H6zM14 3v5h5")
    val edit = icon("M4 20h4L19 9l-4-4L4 16zM13 7l4 4")
    val search = icon("M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4")
    val web = icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18")
    val tool = icon("M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z")
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
        setContent {
            SeimeiTheme(vm.dynamicColor) {
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
                    when (vm.phase) {
                        Phase.Pair -> PairScreen(vm, this)
                        Phase.Locked -> LockScreen(vm, this)
                        Phase.Ready -> AppScreen(vm)
                    }
                }
            }
        }
    }
}

// ---------- pairing and unlocking ----------

@Composable
fun Mark(size: Int = 72) {
    Box(
        Modifier.size(size.dp).clip(RoundedCornerShape((size * 0.28f).dp)).background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Icon(Icons.star, null, Modifier.size((size * 0.55f).dp), tint = MaterialTheme.colorScheme.onPrimaryContainer)
    }
}

@Composable
fun PairScreen(vm: Seimei, activity: Activity) {
    var server by remember { mutableStateOf(vm.prefs.server) }
    var code by remember { mutableStateOf("") }
    var name by remember { mutableStateOf(Build.MODEL ?: "phone") }
    var advanced by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(56.dp))
        Mark()
        Spacer(Modifier.height(20.dp))
        Text("Pair this phone", style = MaterialTheme.typography.headlineSmall)
        Spacer(Modifier.height(8.dp))
        Text(
            "On the laptop's seimei page, open the menu, then Phone app, then Pair a phone. Type the code it shows.",
            color = LocalExtra.current.text3, textAlign = TextAlign.Center, fontSize = 14.sp,
        )
        Spacer(Modifier.height(28.dp))
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
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(name, { name = it.take(60) }, label = { Text("Name for this phone") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        if (advanced) {
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(server, { server = it.trim() }, label = { Text("Server") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        }
        vm.authError?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center, fontSize = 14.sp)
        }
        Spacer(Modifier.height(24.dp))
        Button(
            onClick = { vm.pair(activity, server, code, name) },
            enabled = !vm.authBusy && code.length == 8 && server.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(56.dp),
        ) { Text(if (vm.authBusy) "Pairing" else "Pair", fontSize = 16.sp) }
        if (!advanced) TextButton({ advanced = true }) { Text("Another server") }
        Spacer(Modifier.height(24.dp))
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

val TABS = listOf("chat", "agents", "screen", "usage")
val TITLES = mapOf("chat" to "seimei", "agents" to "Agents", "screen" to "Screen", "usage" to "Usage")

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun AppScreen(vm: Seimei) {
    BackHandler(vm.tab != "chat") { vm.tab = "chat" }
    LaunchedEffect(vm.tab) { if (vm.tab == "chat") vm.unread = false }
    Column(Modifier.fillMaxSize().imePadding()) {
        Header(vm)
        Box(Modifier.weight(1f)) {
            when (vm.tab) {
                "chat" -> ChatTab(vm)
                else -> Coming(TITLES[vm.tab]!!)
            }
        }
        if (vm.tab == "chat") Composer(vm)
        // The keyboard covers the bottom of the screen: while typing, the navigation bar steps aside.
        if (!WindowInsets.isImeVisible) NavBar(vm)
    }
}

@Composable
fun Coming(what: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text("$what comes in the next build.", color = LocalExtra.current.text3)
    }
}

@Composable
fun Header(vm: Seimei) {
    val st = status(vm)
    val working = vm.workers.count { it.str("state") in ACTIVE }
    val asks = vm.workers.count { it.str("state") == "asking" }
    val (dot, text) = when (vm.tab) {
        "chat" -> st.s to st.text
        "agents" -> (if (asks > 0) "waiting" else if (working > 0) "working" else "idle") to
            (listOfNotNull(if (working > 0) "$working working" else null, if (asks > 0) "$asks ${if (asks == 1) "needs" else "need"} you" else null)
                .joinToString(" · ").ifEmpty { "nothing running" })
        else -> "idle" to ""
    }
    // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
    val showStatus = text.isNotEmpty() && !(vm.tab == "chat" && (dot == "idle" || dot == "working"))
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.statusBarsPadding().fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(40.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer), contentAlignment = Alignment.Center) {
            Icon(Icons.star, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer)
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
            IconButton({ menu = true }) { Icon(Icons.more, "Menu") }
            DropdownMenu(menu, { menu = false }) {
                DropdownMenuItem(
                    text = { Text("Material You colors") },
                    trailingIcon = { Switch(vm.dynamicColor, null) },
                    onClick = { vm.setDynamic(!vm.dynamicColor) },
                    enabled = Build.VERSION.SDK_INT >= 31,
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
    val working = vm.workers.count { it.str("state") in ACTIVE }
    val asks = vm.workers.count { it.str("state") == "asking" }
    NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        for (tab in TABS) {
            NavigationBarItem(
                selected = vm.tab == tab,
                onClick = { vm.tab = tab },
                label = { Text(TITLES[tab]!!.replaceFirstChar { it.uppercase() }.let { if (tab == "chat") "Chat" else it }) },
                icon = {
                    val ic = mapOf("chat" to Icons.chat, "agents" to Icons.agents, "screen" to Icons.screen, "usage" to Icons.usage)[tab]!!
                    BadgedBox(badge = {
                        when {
                            tab == "agents" && working > 0 -> Badge(containerColor = if (asks > 0) LocalExtra.current.warn else MaterialTheme.colorScheme.primary,
                                contentColor = MaterialTheme.colorScheme.onPrimary) { Text(if (working > 99) "99+" else "$working") }
                            tab == "agents" && asks > 0 -> Badge(containerColor = LocalExtra.current.warn)
                            tab == "chat" && vm.tab != "chat" && (vm.unread || vm.waiting > 0) -> Badge(containerColor = MaterialTheme.colorScheme.primary)
                        }
                    }) { Icon(ic, null) }
                },
            )
        }
    }
}

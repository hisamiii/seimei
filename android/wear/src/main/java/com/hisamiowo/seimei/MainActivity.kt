package com.hisamiowo.seimei

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.wear.compose.foundation.BasicSwipeToDismissBox
import androidx.wear.compose.foundation.rememberSwipeToDismissBoxState
import androidx.wear.compose.foundation.CurvedTextStyle
import androidx.wear.compose.material3.TimeText
import androidx.wear.compose.material3.timeTextCurvedText
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    companion object {
        const val CALL = "com.hisamiowo.seimei.CALL"
    }

    private val vm: Model by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = vm.onStart()
            override fun onStop(owner: LifecycleOwner) = vm.onStop()
        })
        take(intent)
        setContent {
            WearTheme(vm.theme) {
                val t = LocalT.current
                Box(Modifier.fillMaxSize().background(t.surface)) {
                    if (vm.phase == Phase.Pair) PairScreen(vm) else Nav(vm)
                    TimeText(contentPadding = androidx.compose.foundation.layout.PaddingValues(top = 6.dp)) { time ->
                        timeTextCurvedText(time, CurvedTextStyle(color = t.text2, fontSize = SMALL, fontWeight = FontWeight.Medium, fontFamily = Sans))
                    }
                    if (vm.phase == Phase.Ready) Rim(vm, usageArc = vm.stack.last() != Screen.Usage)
                    Snack(vm)
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        take(intent)
    }

    /** The call's ongoing activity: the chat, where the call's bar is. The assist gesture, a voice command, or the "seimei voice" entry: the chat with the mic already on. */
    private fun take(intent: Intent?) {
        if (intent?.action == CALL && vm.phase == Phase.Ready) {
            vm.go(Screen.Home)
            vm.homePage = 0
            return
        }
        val voice = intent?.action == Intent.ACTION_ASSIST || intent?.action == Intent.ACTION_VOICE_COMMAND ||
            intent?.component?.className?.endsWith(".Voice") == true
        if (!voice || vm.phase != Phase.Ready) return
        vm.go(Screen.Home)
        vm.homePage = 0
        vm.dictateNow = true
    }
}

@Composable
private fun Nav(vm: Model) {
    val stack = vm.stack
    BackHandler(stack.size > 1 || vm.homePage != 0) {
        if (stack.size > 1) vm.back() else vm.homePage = 0
    }
    if (stack.size == 1) return Show(vm, stack.last())
    val state = rememberSwipeToDismissBoxState()
    BasicSwipeToDismissBox(onDismissed = vm::back, state = state, backgroundKey = stack[stack.size - 2], contentKey = stack.last()) { background ->
        val t = LocalT.current
        Box(Modifier.fillMaxSize().background(t.surface)) { Show(vm, if (background) stack[stack.size - 2] else stack.last()) }
    }
}

@Composable
private fun Show(vm: Model, s: Screen) = when (s) {
    Screen.Home -> Home(vm)
    Screen.Drawer -> Drawer(vm)
    Screen.Agents -> Agents(vm)
    Screen.Threads -> Threads(vm)
    Screen.Watches -> Watches(vm)
    Screen.Files -> Files(vm)
    Screen.Usage -> Usage(vm)
    Screen.Settings -> SettingsScreen(vm)
    is Screen.Agent -> vm.page?.takeIf { it.kind == s.kind && it.id == s.id }?.let { AgentPage(vm, it) } ?: Box(Modifier.fillMaxSize())
}

/** A short note for something that went wrong; a tap retries when there is something to retry. */
@Composable
private fun Snack(vm: Model) {
    LaunchedEffect(CallService.told) {
        CallService.told?.let { vm.tell(it, null) }
        CallService.told = null
    }
    val text = vm.snack ?: return
    val t = LocalT.current
    LaunchedEffect(text) {
        delay(3500)
        vm.snack = null
    }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Box(Modifier.padding(bottom = 52.dp).widthIn(max = 150.dp).clip(RoundedCornerShape(14.dp)).background(t.containerHighest)
            .clickable {
                vm.snack = null
                vm.retry?.invoke()
                vm.retry = null
            }.padding(horizontal = 10.dp, vertical = 6.dp)) {
            T(text + if (vm.retry != null) ". Tap to retry" else "", size = SMALL, color = t.text, lines = 3, align = TextAlign.Center)
        }
    }
}

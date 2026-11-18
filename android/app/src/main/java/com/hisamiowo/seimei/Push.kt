package com.hisamiowo.seimei

import com.hisamiowo.seimei.shared.R
import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.io.IOException

/** Sets up the channels and this build's way of getting notifications (Delivery: Firebase, or UnifiedPush). */
class SeimeiApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Push.start(this)
    }
}

object Push {
    /** One channel per kind, so each can be tuned or muted in the system settings. */
    val CHANNELS = listOf(
        Triple("approval", "Approvals", NotificationManager.IMPORTANCE_HIGH),
        Triple("listen", "Background connection", NotificationManager.IMPORTANCE_MIN),
        Triple("quiet", "Quiet pings", NotificationManager.IMPORTANCE_LOW),
        Triple("ping", "Pings", NotificationManager.IMPORTANCE_DEFAULT),
        Triple("urgent", "Urgent", NotificationManager.IMPORTANCE_HIGH),
        Triple("call", "Voice calls", NotificationManager.IMPORTANCE_LOW),
    )
    val ABOUT = mapOf(
        "approval" to "What a session wants to run, edit, or do",
        "quiet" to "Pings without sound",
        "ping" to "What seimei tells you",
        "urgent" to "Emergencies",
        "listen" to "While seimei keeps its own connection open",
        "call" to "While a voice call runs, with mute and hang up",
    )

    /** Whether the app is on screen; the server holds pushes then, and this catches any that slip through. */
    @Volatile
    var visible = false

    /** The chat is on screen (not another tab or an agent's page): replies show everywhere else. */
    @Volatile
    var onChat = true

    /** The fingerprint-unlocked session, while the app's process lives; null once it locks or the phone unpairs. */
    @Volatile
    var live: Api? = null

    /** When the app would ask for the fingerprint again (it left the screen LOCK_AFTER ago). */
    @Volatile
    var lockAt = Long.MAX_VALUE

    /** What a notification's Allow/Deny may act through: never a stored secret, only the unlocked session. */
    fun session(): Api? = live?.takeIf { System.currentTimeMillis() < lockAt }

    fun start(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        for ((id, name, importance) in CHANNELS) {
            nm.createNotificationChannel(NotificationChannel(id, name, importance).apply { description = ABOUT[id] })
        }
        Delivery.start(ctx)
    }

    fun clear(ctx: Context) = ctx.getSystemService(NotificationManager::class.java).cancelAll()

    fun cancel(ctx: Context, tag: String) = ctx.getSystemService(NotificationManager::class.java).cancel(tag, 0)

    /** Turns one of the server's data messages into a notification: one per kind, replaced by later ones, withdrawn on cancel.
     * Every way of delivery ends here. */
    fun show(ctx: Context, d: Map<String, String>) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        // A kind from a newer server than this app knows still shows, on the Pings channel.
        val kind = (d["kind"] ?: return).let { if (nm.getNotificationChannel(it) != null && it != "listen" && it != "call") it else "ping" }
        val tag = d["tag"] ?: kind
        if (d["cancel"] == "1") return nm.cancel(tag, 0)
        // On screen the server holds pushes; "reply ready" still shows when the chat itself is not in front, a test always.
        if (visible && d["test"] != "1" && (tag != "reply" || onChat)) return
        val open = PendingIntent.getActivity(
            ctx, tag.hashCode(),
            Intent(ctx, MainActivity::class.java).putExtra("tab", d["tab"] ?: "chat").putExtra("event", d["event"])
                .putExtra("thread", d["thread"]).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = d["title"].orEmpty()
        // An approval's body is its command or path: monospace
        val body: CharSequence = d["body"].orEmpty().let {
            if (kind != "approval" || d["plain"] == "1") it
            else android.text.SpannableString(it).apply { setSpan(android.text.style.TypefaceSpan("monospace"), 0, length, 0) }
        }
        // The lock screen shows only that something came, never the command or the text.
        val public = Notification.Builder(ctx, kind).setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(PUBLIC[kind] ?: "seimei").build()
        val n = Notification.Builder(ctx, kind)
            .setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(title).setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(open).setAutoCancel(true)
            .setOnlyAlertOnce(d["alert"] != "1")
            .setCategory(if (tag == "reply") Notification.CATEGORY_MESSAGE else Notification.CATEGORY_REMINDER)
            .setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(public)
        d["group"]?.let { n.setGroup(it) }
        val aid = d["aid"]
        // Each card gets Allow and Deny, plus Always allow when it has a rule to remember, else a deny note ("Keep planning" on a plan card).
        // At most three actions, and only on an unlocked phone (API 31+) while the app's session is unlocked.
        val unlocked = Build.VERSION.SDK_INT >= 31 && session() != null
        if (aid != null && unlocked) {
            val perm = d["perm"] == "1"
            val acts = listOfNotNull(Triple(false, "Deny", if (perm) "deny" else null), Triple(true, "Allow", if (perm) "once" else null),
                d["always"]?.let { Triple(true, "Always allow", it) })
            for ((approve, label, option) in acts) {
                val pi = PendingIntent.getBroadcast(
                    ctx, (aid + label).hashCode(),
                    Intent(ctx, Decide::class.java).putExtra("aid", aid).putExtra("approve", approve).putExtra("option", option),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify), label, pi)
                    .setAuthenticationRequired(true).build())
            }
            if (d["always"] == null) {
                val plan = d["plan"] == "1"
                val input = RemoteInput.Builder(Decide.NOTE).setLabel(if (plan) "What to change in the plan" else "Note for the agent").build()
                val pi = PendingIntent.getBroadcast(
                    ctx, (aid + "note").hashCode(),
                    Intent(ctx, Decide::class.java).putExtra("aid", aid).putExtra("approve", false).putExtra("option", if (perm) "deny" else null),
                    PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify),
                    if (plan) "Keep planning" else "Deny with note", pi)
                    .addRemoteInput(input).setAuthenticationRequired(true).setAllowGeneratedReplies(false).build())
            }
        }
        // The reply ping and a side thread's: answer from the shade (RemoteInput), or open the chat or the thread
        val thread = d["thread"]?.takeIf { tag.startsWith("thread:") }
        if ((tag == "reply" || thread != null) && unlocked) {
            val input = RemoteInput.Builder(Reply.KEY).setLabel(if (thread != null) "Reply to $title" else "Reply to seimei").build()
            val pi = PendingIntent.getBroadcast(ctx, tag.hashCode(), Intent(ctx, Reply::class.java).putExtra("thread", thread).putExtra("tag", tag),
                PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify), "Reply", pi)
                .addRemoteInput(input).setAuthenticationRequired(true).setAllowGeneratedReplies(false).build())
            n.addAction(Notification.Action.Builder(Icon.createWithResource(ctx, R.drawable.ic_notify), "Open", open).build())
        }
        nm.notify(tag, 0, n.build())
        if (d["group"] != null) summary(ctx, d["group"]!!, kind)
    }

    /** A group's summary while two or more of its notifications show ("3 approvals"); gone with the last but one. */
    private fun summary(ctx: Context, group: String, kind: String) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        val n = nm.activeNotifications.count { it.notification.group == group && it.tag != group }
        if (n < 2) return nm.cancel(group, 0)
        val title = if (group == "approvals") "$n approvals" else "$n from seimei"
        nm.notify(group, 0, Notification.Builder(ctx, kind).setSmallIcon(R.drawable.ic_notify).setColor(MAUVE)
            .setContentTitle(title).setGroup(group).setGroupSummary(true).setOnlyAlertOnce(true).setAutoCancel(true)
            .setVisibility(Notification.VISIBILITY_PRIVATE).build())
    }

    /** A card's notification, without actions, when the answer from the shade didn't land: the app answers it. */
    fun openToAnswer(ctx: Context, aid: String, title: String, body: String) =
        show(ctx, mapOf("kind" to "approval", "tag" to "approval:$aid", "group" to "approvals", "title" to title, "body" to body, "tab" to "chat"))

    /** Withdraws a card's notification. */
    fun settled(ctx: Context, aid: String) {
        ctx.getSystemService(NotificationManager::class.java).cancel("approval:$aid", 0)
        summary(ctx, "approvals", "approval")
    }

    const val MAUVE = 0xFFCBA6F7.toInt()
    val PUBLIC = mapOf("approval" to "Approval waiting", "quiet" to "seimei", "ping" to "seimei",
        "urgent" to "seimei: urgent")
}

/** Allow, Deny or a note on an approval notification: settles that one card through the unlocked session.
 * With no live process or a locked session it asks to open the app: Android blocks starting an activity from here. */
class Decide : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val aid = intent.getStringExtra("aid") ?: return
        val approve = intent.getBooleanExtra("approve", false)
        val api = Push.session() ?: return Push.openToAnswer(ctx, aid, "Couldn't send your answer", "The app is locked. Tap to answer")
        val body = JSONObject().put("decision", if (approve) "approve" else "deny")
        intent.getStringExtra("option")?.let { body.put("option", it) }
        RemoteInput.getResultsFromIntent(intent)?.getCharSequence(NOTE)?.toString()?.trim()?.takeIf { it.isNotEmpty() }
            ?.let { body.put("note", it) }
        val done = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                api.post("api/approvals/$aid", body)
                Push.settled(ctx, aid)
            } catch (e: HttpError) {
                if (e.code == 409) Push.settled(ctx, aid) else Push.openToAnswer(ctx, aid, "Couldn't send your answer", "Tap to try again")
            } catch (e: IOException) {
                Push.openToAnswer(ctx, aid, "Couldn't send your answer", "Couldn't reach the server. Tap to answer")
            } finally {
                done.finish()
            }
        }
    }

    companion object {
        const val NOTE = "note"
    }
}

/** Reply on the reply ping (or a side thread's): the text goes to the chat (or the thread) through the unlocked session.
 * The notification then shows what was sent.
 * On failure it keeps the text, so nothing typed is lost. */
class Reply : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val text = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY)?.toString()?.trim().orEmpty()
        if (text.isEmpty()) return
        val api = Push.session()
        val thread = intent.getStringExtra("thread")
        val tag = intent.getStringExtra("tag") ?: "reply"
        val done = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            val sent = try {
                if (thread != null) api?.post("api/threads/$thread/send", JSONObject().put("text", text)) != null
                else api?.post("api/send", JSONObject().put("text", text).put("cid", java.util.UUID.randomUUID().toString())
                    .put("images", org.json.JSONArray())) != null
            } catch (e: IOException) {
                false
            }
            val nm = ctx.getSystemService(NotificationManager::class.java)
            nm.notify(tag, 0, Notification.Builder(ctx, "ping").setSmallIcon(R.drawable.ic_notify).setColor(Push.MAUVE)
                .setContentTitle(if (sent) "Sent" else "Couldn't send").setContentText(text)
                .setStyle(Notification.BigTextStyle().bigText(text)).setOnlyAlertOnce(true).setAutoCancel(true)
                .setTimeoutAfter(if (sent) 8_000 else 0).setVisibility(Notification.VISIBILITY_PRIVATE).build())
            done.finish()
        }
    }

    companion object {
        const val KEY = "reply"
    }
}

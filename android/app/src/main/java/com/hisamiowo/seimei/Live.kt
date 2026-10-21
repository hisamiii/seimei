package com.hisamiowo.seimei

import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.util.concurrent.ConcurrentLinkedQueue
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import org.json.JSONObject

/**
 * The live screen: H.264 from api/stream/screen (a config, then access units: flags u8, pts u64 µs, Annex B bytes)
 * into a MediaCodec decoder drawing straight onto a Surface. Surfaces stack: full screen borrows the stream and
 * gives it back. A decoder that falls behind skips to a keyframe it asks for.
 */
class LivePlayer(private val api: () -> Api) {
    /** Width and height of the stream, once it has a config. */
    var size by mutableStateOf<Pair<Int, Int>?>(null)
    /** What to show over the picture: "" while frames flow. */
    var note by mutableStateOf("")
    /** No decoder here for the stream: the tab goes back to stills. */
    var unsupported by mutableStateOf(false)

    private val surfaces = ArrayList<Surface>()
    private val inputs = ConcurrentLinkedQueue<Int>()
    private var thread: HandlerThread? = null
    @Volatile private var codec: MediaCodec? = null
    private var config: JSONObject? = null
    private var ws: WebSocket? = null
    private var want = false
    private var retry = 0
    @Volatile private var needKey = true
    private val main = Handler(android.os.Looper.getMainLooper())

    fun start() {
        if (want) return
        want = true
        open()
    }

    fun stop() {
        want = false
        main.removeCallbacksAndMessages(null)
        ws?.close(1000, null)
        ws = null
        release()
    }

    fun attach(s: Surface) {
        surfaces.remove(s)
        surfaces.add(s)
        val c = codec
        if (c != null) c.setOutputSurface(s) else config?.let { configure(it) }
    }

    fun detach(s: Surface) {
        val top = surfaces.lastOrNull() == s
        surfaces.remove(s)
        if (!top) return
        val next = surfaces.lastOrNull()
        if (next != null) codec?.setOutputSurface(next) else release()
    }

    private fun open() {
        ws = api().socket("api/stream/screen", object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) {
                val m = JSONObject(text)
                main.post {
                    if (webSocket !== ws) return@post
                    when (m.str("type")) {
                        "config" -> { config = m; size = m.optInt("width") to m.optInt("height"); configure(m) }
                        "paused" -> { release(); note = "Paused while a benchmark runs" }
                        "error" -> note = m.str("text").ifEmpty { "The screen capture stopped" }
                    }
                }
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                if (webSocket === ws) feed(bytes.toByteArray())
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = ended(webSocket)
            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = ended(webSocket)
        })
    }

    private fun ended(webSocket: WebSocket) {
        main.post { reconnect(webSocket) }
    }

    private fun reconnect(webSocket: WebSocket) {
        if (webSocket !== ws || !want) return
        ws = null
        release()
        note = "Reconnecting…"
        // Back off: 1 s, 2 s, 4 s ... 15 s.
        main.postDelayed({ if (want && ws == null) open() }, minOf(15000L, 1000L shl minOf(retry++, 4)))
    }

    private fun configure(m: JSONObject) {
        release()
        val s = surfaces.lastOrNull() ?: return
        val fmt = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, m.optInt("width"), m.optInt("height"))
        fmt.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        val name = MediaCodecList(MediaCodecList.REGULAR_CODECS).findDecoderForFormat(fmt)
        if (name == null) {
            unsupported = true
            stop()
            return
        }
        val t = HandlerThread("live-decoder").also { it.start() }
        val c = MediaCodec.createByCodecName(name)
        c.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
                inputs.add(index)
            }

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                codec.releaseOutputBuffer(index, true)
                if (note.isNotEmpty() || retry > 0) main.post { note = ""; retry = 0 }
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                main.post { config?.let { configure(it) } }
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
        }, Handler(t.looper))
        c.configure(fmt, s, null, 0)
        inputs.clear()
        needKey = true
        c.start()
        thread = t
        codec = c
        // The GOP sent on joining may have gone by before the decoder existed: start from a fresh keyframe.
        ws?.send("key")
    }

    private fun feed(msg: ByteArray) {
        val c = codec ?: return
        val key = msg[0].toInt() and 1 == 1
        if (needKey && !key) return
        val index = inputs.poll()
        if (index == null) {
            // The decoder is behind: drop until a fresh keyframe.
            needKey = true
            ws?.send("key")
            return
        }
        var pts = 0L
        for (i in 1..8) pts = (pts shl 8) or (msg[i].toLong() and 0xFF)
        try {
            val buf = c.getInputBuffer(index) ?: return
            buf.clear()
            buf.put(msg, 9, msg.size - 9)
            c.queueInputBuffer(index, 0, msg.size - 9, pts, 0)
            needKey = false
        } catch (e: IllegalStateException) {
            needKey = true
        }
    }

    private fun release() {
        val c = codec
        codec = null
        inputs.clear()
        try {
            c?.stop()
        } catch (e: IllegalStateException) {
        }
        c?.release()
        thread?.quitSafely()
        thread = null
    }
}

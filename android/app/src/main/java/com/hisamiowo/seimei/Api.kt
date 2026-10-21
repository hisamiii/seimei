package com.hisamiowo.seimei

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.URLEncoder
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okio.BufferedSink
import okio.buffer
import okio.sink
import okio.source
import org.json.JSONObject

class HttpError(val code: Int, val body: String) : IOException("HTTP $code: ${body.take(200)}")

/** The server's HTTP API. After login every call carries the bearer token. */
class Api(server: String) {
    val base = server.trimEnd('/') + "/"

    @Volatile
    var token: String? = null

    // The server pings the stream every 15 s, so 40 s of silence means the link is dead.
    private val client = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(40, TimeUnit.SECONDS)
        .build()

    private fun request(path: String) = Request.Builder().url(base + path).apply {
        token?.let { header("Authorization", "Bearer $it") }
    }

    private suspend fun call(req: Request): ByteArray = withContext(Dispatchers.IO) {
        client.newCall(req).execute().use { r ->
            val body = r.body.bytes()
            if (!r.isSuccessful) throw HttpError(r.code, String(body).take(2000))
            body
        }
    }

    suspend fun get(path: String) = JSONObject(String(call(request(path).build())))

    suspend fun bytes(path: String) = call(request(path).build())

    suspend fun post(path: String, body: JSONObject = JSONObject()): JSONObject {
        val req = request(path).post(body.toString().toRequestBody(JSON)).build()
        return JSONObject(String(call(req)))
    }

    /** Runs a call that stops at once when its coroutine is cancelled, reading the body with [read]. */
    private suspend fun <T> await(req: Request, read: (okhttp3.ResponseBody) -> T): T = suspendCancellableCoroutine { cont ->
        val call = client.newCall(req)
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : okhttp3.Callback {
            override fun onFailure(call: okhttp3.Call, e: IOException) = cont.resumeWithException(e)
            override fun onResponse(call: okhttp3.Call, response: okhttp3.Response) = response.use { r ->
                try {
                    if (!r.isSuccessful) throw HttpError(r.code, r.body.string().take(2000))
                    cont.resume(read(r.body))
                } catch (e: Exception) {
                    cont.resumeWithException(e)
                }
            }
        })
    }

    /** Streams a file up to the server; [open] is read once, never held in memory whole. */
    suspend fun upload(name: String, open: () -> InputStream): JSONObject {
        val body = object : RequestBody() {
            override fun contentType() = OCTET
            override fun writeTo(sink: BufferedSink) {
                // Anything but an IOException thrown here would crash OkHttp's thread, not fail the call.
                val input = try {
                    open()
                } catch (e: IOException) {
                    throw e
                } catch (e: Exception) {
                    throw IOException("cannot read the file", e)
                }
                input.use { sink.writeAll(it.source()) }
            }
        }
        val req = request("api/upload?name=" + URLEncoder.encode(name, "UTF-8")).post(body).build()
        return JSONObject(await(req) { it.string() })
    }

    /** Streams a server file into [out], telling [progress] the bytes so far. */
    suspend fun download(path: String, out: OutputStream, progress: (Long) -> Unit = {}) = await(request(path).build()) { body ->
        val src = body.source()
        val buf = okio.Buffer()
        var done = 0L
        out.sink().buffer().use { sink ->
            while (true) {
                val n = src.read(buf, 1L shl 16)
                if (n < 0) break
                sink.write(buf, n)
                done += n
                progress(done)
            }
        }
    }

    @Volatile
    private var current: okhttp3.Call? = null

    /**
     * Closes the event stream's connection now. Cancelling its coroutine alone waits for the blocked read to
     * return, which takes until the server next writes; meanwhile the server would think the app is on screen.
     */
    fun closeStream() {
        current?.cancel()
    }

    /** Reads the event stream until it drops; each data line is one JSON event. */
    suspend fun stream(after: Long, onOpen: () -> Unit, onEvent: (JSONObject) -> Unit) = withContext(Dispatchers.IO) {
        val req = request("api/stream?after=$after").header("Accept", "text/event-stream").build()
        val call = client.newCall(req)
        current = call
        val ctx = currentCoroutineContext()
        ctx[kotlinx.coroutines.Job]?.invokeOnCompletion { call.cancel() }
        call.execute().use { r ->
            if (!r.isSuccessful) throw HttpError(r.code, "stream")
            onOpen()
            val src = r.body.source()
            while (true) {
                ctx.ensureActive()
                val line = src.readUtf8Line() ?: break
                if (line.startsWith("data:")) onEvent(JSONObject(line.substring(5).trim()))
            }
        }
    }

    companion object {
        val JSON = "application/json".toMediaType()
        val OCTET = "application/octet-stream".toMediaType()
    }
}

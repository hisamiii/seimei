package org.example.seimei

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.io.IOException
import java.util.concurrent.TimeUnit

class HttpError(val code: Int, val body: String) : IOException("HTTP $code: ${body.take(200)}")

/** The laptop's HTTP API. After login every call carries the bearer token. */
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

    /** Reads the event stream until it drops; each data line is one JSON event. */
    suspend fun stream(after: Long, onOpen: () -> Unit, onEvent: (JSONObject) -> Unit) = withContext(Dispatchers.IO) {
        val req = request("api/stream?after=$after").header("Accept", "text/event-stream").build()
        val call = client.newCall(req)
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
    }
}

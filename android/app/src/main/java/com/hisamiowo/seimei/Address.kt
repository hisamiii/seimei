package com.hisamiowo.seimei

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** What the server step makes of a typed address: the origin to talk to, or why not. */
sealed interface Address {
    data class Ok(val url: String) : Address {
        /** The address as the user knows it: no https://. */
        val shown get() = url.substringAfter("://")
    }

    data class Bad(val why: String) : Address
}

const val HTTPS_ONLY = "Needs an https address"

/**
 * The origin in what the user typed: https:// unless given, no path, no login.
 * The server checks a browser login's Origin against its config, so this must come out exactly as that would.
 * Plain http only reaches this phone's own localhost (the tests, through adb reverse).
 */
fun address(typed: String): Address {
    val t = typed.trim()
    if (t.isEmpty()) return Address.Bad("Type the server's address")
    val u = (if ("://" in t) t else "https://$t").toHttpUrlOrNull() ?: return Address.Bad("That isn't a server address")
    if (u.scheme == "http" && u.host !in setOf("localhost", "127.0.0.1")) return Address.Bad(HTTPS_ONLY)
    val origin = u.newBuilder().username("").password("").encodedPath("/").query(null).fragment(null).build()
    return Address.Ok(origin.toString().trimEnd('/'))
}

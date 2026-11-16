package com.hisamiowo.seimei

import com.hisamiowo.seimei.wear.BuildConfig
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.Base64

/** What the watch remembers between launches. The login token is never stored. */
class Prefs(ctx: Context) {
    private val p = ctx.getSharedPreferences("seimei", Context.MODE_PRIVATE)

    var server: String
        get() = p.getString("server", null) ?: BuildConfig.SERVER
        set(v) = p.edit().putString("server", v).apply()
    var device: String?
        get() = p.getString("device", null)
        set(v) = p.edit().putString("device", v).apply()
    var theme: String
        get() = p.getString("theme", null) ?: "catppuccin"
        set(v) = p.edit().putString("theme", v).apply()
    var draft: String
        get() = p.getString("draft", "") ?: ""
        set(v) = p.edit().putString("draft", v).apply()
    /** The Firebase token, kept for the next login to hand to the server. */
    var pushToken: String?
        get() = p.getString("pushToken", null)
        set(v) = p.edit().putString("pushToken", v).apply()
    /** session id -> the last activity time the user has looked at, for "new reply" marks. */
    var seen: String
        get() = p.getString("seen", "{}") ?: "{}"
        set(v) = p.edit().putString("seen", v).apply()
}

private val b64url: Base64.Encoder = Base64.getUrlEncoder().withoutPadding()

/**
 * The watch's identity: an EC P-256 key in the Android keystore, which never leaves the secure hardware.
 * A watch has no fingerprint: the key works only while the watch is unlocked, so a watch taken off the wrist cannot open seimei.
 */
object WatchKey {
    private const val ALIAS = "seimei-device"
    private val LOGIN = "seimei device login\n".toByteArray()

    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    fun exists() = store().containsAlias(ALIAS)

    fun delete() {
        store().deleteEntry(ALIAS)
    }

    /** Makes a new key and returns its public half (SPKI DER, base64url). */
    fun create(): String {
        delete()
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUnlockedDeviceRequired(true)
            .build()
        val gen = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        gen.initialize(spec)
        return b64url.encodeToString(gen.generateKeyPair().public.encoded)
    }

    fun sign(nonce: String): String {
        val s = Signature.getInstance("SHA256withECDSA")
        s.initSign(store().getKey(ALIAS, null) as PrivateKey)
        s.update(LOGIN + nonce.toByteArray())
        return b64url.encodeToString(s.sign())
    }
}

package com.hisamiowo.seimei

import android.app.Activity
import android.content.Context
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricPrompt
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.Base64
import kotlin.coroutines.resume
import kotlin.coroutines.suspendCoroutine

/** What the app remembers between launches. The login token is never stored. */
class Prefs(ctx: Context) {
    private val p = ctx.getSharedPreferences("seimei", Context.MODE_PRIVATE)

    var server: String
        get() = p.getString("server", null) ?: BuildConfig.SERVER
        set(v) = p.edit().putString("server", v).apply()
    var device: String?
        get() = p.getString("device", null)
        set(v) = p.edit().putString("device", v).apply()
    /** Whether the device key needs the fingerprint (false only for debug test keys). */
    var keyAuth: Boolean
        get() = p.getBoolean("keyAuth", true)
        set(v) = p.edit().putBoolean("keyAuth", v).apply()
    /** The theme: one of THEMES. */
    var theme: String
        get() = p.getString("theme", null) ?: "catppuccin"
        set(v) = p.edit().putString("theme", v).apply()
    /** Light or dark: "system", "light" or "dark". */
    var themeMode: String
        get() = p.getString("themeMode", "system") ?: "system"
        set(v) = p.edit().putString("themeMode", v).apply()
    /** Catppuccin's accent; null for its default (mauve). */
    var accent: String?
        get() = p.getString("accent", null)
        set(v) = p.edit().putString("accent", v).apply()
    /** The wallpaper seed last sent to the server, so it is sent again only when the wallpaper changes. */
    var seedSent: String?
        get() = p.getString("seedSent", null)
        set(v) = p.edit().putString("seedSent", v).apply()
    var draft: String
        get() = p.getString("draft", "") ?: ""
        set(v) = p.edit().putString("draft", v).apply()
    /** The Firebase token, kept for the next login to hand to the server. */
    var pushToken: String?
        get() = p.getString("pushToken", null)
        set(v) = p.edit().putString("pushToken", v).apply()
    /** The push app's endpoint and keys (UnifiedPush), as JSON for the server; null before it gave one. */
    var upEndpoint: String?
        get() = p.getString("upEndpoint", null)
        set(v) = p.edit().putString("upEndpoint", v).apply()
    /** The background connection's secret: it opens only the push stream, so it may wait here for a reboot. */
    var listenSecret: String?
        get() = p.getString("listenSecret", null)
        set(v) = p.edit().putString("listenSecret", v).apply()
    /** Without a push app: keep the background connection (the user may turn it off). */
    var listen: Boolean
        get() = p.getBoolean("listen", true)
        set(v) = p.edit().putBoolean("listen", v).apply()
    var askedNotifications: Boolean
        get() = p.getBoolean("askedNotifications", false)
        set(v) = p.edit().putBoolean("askedNotifications", v).apply()
    /** server file id -> the copy saved in Downloads, so a second tap opens it. */
    var saved: String
        get() = p.getString("saved", "{}") ?: "{}"
        set(v) = p.edit().putString("saved", v).apply()
    /** session id -> the last activity time the user has looked at, for "new reply" marks. */
    var seen: String
        get() = p.getString("seen", "{}") ?: "{}"
        set(v) = p.edit().putString("seen", v).apply()
    /** Task board sections opened or shut by hand: project name -> open. */
    var boardOpen: String
        get() = p.getString("boardOpen", "{}") ?: "{}"
        set(v) = p.edit().putString("boardOpen", v).apply()
}

val b64url: Base64.Encoder = Base64.getUrlEncoder().withoutPadding()

/**
 * The phone's identity: an EC P-256 key in the Android keystore.
 * It never leaves the secure hardware, and each use needs a fresh fingerprint, so a stolen unlocked phone still cannot open seimei.
 */
object DeviceKey {
    private const val ALIAS = "seimei-device"
    val LOGIN = "seimei device login\n".toByteArray()

    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    fun exists() = store().containsAlias(ALIAS)

    fun delete() {
        store().deleteEntry(ALIAS)
    }

    /** Makes a new key and returns its public half (SPKI DER, base64url). */
    fun create(fingerprint: Boolean): String {
        delete()
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .apply {
                if (fingerprint) {
                    setUserAuthenticationRequired(true)
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                    setInvalidatedByBiometricEnrollment(true)
                }
            }
            .build()
        val gen = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        gen.initialize(spec)
        return b64url.encodeToString(gen.generateKeyPair().public.encoded)
    }

    /** A signer bound to the key. Throws KeyPermanentlyInvalidatedException after a new fingerprint was enrolled. */
    fun signer(): Signature = Signature.getInstance("SHA256withECDSA").apply {
        initSign(store().getKey(ALIAS, null) as PrivateKey)
    }

    fun sign(signer: Signature, nonce: String): String {
        signer.update(LOGIN + nonce.toByteArray())
        return b64url.encodeToString(signer.sign())
    }
}

fun canFingerprint(ctx: Context): Boolean =
    ctx.getSystemService(BiometricManager::class.java).canAuthenticate(BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS

sealed interface Unlock {
    data class Ok(val signer: Signature) : Unlock
    data object Cancelled : Unlock
    data class Failed(val why: String) : Unlock
}

/** Shows the fingerprint prompt; a success unlocks the signer for exactly one signature. */
suspend fun fingerprint(activity: Activity, signer: Signature): Unlock = suspendCoroutine { cont ->
    var done = false
    fun finish(u: Unlock) {
        if (!done) {
            done = true
            cont.resume(u)
        }
    }
    val exec = activity.mainExecutor
    val prompt = BiometricPrompt.Builder(activity)
        .setTitle("Open seimei")
        .setAllowedAuthenticators(BIOMETRIC_STRONG)
        .setNegativeButton("Cancel", exec) { _, _ -> finish(Unlock.Cancelled) }
        .build()
    prompt.authenticate(BiometricPrompt.CryptoObject(signer), CancellationSignal(), exec,
        object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val s = result.cryptoObject?.signature
                finish(if (s != null) Unlock.Ok(s) else Unlock.Failed("The fingerprint didn't unlock the key"))
            }

            override fun onAuthenticationError(code: Int, msg: CharSequence) {
                val cancelled = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED || code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED
                finish(if (cancelled) Unlock.Cancelled else Unlock.Failed(msg.toString()))
            }
        })
}

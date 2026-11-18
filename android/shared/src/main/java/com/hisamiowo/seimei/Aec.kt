package com.hisamiowo.seimei

/**
 * The app's own echo canceller: webrtc's AEC3, its noise suppressor and high-pass filter (src/main/cpp/echo.cc), on 16 kHz mono in 10 ms frames.
 * [far] takes what goes to the speaker, in order and at its pace; [near] cleans the microphone's frames in place.
 * The speaker's thread calls [far] and the microphone's [near]: the native side locks each path on its own.
 */
class Aec private constructor(private var h: Long) {
    // 10 ms of what went to the speaker, gathered from writes of any size.
    private val pending = ShortArray(FRAME)
    private var have = 0

    /** [n] samples from [off] went to the speaker. */
    fun far(s: ShortArray, off: Int = 0, n: Int = s.size) {
        var i = off
        val end = off + n
        while (i < end) {
            val k = minOf(FRAME - have, end - i)
            System.arraycopy(s, i, pending, have, k)
            have += k
            i += k
            if (have == FRAME) {
                nFar(h, pending, 0)
                have = 0
            }
        }
    }

    /** A microphone frame of whole 10 ms frames, cleaned in place; [delayMs] is a hint of the speaker's and microphone's latency. */
    fun near(f: ShortArray, delayMs: Int) {
        var off = 0
        while (off + FRAME <= f.size) {
            nNear(h, f, off, delayMs)
            off += FRAME
        }
    }

    /** Echo return loss and its enhancement in dB, the delay in ms and the residual echo likelihood; NaN where unknown. */
    fun stats() = DoubleArray(4).also { nStats(h, it) }

    /** Call once both threads are done with it. */
    fun release() {
        if (h != 0L) nFree(h)
        h = 0
    }

    companion object {
        const val FRAME = 160

        /** A canceller, or null when this build or phone has none. */
        fun make(): Aec? = try {
            System.loadLibrary("seimeiecho")
            nMake().takeIf { it != 0L }?.let(::Aec)
        } catch (e: UnsatisfiedLinkError) {
            null
        }

        @JvmStatic private external fun nMake(): Long
        @JvmStatic private external fun nFar(h: Long, f: ShortArray, off: Int)
        @JvmStatic private external fun nNear(h: Long, f: ShortArray, off: Int, delay: Int)
        @JvmStatic private external fun nStats(h: Long, out: DoubleArray)
        @JvmStatic private external fun nFree(h: Long)
    }
}

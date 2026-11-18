// The JNI side of Aec.kt: one canceller per call, a handle as a long.
#include <jni.h>

#include "echo.h"

extern "C" {

JNIEXPORT jlong JNICALL Java_com_hisamiowo_seimei_Aec_nMake(JNIEnv*, jclass) {
    return reinterpret_cast<jlong>(echo_new());
}

JNIEXPORT void JNICALL Java_com_hisamiowo_seimei_Aec_nFar(JNIEnv* env, jclass, jlong h, jshortArray f, jint off) {
    int16_t frame[ECHO_FRAME];
    env->GetShortArrayRegion(f, off, ECHO_FRAME, frame);
    echo_far(reinterpret_cast<Echo*>(h), frame);
}

JNIEXPORT void JNICALL Java_com_hisamiowo_seimei_Aec_nNear(JNIEnv* env, jclass, jlong h, jshortArray f, jint off, jint delay) {
    int16_t frame[ECHO_FRAME];
    env->GetShortArrayRegion(f, off, ECHO_FRAME, frame);
    echo_near(reinterpret_cast<Echo*>(h), frame, delay);
    env->SetShortArrayRegion(f, off, ECHO_FRAME, frame);
}

JNIEXPORT void JNICALL Java_com_hisamiowo_seimei_Aec_nStats(JNIEnv* env, jclass, jlong h, jdoubleArray out) {
    double s[4];
    echo_stats(reinterpret_cast<Echo*>(h), s);
    env->SetDoubleArrayRegion(out, 0, 4, s);
}

JNIEXPORT void JNICALL Java_com_hisamiowo_seimei_Aec_nFree(JNIEnv*, jclass, jlong h) {
    echo_free(reinterpret_cast<Echo*>(h));
}

}

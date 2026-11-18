// The app's echo canceller: webrtc's audio processing (AEC3, its noise suppressor and high-pass filter) on 16 kHz mono in 10 ms frames.
// A small C API, so the JNI layer and a desktop test can share it.
#include "echo.h"

#include <cmath>

#include "api/audio/audio_processing.h"

struct Echo {
    rtc::scoped_refptr<webrtc::AudioProcessing> apm;
    webrtc::StreamConfig stream{ECHO_RATE, 1};
};

extern "C" {

Echo* echo_new(void) {
    webrtc::AudioProcessing::Config c;
    c.echo_canceller.enabled = true;
    c.echo_canceller.mobile_mode = false;
    c.noise_suppression.enabled = true;
    c.noise_suppression.level = webrtc::AudioProcessing::Config::NoiseSuppression::kModerate;
    c.high_pass_filter.enabled = true;
    auto* e = new Echo;
    e->apm = webrtc::AudioProcessingBuilder().SetConfig(c).Create();
    if (!e->apm) {
        delete e;
        return nullptr;
    }
    return e;
}

int echo_far(Echo* e, const int16_t* frame) {
    int16_t out[ECHO_FRAME];
    return e->apm->ProcessReverseStream(frame, e->stream, e->stream, out);
}

int echo_near(Echo* e, int16_t* frame, int delay_ms) {
    e->apm->set_stream_delay_ms(delay_ms);
    return e->apm->ProcessStream(frame, e->stream, e->stream, frame);
}

void echo_stats(Echo* e, double* out) {
    // true: the echo statistics are only kept while there is a far end.
    auto s = e->apm->GetStatistics(true);
    out[0] = s.echo_return_loss.value_or(NAN);
    out[1] = s.echo_return_loss_enhancement.value_or(NAN);
    out[2] = s.delay_ms ? *s.delay_ms : NAN;
    out[3] = s.residual_echo_likelihood.value_or(NAN);
}

void echo_free(Echo* e) {
    delete e;
}

}

// The app's echo canceller (echo.cc): 16 kHz mono, 10 ms frames of 16-bit samples.
#pragma once

#include <stdint.h>

#define ECHO_RATE 16000
#define ECHO_FRAME 160

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Echo Echo;

/** A new canceller, or null. */
Echo* echo_new(void);
/** A frame that went to the speaker: the echo's reference. 0 on success. */
int echo_far(Echo* e, const int16_t* frame);
/** A microphone frame, cleaned in place; delay_ms is the playout and capture latency, a hint. 0 on success. */
int echo_near(Echo* e, int16_t* frame, int delay_ms);
/** Echo return loss and its enhancement in dB, the delay in ms and the residual echo likelihood; NaN where unknown. */
void echo_stats(Echo* e, double* out);
void echo_free(Echo* e);

#ifdef __cplusplus
}
#endif

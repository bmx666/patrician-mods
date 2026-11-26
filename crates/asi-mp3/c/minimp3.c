/* minimp3 (CC0, LICENSE.minimp3), SIMD path only, plus the frame length of a header for the Rust side. */
#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_SIMD
#include "minimp3.h"

/* Length of the frame whose 4-byte header is at h (padding included), 0 = no valid header or free format. */
int mp3_frame_len(const uint8_t *h) {
    return hdr_valid(h) ? hdr_frame_bytes(h, 0) + hdr_padding(h) : 0;
}

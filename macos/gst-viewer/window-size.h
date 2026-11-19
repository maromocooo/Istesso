// How the everyday trial's viewer sizes its window and its picture against each other
// (ISTESSO_DAILY_RECONNECT, docs/trial-daily.md and the Mac host's docs/m4-daily-host.md, "Air の窓の
// 大きさと、前面"). Pure arithmetic, tested by window-size-test.c without a window.
#ifndef ISTESSO_WINDOW_SIZE_H
#define ISTESSO_WINDOW_SIZE_H

#include <math.h>

// The sizes a resize may ask the host for: 64 to 8192 pixels a side (istesso_proto::resize_is_acceptable).
enum { RESIZE_LEAST = 64, RESIZE_MOST = 8192 };

static inline unsigned resize_clamp(double pixels) {
    double whole = floor(pixels + 0.5);
    return whole < RESIZE_LEAST ? RESIZE_LEAST : whole > RESIZE_MOST ? RESIZE_MOST : (unsigned)whole;
}

// The size the picture area asks the host for, once its window was resized by hand: the area's
// points, at `each` points to a picture pixel, in picture pixels.
static inline void picture_size_for_window(double width, double height, double each, unsigned *out_width,
                                           unsigned *out_height) {
    if (!(each > 0)) {
        each = 1;
    }
    *out_width = resize_clamp(width / each);
    *out_height = resize_clamp(height / each);
}

// The picture area for a picture of `width` x `height` pixels: `each` points to a pixel, made
// smaller as a whole (the same for both sides, so that the pixels stay square) when it would not
// fit into `room_width` x `room_height` points. Whole points.
static inline void window_size_for_picture(unsigned width, unsigned height, double each, double room_width,
                                           double room_height, double *out_width, double *out_height) {
    if (!(each > 0)) {
        each = 1;
    }
    double fit = 1;
    if (width > 0 && height > 0) {
        fit = fmin(1.0, fmin(room_width / (width * each), room_height / (height * each)));
    }
    // Whole points, not one less for a rounding error of the division.
    *out_width = floor(width * each * fit + 1e-6);
    *out_height = floor(height * each * fit + 1e-6);
}

#endif

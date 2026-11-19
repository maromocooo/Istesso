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

// The host's density: picture pixels for one point of the host's application. The Mac host's
// application on its 2x virtual display: 2. Ubuntu's compositor (scale 1): 1. The viewer is told it
// (ISTESSO_PICTURE_DENSITY); `text` NULL, or not a number from 0.25 to 4: `fallback`.
static inline double picture_density_from(const char *text, double fallback) {
    if (text == NULL) {
        return fallback;
    }
    char *end = NULL;
    double value = strtod(text, &end);
    if (end == text || *end != '\0' || !(value >= 0.25 && value <= 4)) {
        return fallback;
    }
    return value;
}

// The one window's sizing, kept by the viewer on its main thread.
//
// `each`, the points of the window to one picture pixel, is 1 / the host's density, for every
// picture. It is never taken from a picture: the first one may come before the host's window is
// on its 2x display (1906x1074 of Dia at 1x, then 3420x2150 at 2x), and the window may have been
// opened at a size guessed before it. Nor from the backing scale of the screen this window is on:
// what the host's application looks like in points does not depend on that screen. A window
// dragged from the Air's 2x screen onto a 1x monitor keeps its size in points (macOS keeps it),
// and so does its picture: on the 1x screen a 2x picture is shown at half its pixels, as macOS
// shows any Retina window there. Shown pixel for pixel there, everything would be twice as large.
typedef struct {
    double each;
    int resize_asked;        // a resize was asked of the host; its answer (the next stream's size) is awaited
    unsigned width, height;  // the picture's size last seen; 0 before the first
} WindowSizing;

static inline WindowSizing window_sizing_start(double density) {
    WindowSizing sizing = {density > 0 ? 1 / density : 1, 0, 0, 0};
    return sizing;
}

// A new stream of `width` x `height` pixels arrived. The window's picture area is `content_width` x
// `content_height` points, and its screen has `room_width` x `room_height` points for it. Returns 1,
// with the area's new size in `out_*`, when the window is to change: the size changed, or this is
// the answer to a resize that was asked (also when it is the same size), or it is the first picture
// and the window does not have its size at `each` (the window was opened at a size guessed before
// any picture: the Mac host's launcher takes the size the application's window had on the host's
// own 1x screen; Ubuntu's launcher opens it at the picture's size, which it keeps). 0: it stays.
static inline int window_sizing_picture(WindowSizing *s, unsigned width, unsigned height, double content_width,
                                        double content_height, double room_width, double room_height,
                                        double *out_width, double *out_height) {
    int first = s->width == 0 || s->height == 0;
    int changed = !first && (width != s->width || height != s->height);
    int answer = s->resize_asked;
    s->width = width;
    s->height = height;
    s->resize_asked = 0;
    window_size_for_picture(width, height, s->each, room_width, room_height, out_width, out_height);
    if (changed || answer) {
        return 1;
    }
    if (first) {
        // Compared with the picture at `each` as it is, not made smaller for the screen: a window
        // that already has its picture's size stays as it was opened.
        double whole_width, whole_height;
        window_size_for_picture(width, height, s->each, INFINITY, INFINITY, &whole_width, &whole_height);
        return fabs(whole_width - content_width) > 2 || fabs(whole_height - content_height) > 2;
    }
    return 0;
}

// The person has stopped resizing the window by hand; its picture area is `width` x `height` points.
// The size to ask the host for, in picture pixels; its answer is then awaited.
static inline void window_sizing_hand_resized(WindowSizing *s, double width, double height, unsigned *out_width,
                                              unsigned *out_height) {
    picture_size_for_window(width, height, s->each, out_width, out_height);
    s->resize_asked = 1;
}

// Pixels of a screen of backing scale `backing` (pixels a point) that one picture pixel takes, when
// the window has the picture's size at `each`: 1 is pixel for pixel. For the log only. The window's
// screen changing its scale changes nothing else, and nothing is asked of the host: its picture
// already has the pixels for the window's points at the host's density.
static inline double screen_pixels_per_picture_pixel(const WindowSizing *s, double backing) {
    return s->each * backing;
}

#endif

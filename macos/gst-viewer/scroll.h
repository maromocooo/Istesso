// What the trial viewer sends for one scroll event of macOS (docs/adr/0007-control-channel.md,
// "スクロールの種類と段階"). Plain C, so that it is tested without a window (scroll-test.c).
//
// A wheel (no precise deltas) goes as notches, 120 to a notch: the whole lines of the event
// (kCGScrollWheelEventDeltaAxis1 and 2), not the fractional lines macOS speeds up. Fingers on a
// trackpad (precise deltas) go as pixels of the picture, with the stage of the gesture and of
// the inertia that macOS adds after the fingers have left. `dx` and `dy` keep the values of the
// old form (the trackpad's points, 15 for a line of the wheel), for a receiver that knows
// nothing else.
//
// When the fingers leave, the end of the gesture is held back for up to SCROLL_MOMENTUM_WAIT_MS:
// if macOS begins its inertia within that time, the end goes with "momentum_follows", otherwise
// without (scroll_due). The wait is here, next to the system, so that no jitter of the network
// decides whether an application starts an inertia of its own. Movement is never held back.
#ifndef ISTESSO_SCROLL_H
#define ISTESSO_SCROLL_H

#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// The stages as NSEvent's `phase` and `momentumPhase` give them (NSEventPhase).
enum {
    SCROLL_PHASE_NONE = 0,
    SCROLL_PHASE_BEGAN = 1,
    SCROLL_PHASE_STATIONARY = 2,
    SCROLL_PHASE_CHANGED = 4,
    SCROLL_PHASE_ENDED = 8,
    SCROLL_PHASE_CANCELLED = 16,
    SCROLL_PHASE_MAY_BEGIN = 32,
};

#define SCROLL_MOMENTUM_WAIT_MS 60.0

typedef struct {
    double rest[2];  // parts of a pixel of `dx` and `dy` not yet passed on
    double notch[2]; // parts of a 120th of a notch not yet passed on (a wheel without whole lines)
    bool held;       // the end of a gesture is held back, waiting for inertia
    double held_at;  // since when (milliseconds)
} ScrollState;

typedef struct {
    bool precise;            // hasPreciseScrollingDeltas: a trackpad or a touch surface
    double dx, dy;           // scrollingDeltaX, scrollingDeltaY: how far the content moves
    bool has_lines;          // lines_x and lines_y are given (a wheel's CGEvent)
    long lines_x, lines_y;   // kCGScrollWheelEventDeltaAxis2 and 1: whole lines, as the content moves
    unsigned phase;          // phase
    unsigned momentum;       // momentumPhase
    double scale_x, scale_y; // the picture's pixels to a point of the view, across and down
} ScrollIn;

// Where the datagrams go.
typedef void (*ScrollSend)(void *context, const char *message);

static inline const char *scroll_stage(unsigned phase, bool momentum) {
    if (phase & SCROLL_PHASE_BEGAN) return "began";
    if (phase & (SCROLL_PHASE_CHANGED | SCROLL_PHASE_STATIONARY)) return "changed";
    if (phase & SCROLL_PHASE_ENDED) return "ended";
    // The system took the gesture back; inertia only ever ends.
    if (phase & SCROLL_PHASE_CANCELLED) return momentum ? "ended" : "cancelled";
    return NULL; // none, or "may begin" (fingers down, nothing moved yet): not a stage that is sent
}

// Takes the whole numbers out of an accumulated amount, and leaves the rest.
static inline int scroll_take(double *rest) {
    int whole = (int)*rest;
    *rest -= whole;
    return whole;
}

// Thousandths of a pixel, written by hand: no locale can make a comma of the point.
static inline void scroll_number(char *out, size_t size, double value) {
    long long m = llround(value * 1000.0);
    snprintf(out, size, "%s%lld.%03lld", m < 0 ? "-" : "", llabs(m) / 1000, llabs(m) % 1000);
}

static inline void scroll_finger(ScrollSend send, void *context, int dx, int dy, double px, double py, const char *phase,
                          const char *momentum, bool follows) {
    char fx[32], fy[32], out[256];
    scroll_number(fx, sizeof fx, px);
    scroll_number(fy, sizeof fy, py);
    int n = snprintf(out, sizeof out, "{\"kind\":\"scroll\",\"dx\":%d,\"dy\":%d,\"source\":\"finger\",\"pixels\":[%s,%s]%s%s%s%s%s%s%s}",
                     dx, dy, fx, fy,
                     phase ? ",\"phase\":\"" : "", phase ? phase : "", phase ? "\"" : "",
                     momentum ? ",\"momentum\":\"" : "", momentum ? momentum : "", momentum ? "\"" : "",
                     follows ? ",\"momentum_follows\":true" : "");
    if (n > 0 && (size_t)n < sizeof out) send(context, out);
}

// Sends the end of the gesture that is held back, if one is.
static inline void scroll_let_go(ScrollState *s, bool follows, ScrollSend send, void *context) {
    if (!s->held) return;
    s->held = false;
    scroll_finger(send, context, 0, 0, 0, 0, "ended", NULL, follows);
}

// To be called again once SCROLL_MOMENTUM_WAIT_MS have passed after an event that left an end
// held back (s->held): sends it, without inertia, when its wait is over. Says whether one is
// still held.
static inline bool scroll_due(ScrollState *s, double now_ms, ScrollSend send, void *context) {
    if (s->held && now_ms - s->held_at >= SCROLL_MOMENTUM_WAIT_MS) scroll_let_go(s, false, send, context);
    return s->held;
}

// What is sent for one event of macOS, at now_ms: none, one or two datagrams.
static inline void scroll_event(ScrollState *s, const ScrollIn *in, double now_ms, ScrollSend send, void *context) {
    const char *phase = in->precise ? scroll_stage(in->phase, false) : NULL;
    const char *momentum = in->precise && phase == NULL ? scroll_stage(in->momentum, true) : NULL;
    if (s->held) {
        // The inertia begins: the end goes first, saying so. Anything else: the end goes as it is.
        scroll_let_go(s, momentum != NULL && strcmp(momentum, "began") == 0, send, context);
    }
    // macOS says how far the content moves; the other side wants how far to scroll.
    double old_scale = in->precise ? 1.0 : 15.0;
    s->rest[0] -= in->dx * old_scale;
    s->rest[1] -= in->dy * old_scale;
    int dx = scroll_take(&s->rest[0]), dy = scroll_take(&s->rest[1]);
    char out[256];
    if (!in->precise) {
        int nx, ny;
        if (in->has_lines) {
            nx = (int)(-in->lines_x * 120);
            ny = (int)(-in->lines_y * 120);
        } else {
            s->notch[0] -= in->dx * 120.0;
            s->notch[1] -= in->dy * 120.0;
            nx = scroll_take(&s->notch[0]);
            ny = scroll_take(&s->notch[1]);
        }
        if (nx == 0 && ny == 0) return;
        int n = snprintf(out, sizeof out, "{\"kind\":\"scroll\",\"dx\":%d,\"dy\":%d,\"source\":\"wheel\",\"v120\":[%d,%d]}",
                         dx, dy, nx, ny);
        if (n > 0 && (size_t)n < sizeof out) send(context, out);
        return;
    }
    double px = -in->dx * (in->scale_x > 0 ? in->scale_x : 1.0);
    double py = -in->dy * (in->scale_y > 0 ? in->scale_y : 1.0);
    if (!isfinite(px) || !isfinite(py)) return;
    bool moved = px != 0.0 || py != 0.0;
    if (phase != NULL && strcmp(phase, "ended") == 0) {
        // What moved goes now; the end waits for the inertia.
        if (moved || dx != 0 || dy != 0) scroll_finger(send, context, dx, dy, px, py, "changed", NULL, false);
        s->held = true;
        s->held_at = now_ms;
        return;
    }
    // A stage on its own is worth sending when it begins or ends something.
    bool edge = (phase != NULL && strcmp(phase, "changed") != 0) || (momentum != NULL && strcmp(momentum, "changed") != 0);
    if (!moved && !edge) return;
    scroll_finger(send, context, dx, dy, px, py, phase, momentum, false);
}

// An event's datagrams with something done once before the first of them, and only when there is
// one: the window brings the modifiers over there in line with the event (and lends them when it
// is not the one typed into) around a scroll that is sent, never around an event that sends
// nothing (a gesture that may begin, a stationary one, an end held back, less than a line).
typedef struct {
    ScrollSend send;              // where the datagrams go
    void *context;                // given to `send` and to `first`
    void (*first)(void *context); // before the first datagram of the event, once
    bool sent;                    // a datagram of the event went
} ScrollAround;

static inline void scroll_around_send(void *around, const char *message) {
    ScrollAround *a = (ScrollAround *)around;
    if (!a->sent) {
        a->sent = true;
        if (a->first) a->first(a->context);
    }
    a->send(a->context, message);
}

// scroll_event, with `a->first` before its first datagram. Says whether one went.
static inline bool scroll_event_around(ScrollState *s, const ScrollIn *in, double now_ms, ScrollAround *a) {
    a->sent = false;
    scroll_event(s, in, now_ms, scroll_around_send, a);
    return a->sent;
}

#endif

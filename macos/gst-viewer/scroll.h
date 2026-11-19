// What the trial viewer sends for one scroll event of macOS (docs/adr/0007-control-channel.md,
// "スクロールの種類と段階"). Plain C, so that it is tested without a window (scroll-test.c).
//
// A wheel (no precise deltas) goes as notches, 120 to a notch; fingers on a trackpad (precise
// deltas) go as pixels of the picture, with the stage of the gesture and of the inertia that
// macOS adds after the fingers have left. `dx` and `dy` keep the values of the old form (the
// trackpad's points, 15 for a line of the wheel), for a receiver that knows nothing else.
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

typedef struct {
    double rest[2];  // parts of a pixel of `dx` and `dy` not yet passed on
    double notch[2]; // parts of a 120th of a notch not yet passed on
} ScrollState;

typedef struct {
    bool precise;          // hasPreciseScrollingDeltas: a trackpad or a touch surface
    double dx, dy;         // scrollingDeltaX, scrollingDeltaY: how far the content moves
    unsigned phase;        // phase
    unsigned momentum;     // momentumPhase
    double scale_x, scale_y; // the picture's pixels to a point of the view, across and down
} ScrollIn;

static const char *scroll_stage(unsigned phase, bool momentum) {
    if (phase & SCROLL_PHASE_BEGAN) return "began";
    if (phase & (SCROLL_PHASE_CHANGED | SCROLL_PHASE_STATIONARY)) return "changed";
    if (phase & SCROLL_PHASE_ENDED) return "ended";
    // The system took the gesture back; inertia only ever ends.
    if (phase & SCROLL_PHASE_CANCELLED) return momentum ? "ended" : "cancelled";
    return NULL; // none, or "may begin" (fingers down, nothing moved yet): not a stage that is sent
}

// Takes the whole numbers out of an accumulated amount, and leaves the rest.
static int scroll_take(double *rest) {
    int whole = (int)*rest;
    *rest -= whole;
    return whole;
}

// The datagram for one event, written into `out`; its length, or 0 when nothing is to be sent.
static size_t scroll_message(ScrollState *s, const ScrollIn *in, char *out, size_t size) {
    // macOS says how far the content moves; the other side wants how far to scroll.
    double old_scale = in->precise ? 1.0 : 15.0;
    s->rest[0] -= in->dx * old_scale;
    s->rest[1] -= in->dy * old_scale;
    int dx = scroll_take(&s->rest[0]), dy = scroll_take(&s->rest[1]);
    int n;
    if (!in->precise) {
        // A wheel: lines, as macOS counts them (it speeds them up when the wheel turns fast).
        s->notch[0] -= in->dx * 120.0;
        s->notch[1] -= in->dy * 120.0;
        int nx = scroll_take(&s->notch[0]), ny = scroll_take(&s->notch[1]);
        if (nx == 0 && ny == 0) return 0;
        n = snprintf(out, size, "{\"kind\":\"scroll\",\"dx\":%d,\"dy\":%d,\"source\":\"wheel\",\"v120\":[%d,%d]}",
                     dx, dy, nx, ny);
    } else {
        double px = -in->dx * (in->scale_x > 0 ? in->scale_x : 1.0);
        double py = -in->dy * (in->scale_y > 0 ? in->scale_y : 1.0);
        if (!isfinite(px) || !isfinite(py)) return 0;
        const char *phase = scroll_stage(in->phase, false);
        const char *momentum = phase == NULL ? scroll_stage(in->momentum, true) : NULL;
        bool moved = px != 0.0 || py != 0.0;
        // A stage on its own is worth sending when it begins or ends something.
        bool edge = (phase != NULL && strcmp(phase, "changed") != 0) || (momentum != NULL && strcmp(momentum, "changed") != 0);
        if (!moved && !edge) return 0;
        // Thousandths of a pixel, written by hand: no locale can make a comma of the point.
        long long mx = llround(px * 1000.0), my = llround(py * 1000.0);
        char fx[32], fy[32];
        snprintf(fx, sizeof fx, "%s%lld.%03lld", mx < 0 ? "-" : "", llabs(mx) / 1000, llabs(mx) % 1000);
        snprintf(fy, sizeof fy, "%s%lld.%03lld", my < 0 ? "-" : "", llabs(my) / 1000, llabs(my) % 1000);
        n = snprintf(out, size, "{\"kind\":\"scroll\",\"dx\":%d,\"dy\":%d,\"source\":\"finger\",\"pixels\":[%s,%s]%s%s%s%s%s%s}",
                     dx, dy, fx, fy,
                     phase ? ",\"phase\":\"" : "", phase ? phase : "", phase ? "\"" : "",
                     momentum ? ",\"momentum\":\"" : "", momentum ? momentum : "", momentum ? "\"" : "");
    }
    return n > 0 && (size_t)n < size ? (size_t)n : 0;
}

#endif

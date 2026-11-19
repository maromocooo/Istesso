// Tests of scroll.h: cc -Wall -o build/scroll-test scroll-test.c -lm && build/scroll-test
#include <locale.h>
#include <stdio.h>
#include <string.h>

#include "scroll.h"

static int failed = 0;

static void expect(ScrollState *s, ScrollIn in, const char *want, int line) {
    char out[512];
    size_t n = scroll_message(s, &in, out, sizeof out);
    const char *got = n ? out : "";
    if (strcmp(got, want) != 0) {
        fprintf(stderr, "scroll: failed (line %d)\n  got  %s\n  want %s\n", line, got, want);
        failed = 1;
    }
}
#define EXPECT(state, in, want) expect(state, in, want, __LINE__)

static ScrollIn finger(double dy, unsigned phase, unsigned momentum, double scale) {
    return (ScrollIn){.precise = true, .dy = dy, .phase = phase, .momentum = momentum, .scale_x = scale, .scale_y = scale};
}

int main(void) {
    // Whatever the locale, the point stays a point.
    setlocale(LC_ALL, "de_DE.UTF-8");

    // A wheel, one notch down (macOS: the content moves up by a line): 120, and 15 in the old form.
    ScrollState s = {0};
    EXPECT(&s, ((ScrollIn){.dy = -1}), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":15,\"source\":\"wheel\",\"v120\":[0,120]}");
    // Three lines at once (macOS sped the wheel up), to the left.
    EXPECT(&s, ((ScrollIn){.dx = 3}), "{\"kind\":\"scroll\",\"dx\":-45,\"dy\":0,\"source\":\"wheel\",\"v120\":[-360,0]}");
    // A tenth of a line, twice: parts of a 120th (and of a pixel of the old form) are kept for later.
    EXPECT(&s, ((ScrollIn){.dy = -0.104}), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":1,\"source\":\"wheel\",\"v120\":[0,12]}");
    EXPECT(&s, ((ScrollIn){.dy = -0.104}), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":2,\"source\":\"wheel\",\"v120\":[0,12]}");
    // What was kept adds up to a 120th.
    EXPECT(&s, ((ScrollIn){.dy = -0.0005}), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"wheel\",\"v120\":[0,1]}");
    // Less than a 120th: nothing yet.
    EXPECT(&s, ((ScrollIn){.dy = -0.0005}), "");

    // Two fingers, in a window that shows two pixels of the picture to a point.
    s = (ScrollState){0};
    // Fingers down, nothing moved yet: nothing.
    EXPECT(&s, finger(0, SCROLL_PHASE_MAY_BEGIN, 0, 2), "");
    EXPECT(&s, finger(0, SCROLL_PHASE_BEGAN, 0, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,0.000],\"phase\":\"began\"}");
    // 0.5 point up (the content moves up): 1 pixel down; the old form keeps the half point.
    EXPECT(&s, finger(-0.5, SCROLL_PHASE_CHANGED, 0, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,1.000],\"phase\":\"changed\"}");
    EXPECT(&s, finger(-0.75, SCROLL_PHASE_CHANGED, 0, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":1,\"source\":\"finger\",\"pixels\":[0.000,1.500],\"phase\":\"changed\"}");
    // Not moved, not an edge: nothing.
    EXPECT(&s, finger(0, SCROLL_PHASE_STATIONARY, 0, 2), "");
    EXPECT(&s, finger(0, SCROLL_PHASE_ENDED, 0, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,0.000],\"phase\":\"ended\"}");
    // The inertia macOS goes on with.
    EXPECT(&s, finger(-12.25, 0, SCROLL_PHASE_BEGAN, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":12,\"source\":\"finger\",\"pixels\":[0.000,24.500],\"momentum\":\"began\"}");
    EXPECT(&s, finger(-3, 0, SCROLL_PHASE_CHANGED, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":3,\"source\":\"finger\",\"pixels\":[0.000,6.000],\"momentum\":\"changed\"}");
    EXPECT(&s, finger(0, 0, SCROLL_PHASE_CHANGED, 2), "");
    EXPECT(&s, finger(0, 0, SCROLL_PHASE_ENDED, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,0.000],\"momentum\":\"ended\"}");
    // Inertia taken back by the system ends it.
    EXPECT(&s, finger(0, 0, SCROLL_PHASE_CANCELLED, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,0.000],\"momentum\":\"ended\"}");
    EXPECT(&s, finger(0, SCROLL_PHASE_CANCELLED, 0, 2), "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[0.000,0.000],\"phase\":\"cancelled\"}");
    // A precise device that says no stage (an old mouse with a smooth wheel): pixels only; to the
    // right and up, negative numbers keep their sign below one, and less than a thousandth is 0.
    s = (ScrollState){0};
    EXPECT(&s, ((ScrollIn){.precise = true, .dx = 0.25, .dy = 0.0004, .scale_x = 1, .scale_y = 1}),
           "{\"kind\":\"scroll\",\"dx\":0,\"dy\":0,\"source\":\"finger\",\"pixels\":[-0.250,0.000]}");
    // An unknown scale counts as one pixel a point.
    EXPECT(&s, ((ScrollIn){.precise = true, .dy = 2, .phase = SCROLL_PHASE_CHANGED}),
           "{\"kind\":\"scroll\",\"dx\":0,\"dy\":-2,\"source\":\"finger\",\"pixels\":[0.000,-2.000],\"phase\":\"changed\"}");

    if (!failed) printf("scroll: all passed\n");
    return failed;
}

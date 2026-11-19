// Tests of scroll.h: cc -Wall -o build/scroll-test scroll-test.c -lm && build/scroll-test
#include <locale.h>
#include <stdio.h>
#include <string.h>

#include "scroll.h"

static int failed = 0;
static char got[2048];

// The datagrams, one after the other, with "|" between them.
static void collect(void *context, const char *message) {
    (void)context;
    if (got[0]) strncat(got, "|", sizeof got - strlen(got) - 1);
    strncat(got, message, sizeof got - strlen(got) - 1);
}

static void check(const char *want, int line) {
    if (strcmp(got, want) != 0) {
        fprintf(stderr, "scroll: failed (line %d)\n  got  %s\n  want %s\n", line, got, want);
        failed = 1;
    }
    got[0] = 0;
}
static void run(ScrollState *state, ScrollIn in, double now) { scroll_event(state, &in, now, collect, NULL); }
#define EXPECT(state, in, now, want) (run(state, in, now), check(want, __LINE__))
#define EXPECT_DUE(state, now, want) (scroll_due(state, now, collect, NULL), check(want, __LINE__))

// With something done before the first datagram (the window's modifiers): "<first>".
static void mark_first(void *context) {
    (void)context;
    collect(NULL, "<first>");
}
static bool sent_around(ScrollState *state, ScrollIn in, double now) {
    ScrollAround a = {collect, NULL, mark_first, false};
    return scroll_event_around(state, &in, now, &a);
}
#define EXPECT_AROUND(state, in, now, sends, want) \
    ((sent_around(state, in, now) != (sends) ? (fprintf(stderr, "scroll: failed (line %d): sent\n", __LINE__), failed = 1) : 0), \
     check(want, __LINE__))

#define FINGER(dy_, phase_, momentum_) ((ScrollIn){.precise = true, .dy = dy_, .phase = phase_, .momentum = momentum_, .scale_x = 2, .scale_y = 2})
#define WHEEL(dx_, dy_, lx, ly) ((ScrollIn){.dx = dx_, .dy = dy_, .has_lines = true, .lines_x = lx, .lines_y = ly})

#define F(dx, dy, px, py, rest) "{\"kind\":\"scroll\",\"dx\":" #dx ",\"dy\":" #dy ",\"source\":\"finger\",\"pixels\":[" px "," py "]" rest "}"

int main(void) {
    // Whatever the locale, the point stays a point.
    setlocale(LC_ALL, "de_DE.UTF-8");

    // A wheel, one click down (macOS: the content moves up by a line): 120, and 15 in the old form.
    ScrollState s = {0};
    EXPECT(&s, WHEEL(0, -1, 0, -1), 0, "{\"kind\":\"scroll\",\"dx\":0,\"dy\":15,\"source\":\"wheel\",\"v120\":[0,120]}");
    // A slow click that macOS gives as a tenth of a line is still one notch (its whole line).
    EXPECT(&s, WHEEL(0, -0.1, 0, -1), 0, "{\"kind\":\"scroll\",\"dx\":0,\"dy\":1,\"source\":\"wheel\",\"v120\":[0,120]}");
    // Three lines at once, to the left.
    EXPECT(&s, WHEEL(3, 0, 3, 0), 0, "{\"kind\":\"scroll\",\"dx\":-45,\"dy\":0,\"source\":\"wheel\",\"v120\":[-360,0]}");
    // No whole line: nothing (the old form keeps its parts of a pixel for later).
    EXPECT(&s, WHEEL(0, -0.01, 0, 0), 0, "");
    // A wheel without whole lines given (another caller): the fractional lines, kept up.
    s = (ScrollState){0};
    EXPECT(&s, ((ScrollIn){.dy = -0.104}), 0, "{\"kind\":\"scroll\",\"dx\":0,\"dy\":1,\"source\":\"wheel\",\"v120\":[0,12]}");
    EXPECT(&s, ((ScrollIn){.dy = -0.104}), 0, "{\"kind\":\"scroll\",\"dx\":0,\"dy\":2,\"source\":\"wheel\",\"v120\":[0,12]}");

    // Two fingers, in a window that shows two pixels of the picture to a point; then inertia.
    s = (ScrollState){0};
    EXPECT(&s, FINGER(0, SCROLL_PHASE_MAY_BEGIN, 0), 0, "");
    EXPECT(&s, FINGER(0, SCROLL_PHASE_BEGAN, 0), 0, F(0, 0, "0.000", "0.000", ",\"phase\":\"began\""));
    // 0.5 point up (the content moves up): 1 pixel down; the old form keeps the half point.
    EXPECT(&s, FINGER(-0.5, SCROLL_PHASE_CHANGED, 0), 8, F(0, 0, "0.000", "1.000", ",\"phase\":\"changed\""));
    EXPECT(&s, FINGER(-0.75, SCROLL_PHASE_CHANGED, 0), 16, F(0, 1, "0.000", "1.500", ",\"phase\":\"changed\""));
    EXPECT(&s, FINGER(0, SCROLL_PHASE_STATIONARY, 0), 24, "");
    // The fingers leave: the end is held back.
    EXPECT(&s, FINGER(0, SCROLL_PHASE_ENDED, 0), 30, "");
    EXPECT_DUE(&s, 50, "");
    // The inertia begins within the wait: the end goes first, saying so.
    EXPECT(&s, FINGER(-12.25, 0, SCROLL_PHASE_BEGAN), 60,
           F(0, 0, "0.000", "0.000", ",\"phase\":\"ended\",\"momentum_follows\":true") "|"
           F(0, 12, "0.000", "24.500", ",\"momentum\":\"began\""));
    EXPECT_DUE(&s, 200, "");
    EXPECT(&s, FINGER(-3, 0, SCROLL_PHASE_CHANGED), 68, F(0, 3, "0.000", "6.000", ",\"momentum\":\"changed\""));
    EXPECT(&s, FINGER(0, 0, SCROLL_PHASE_CHANGED), 76, "");
    EXPECT(&s, FINGER(0, 0, SCROLL_PHASE_ENDED), 84, F(0, 0, "0.000", "0.000", ",\"momentum\":\"ended\""));
    // Inertia taken back by the system ends it.
    EXPECT(&s, FINGER(0, 0, SCROLL_PHASE_CANCELLED), 90, F(0, 0, "0.000", "0.000", ",\"momentum\":\"ended\""));

    // No inertia: the end goes when the wait is over, and only once.
    s = (ScrollState){0};
    EXPECT(&s, FINGER(-2, SCROLL_PHASE_CHANGED, 0), 0, F(0, 2, "0.000", "4.000", ",\"phase\":\"changed\""));
    // An end that moved: what moved goes at once, as a change.
    EXPECT(&s, FINGER(-1, SCROLL_PHASE_ENDED, 0), 10, F(0, 1, "0.000", "2.000", ",\"phase\":\"changed\""));
    EXPECT_DUE(&s, 69, "");
    EXPECT_DUE(&s, 70, F(0, 0, "0.000", "0.000", ",\"phase\":\"ended\""));
    EXPECT_DUE(&s, 200, "");
    // An end held back, and the next gesture (or a wheel) comes first: the end goes as it is.
    EXPECT(&s, FINGER(0, SCROLL_PHASE_ENDED, 0), 300, "");
    EXPECT(&s, FINGER(0, SCROLL_PHASE_BEGAN, 0), 320,
           F(0, 0, "0.000", "0.000", ",\"phase\":\"ended\"") "|" F(0, 0, "0.000", "0.000", ",\"phase\":\"began\""));
    EXPECT(&s, FINGER(0, SCROLL_PHASE_CANCELLED, 0), 330, F(0, 0, "0.000", "0.000", ",\"phase\":\"cancelled\""));

    // A precise device that says no stage: pixels only; negative numbers keep their sign below
    // one, and less than a thousandth is 0.
    s = (ScrollState){0};
    EXPECT(&s, ((ScrollIn){.precise = true, .dx = 0.25, .dy = 0.0004, .scale_x = 1, .scale_y = 1}), 0,
           F(0, 0, "-0.250", "0.000", ""));
    // An unknown scale counts as one pixel a point.
    EXPECT(&s, ((ScrollIn){.precise = true, .dy = 2, .phase = SCROLL_PHASE_CHANGED}), 0,
           F(0, -2, "0.000", "-2.000", ",\"phase\":\"changed\""));

    // The modifiers go only around an event that sends: never for one that sends nothing, once
    // before the first datagram of one that sends two.
    s = (ScrollState){0};
    EXPECT_AROUND(&s, FINGER(0, SCROLL_PHASE_MAY_BEGIN, 0), 0, false, "");
    EXPECT_AROUND(&s, FINGER(-1, SCROLL_PHASE_CHANGED, 0), 8, true, "<first>|" F(0, 1, "0.000", "2.000", ",\"phase\":\"changed\""));
    EXPECT_AROUND(&s, FINGER(0, SCROLL_PHASE_STATIONARY, 0), 16, false, "");
    EXPECT_AROUND(&s, FINGER(0, SCROLL_PHASE_ENDED, 0), 24, false, "");
    EXPECT_AROUND(&s, FINGER(0, SCROLL_PHASE_BEGAN, 0), 40, true,
                  "<first>|" F(0, 0, "0.000", "0.000", ",\"phase\":\"ended\"") "|" F(0, 0, "0.000", "0.000", ",\"phase\":\"began\""));
    EXPECT_AROUND(&s, WHEEL(0, -0.01, 0, 0), 50, false, "");
    EXPECT_AROUND(&s, WHEEL(0, -1, 0, -1), 60, true, "<first>|{\"kind\":\"scroll\",\"dx\":0,\"dy\":15,\"source\":\"wheel\",\"v120\":[0,120]}");

    if (!failed) printf("scroll: all passed\n");
    return failed;
}

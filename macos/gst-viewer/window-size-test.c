// Tests of window-size.h: cc -Wall -o build/window-size-test window-size-test.c && build/window-size-test
#include <stdio.h>
#include <stdlib.h>

#include "window-size.h"

static int failed = 0;

#define CHECK(what)                                                    \
    do {                                                               \
        if (!(what)) {                                                 \
            fprintf(stderr, "window-size: failed: %s (line %d)\n", #what, __LINE__); \
            failed = 1;                                                \
        }                                                              \
    } while (0)

int main(void) {
    unsigned w, h;
    // A window resized by hand to 1000x600 points, one point a pixel: it asks for 1000x600.
    picture_size_for_window(1000, 600, 1, &w, &h);
    CHECK(w == 1000 && h == 600);
    // At two points a pixel, half as many pixels.
    picture_size_for_window(1000, 600, 2, &w, &h);
    CHECK(w == 500 && h == 300);
    // Never less than 64 or more than 8192 a side.
    picture_size_for_window(20, 9000, 1, &w, &h);
    CHECK(w == 64 && h == 8192);
    // An unknown scale counts as one point a pixel.
    picture_size_for_window(800, 450, 0, &w, &h);
    CHECK(w == 800 && h == 450);

    double x, y;
    // The answer of the host, 960x540, fits: one point a pixel.
    window_size_for_picture(960, 540, 1, 1700, 1000, &x, &y);
    CHECK(x == 960 && y == 540);
    // 1920x1080 does not fit into 1710x961: smaller as a whole, the pixels square.
    window_size_for_picture(1920, 1080, 1, 1710, 961, &x, &y);
    CHECK(x == 1708 && y == 961);
    CHECK(fabs(x / y - 1920.0 / 1080.0) < 0.002);
    // What the window asks for and what it is then given, when the host takes the size as asked:
    // the same, so nothing is asked again (no going back and forth).
    picture_size_for_window(1234, 567, 1, &w, &h);
    window_size_for_picture(w, h, 1, 4000, 4000, &x, &y);
    CHECK(x == 1234 && y == 567);

    if (failed) {
        return 1;
    }
    printf("window size: all tests passed\n");
    return 0;
}

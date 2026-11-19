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

// A window as the viewer has it: its picture area in points, and the screen it is on (backing scale
// in pixels a point; the points the screen has for the picture area). The Air's built-in screen
// looks like 1470x956 points at 2x; the LG monitor is 3440x1440 at 1x. Less the menu bar and the
// window's title bar.
typedef struct {
    double width, height, backing, room_width, room_height;
} Window;
static const Window BUILT_IN = {0, 0, 2, 1470, 900};
static const Window LG = {0, 0, 1, 3440, 1388};

static Window opened(double width, double height, Window screen) {
    screen.width = width;
    screen.height = height;
    return screen;
}

// macOS keeps the size in points of a window dragged onto another screen.
static void drag(Window *window, Window screen) {
    window->backing = screen.backing;
    window->room_width = screen.room_width;
    window->room_height = screen.room_height;
}

// A stream of this size arrives; returns whether the window changed.
static int picture(WindowSizing *s, Window *window, unsigned width, unsigned height) {
    double x, y;
    if (!window_sizing_picture(s, width, height, window->width, window->height, window->room_width, window->room_height, &x, &y)) {
        return 0;
    }
    window->width = x;
    window->height = y;
    return 1;
}

// The person resizes the window by hand; the size asked of the host.
static void hand(WindowSizing *s, Window *window, double width, double height, unsigned *asked_width, unsigned *asked_height) {
    window->width = width;
    window->height = height;
    window_sizing_hand_resized(s, width, height, asked_width, asked_height);
}

static void scenarios(void) {
    unsigned w, h;

    // A Mac host's application (2 pixels a point on its virtual display), its window opened on the
    // Air's built-in screen (2x) at the size the launcher guessed, dragged to the LG (1x), made
    // smaller there: the owner's case (2026-11-19, Dia).
    WindowSizing s = window_sizing_start(2);
    Window window = opened(1720, 970, BUILT_IN);
    CHECK(picture(&s, &window, 3420, 2150) == 1);
    // 1710x1075 points, made smaller as a whole for the built-in screen.
    CHECK(window.width == 1431 && window.height == 900);
    CHECK(screen_pixels_per_picture_pixel(&s, window.backing) == 1);
    drag(&window, LG);
    // Same points, same points a picture pixel: half a pixel of the LG for a picture pixel (not
    // pixel for pixel, which made everything twice as large).
    CHECK(window.width == 1431 && window.height == 900 && s.each == 0.5);
    CHECK(screen_pixels_per_picture_pixel(&s, window.backing) == 0.5);
    hand(&s, &window, 1000, 600, &w, &h);
    CHECK(w == 2000 && h == 1200);
    // The host's answer: the application's window at 1000x600 points, 2000x1200 pixels. The window
    // goes to 1000x600 points: the application as large as on the host (no jump to twice the size).
    CHECK(picture(&s, &window, 2000, 1200) == 1);
    CHECK(window.width == 1000 && window.height == 600);
    CHECK(window.width * 2 == 2000);
    // The same size again (a connection made again): nothing changes, nothing is asked.
    CHECK(picture(&s, &window, 2000, 1200) == 0 && s.resize_asked == 0);

    // The other way: opened on the LG (1x), dragged to the built-in screen (2x), made smaller.
    s = window_sizing_start(2);
    window = opened(1720, 970, LG);
    CHECK(picture(&s, &window, 3420, 2150) == 1);
    CHECK(window.width == 1710 && window.height == 1075);
    drag(&window, BUILT_IN);
    CHECK(window.width == 1710 && s.each == 0.5);
    CHECK(screen_pixels_per_picture_pixel(&s, window.backing) == 1);
    hand(&s, &window, 1200, 700, &w, &h);
    CHECK(w == 2400 && h == 1400);
    CHECK(picture(&s, &window, 2400, 1400) == 1);
    CHECK(window.width == 1200 && window.height == 700);

    // The first picture at 1x (the host's window still on the host's own screen), then at 2x on its
    // virtual display: the window follows each at half a point a pixel. The old rule took 0.90
    // points a pixel from the first picture (1720 / 1906) and kept it.
    s = window_sizing_start(2);
    window = opened(1720, 970, LG);
    CHECK(picture(&s, &window, 1906, 1074) == 1);
    CHECK(window.width == 953 && window.height == 537);
    CHECK(picture(&s, &window, 3420, 2150) == 1);
    CHECK(window.width == 1710 && window.height == 1075);
    hand(&s, &window, 1000, 563, &w, &h);
    CHECK(w == 2000 && h == 1126);
    CHECK(picture(&s, &window, 2000, 1126) == 1);
    CHECK(window.width == 1000 && window.height == 563);
    double old_x, old_y;
    window_size_for_picture(2000, 1128, 1720.0 / 1906, LG.room_width, LG.room_height, &old_x, &old_y);
    // 1804 points for an application 1000 points wide: about twice as large (the owner's log had
    // 1794x1012 for 2000x1128).
    CHECK(old_x == 1804);
    // Whatever size the window was opened at, it ends the same.
    WindowSizing other = window_sizing_start(2);
    Window small = opened(640, 432, LG);
    picture(&other, &small, 1906, 1074);
    picture(&other, &small, 3420, 2150);
    CHECK(small.width == 1710 && small.height == 1075);

    // A first picture that has the window's size at half a point a pixel leaves it as opened
    // (TextEdit, 640x432 points, its first picture already at 2x).
    s = window_sizing_start(2);
    window = opened(640, 432, BUILT_IN);
    CHECK(picture(&s, &window, 1280, 864) == 0);
    CHECK(window.width == 640 && window.height == 432);

    // An Ubuntu application (its compositor at 1, the launcher passes 1): the window opened at the
    // picture's size stays so, also wider than the built-in screen, as before; dragged either way,
    // nothing changes; resized by hand, it asks for its points in pixels.
    s = window_sizing_start(1);
    window = opened(1720, 720, BUILT_IN);
    CHECK(picture(&s, &window, 1720, 720) == 0);
    CHECK(window.width == 1720 && window.height == 720);
    drag(&window, LG);
    CHECK(window.width == 1720 && s.each == 1);
    CHECK(screen_pixels_per_picture_pixel(&s, window.backing) == 1);
    drag(&window, BUILT_IN);
    CHECK(screen_pixels_per_picture_pixel(&s, window.backing) == 2);
    hand(&s, &window, 1000, 500, &w, &h);
    CHECK(w == 1000 && h == 500);
}

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

    // The host's density, as the launcher says it.
    CHECK(picture_density_from(NULL, 2) == 2);
    CHECK(picture_density_from("1", 2) == 1);
    CHECK(picture_density_from("1.5", 2) == 1.5);
    CHECK(picture_density_from("0", 2) == 2);
    CHECK(picture_density_from("5", 2) == 2);
    CHECK(picture_density_from("2x", 1) == 1);
    CHECK(picture_density_from("", 1) == 1);

    scenarios();

    if (failed) {
        return 1;
    }
    printf("window size: all tests passed\n");
    return 0;
}

// A Wayland client for checks of the everyday trial's scrolling (docs/trial-daily.md, "スクロール"):
// one window of one colour, and a line on standard output for every scroll the compositor tells
// it, as the application would get it. Nothing else is read or written. Trial checks only.
//
//   scroll-log            (in the hidden compositor: WAYLAND_DISPLAY is that compositor's)
//
// Lines (JSON): {"t_ms":T,"event":"axis","axis":"v","value":V}, "axis_source" ("source": wheel,
// finger, continuous, wheel_tilt), "value120" ("value120": N), "stop", and "frame", which ends a
// group of them. T is the compositor's time of the event (milliseconds), where it has one.
//
// Built on the host with the system's libwayland-client (MIT) and the xdg-shell protocol (MIT):
//   wayland-scanner client-header xdg-shell.xml xdg-shell.h
//   wayland-scanner private-code xdg-shell.xml xdg-shell.c
//   cc -O2 -Wall -I. -o scroll-log scroll-log.c xdg-shell.c -lwayland-client
// (xdg-shell.xml is in the wayland-protocols package, or in the wayland-protocols crate's source.)
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>

#include "xdg-shell.h"

static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct wl_seat *seat;
static struct xdg_wm_base *wm_base;
static struct wl_surface *surface;
static struct xdg_surface *xdg_surface;
static struct xdg_toplevel *toplevel;
static struct wl_pointer *pointer;
static int width = 640, height = 480, configured = 0, closed = 0;

static const char *axis_name(uint32_t axis) { return axis == WL_POINTER_AXIS_HORIZONTAL_SCROLL ? "h" : "v"; }

static void say(const char *line) {
    fputs(line, stdout);
    fputc('\n', stdout);
    fflush(stdout);
}

static void pointer_enter(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)s; (void)x; (void)y;
    wl_pointer_set_cursor(p, serial, NULL, 0, 0);
    say("{\"event\":\"enter\"}");
}
static void pointer_leave(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s) {
    (void)d; (void)p; (void)serial; (void)s;
    say("{\"event\":\"leave\"}");
}
static void pointer_motion(void *d, struct wl_pointer *p, uint32_t t, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)p; (void)t; (void)x; (void)y;
}
static void pointer_button(void *d, struct wl_pointer *p, uint32_t serial, uint32_t t, uint32_t b, uint32_t state) {
    (void)d; (void)p; (void)serial; (void)t; (void)b; (void)state;
}
static void pointer_axis(void *d, struct wl_pointer *p, uint32_t t, uint32_t axis, wl_fixed_t value) {
    (void)d; (void)p;
    char line[160];
    snprintf(line, sizeof line, "{\"t_ms\":%u,\"event\":\"axis\",\"axis\":\"%s\",\"value\":%.4f}", t, axis_name(axis), wl_fixed_to_double(value));
    say(line);
}
static void pointer_frame(void *d, struct wl_pointer *p) {
    (void)d; (void)p;
    say("{\"event\":\"frame\"}");
}
static void pointer_axis_source(void *d, struct wl_pointer *p, uint32_t source) {
    (void)d; (void)p;
    const char *names[] = {"wheel", "finger", "continuous", "wheel_tilt"};
    char line[96];
    snprintf(line, sizeof line, "{\"event\":\"axis_source\",\"source\":\"%s\"}", source < 4 ? names[source] : "other");
    say(line);
}
static void pointer_axis_stop(void *d, struct wl_pointer *p, uint32_t t, uint32_t axis) {
    (void)d; (void)p;
    char line[96];
    snprintf(line, sizeof line, "{\"t_ms\":%u,\"event\":\"stop\",\"axis\":\"%s\"}", t, axis_name(axis));
    say(line);
}
static void pointer_axis_discrete(void *d, struct wl_pointer *p, uint32_t axis, int32_t discrete) {
    (void)d; (void)p; (void)axis; (void)discrete; // version 8 and later send value120 instead
}
static void pointer_axis_value120(void *d, struct wl_pointer *p, uint32_t axis, int32_t value120) {
    (void)d; (void)p;
    char line[96];
    snprintf(line, sizeof line, "{\"event\":\"value120\",\"axis\":\"%s\",\"value120\":%d}", axis_name(axis), value120);
    say(line);
}
static void pointer_axis_relative_direction(void *d, struct wl_pointer *p, uint32_t axis, uint32_t direction) {
    (void)d; (void)p; (void)axis; (void)direction;
}
static const struct wl_pointer_listener pointer_listener = {
    pointer_enter, pointer_leave, pointer_motion, pointer_button, pointer_axis, pointer_frame,
    pointer_axis_source, pointer_axis_stop, pointer_axis_discrete, pointer_axis_value120,
    pointer_axis_relative_direction,
};

static void seat_capabilities(void *d, struct wl_seat *s, uint32_t caps) {
    (void)d;
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && pointer == NULL) {
        pointer = wl_seat_get_pointer(s);
        wl_pointer_add_listener(pointer, &pointer_listener, NULL);
    }
}
static void seat_name(void *d, struct wl_seat *s, const char *name) { (void)d; (void)s; (void)name; }
static const struct wl_seat_listener seat_listener = {seat_capabilities, seat_name};

static void wm_base_ping(void *d, struct xdg_wm_base *b, uint32_t serial) { (void)d; xdg_wm_base_pong(b, serial); }
static const struct xdg_wm_base_listener wm_base_listener = {wm_base_ping};

static void global(void *d, struct wl_registry *r, uint32_t name, const char *interface, uint32_t version) {
    (void)d;
    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        compositor = wl_registry_bind(r, name, &wl_compositor_interface, 4);
    } else if (strcmp(interface, wl_shm_interface.name) == 0) {
        shm = wl_registry_bind(r, name, &wl_shm_interface, 1);
    } else if (strcmp(interface, wl_seat_interface.name) == 0) {
        // Version 9 at most (what this listener knows): value120 from 8 on.
        seat = wl_registry_bind(r, name, &wl_seat_interface, version < 9 ? version : 9);
        wl_seat_add_listener(seat, &seat_listener, NULL);
        char line[64];
        snprintf(line, sizeof line, "{\"event\":\"seat\",\"version\":%u}", version < 9 ? version : 9);
        say(line);
    } else if (strcmp(interface, xdg_wm_base_interface.name) == 0) {
        wm_base = wl_registry_bind(r, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wm_base, &wm_base_listener, NULL);
    }
}
static void global_remove(void *d, struct wl_registry *r, uint32_t name) { (void)d; (void)r; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

static struct wl_buffer *buffer_of_one_colour(int w, int h) {
    int stride = w * 4, size = stride * h;
    int fd = memfd_create("scroll-log", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, size) < 0) return NULL;
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) return NULL;
    for (int i = 0; i < w * h; i++) pixels[i] = 0xff3060a0;
    munmap(pixels, size);
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, size);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, w, h, stride, WL_SHM_FORMAT_XRGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);
    return buffer;
}

static void xdg_surface_configure(void *d, struct xdg_surface *s, uint32_t serial) {
    (void)d;
    xdg_surface_ack_configure(s, serial);
    struct wl_buffer *buffer = buffer_of_one_colour(width, height);
    if (buffer == NULL) {
        say("{\"event\":\"error\",\"what\":\"no buffer\"}");
        exit(1);
    }
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, width, height);
    wl_surface_commit(surface);
    if (!configured) say("{\"event\":\"shown\"}");
    configured = 1;
}
static const struct xdg_surface_listener xdg_surface_listener = {xdg_surface_configure};

static void toplevel_configure(void *d, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *states) {
    (void)d; (void)t; (void)states;
    if (w > 0 && h > 0) { width = w; height = h; }
}
static void toplevel_close(void *d, struct xdg_toplevel *t) { (void)d; (void)t; closed = 1; }
static void toplevel_bounds(void *d, struct xdg_toplevel *t, int32_t w, int32_t h) { (void)d; (void)t; (void)w; (void)h; }
static void toplevel_capabilities(void *d, struct xdg_toplevel *t, struct wl_array *c) { (void)d; (void)t; (void)c; }
static const struct xdg_toplevel_listener toplevel_listener = {toplevel_configure, toplevel_close, toplevel_bounds, toplevel_capabilities};

int main(void) {
    struct wl_display *display = wl_display_connect(NULL);
    if (display == NULL) {
        fprintf(stderr, "scroll-log: no Wayland display\n");
        return 1;
    }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    wl_display_roundtrip(display);
    if (compositor == NULL || shm == NULL || seat == NULL || wm_base == NULL) {
        fprintf(stderr, "scroll-log: the compositor lacks a global this needs\n");
        return 1;
    }
    surface = wl_compositor_create_surface(compositor);
    xdg_surface = xdg_wm_base_get_xdg_surface(wm_base, surface);
    xdg_surface_add_listener(xdg_surface, &xdg_surface_listener, NULL);
    toplevel = xdg_surface_get_toplevel(xdg_surface);
    xdg_toplevel_add_listener(toplevel, &toplevel_listener, NULL);
    xdg_toplevel_set_title(toplevel, "scroll-log");
    xdg_toplevel_set_app_id(toplevel, "scroll-log");
    wl_surface_commit(surface);
    while (!closed && wl_display_dispatch(display) != -1) {
    }
    say("{\"event\":\"closed\"}");
    wl_display_disconnect(display);
    return 0;
}

// The application's other windows, each in a window of its own (ADR 0027, a proposal; PROTOTYPE,
// trial only, and only with ISTESSO_WINDOWS=1). The host lists its application's windows (packet
// kind 6 of the picture's connection, JSON); this file reads that list and says what the viewer
// has to open, close and rename. Pure Foundation, tested by window-list-test.m without a window.
//
// The list, as the host writes it:
//   {"serial":N,"windows":[{"id":2,"port":P,"width":W,"height":H,"title":"...","modal":false,
//     "key":false,"attention":false}, ...]}
// `id` is the host's number of the window (2 and up; the first window is the connection's own
// picture and is never listed); `port` is where its picture is listened for, on the same host
// (0: no picture yet); `width` x `height` its picture in pixels. Titles are shown, never written
// anywhere.
#ifndef ISTESSO_WINDOW_LIST_H
#define ISTESSO_WINDOW_LIST_H

#import <Foundation/Foundation.h>
#include <string.h>

// Windows of one application this viewer shows at most: the first and three more (ADR 0027, 2.2).
enum { WINDOWS_MOST_EXTRA = 3, WINDOW_TITLE_MOST = 256, WINDOW_SIDE_MOST = 8192 };

@interface ListedWindow : NSObject
@property(nonatomic) uint32_t number;
@property(nonatomic) uint16_t port;
@property(nonatomic) int width, height;
@property(nonatomic, copy) NSString *title;
@property(nonatomic) BOOL modal, key, attention;
@end

@implementation ListedWindow
@end

// The listed windows this viewer can show, in the host's order: those with a picture to connect to
// (a port), a number of 2 or more, a size within 1 to 8192 pixels and a title of at most 256
// bytes; a number listed twice counts once (the first). At most WINDOWS_MOST_EXTRA. nil when the
// list cannot be read at all (then nothing is changed).
static NSArray<ListedWindow *> *windows_from_list(NSData *json) {
    if (json.length == 0 || json.length > 8192) {
        return nil;
    }
    id root = [NSJSONSerialization JSONObjectWithData:json options:0 error:nil];
    if (![root isKindOfClass:NSDictionary.class]) {
        return nil;
    }
    id windows = ((NSDictionary *)root)[@"windows"];
    if (![windows isKindOfClass:NSArray.class]) {
        return nil;
    }
    NSMutableArray<ListedWindow *> *out = [NSMutableArray array];
    NSMutableSet<NSNumber *> *seen = [NSMutableSet set];
    for (id entry in (NSArray *)windows) {
        if (out.count >= WINDOWS_MOST_EXTRA) {
            break;
        }
        if (![entry isKindOfClass:NSDictionary.class]) {
            continue;
        }
        NSDictionary *w = entry;
        NSNumber *number = w[@"id"], *port = w[@"port"], *width = w[@"width"], *height = w[@"height"];
        NSString *title = w[@"title"];
        if (![number isKindOfClass:NSNumber.class] || ![port isKindOfClass:NSNumber.class] || ![width isKindOfClass:NSNumber.class] ||
            ![height isKindOfClass:NSNumber.class]) {
            continue;
        }
        long long n = number.longLongValue, p = port.longLongValue, x = width.longLongValue, y = height.longLongValue;
        if (n < 2 || n > UINT32_MAX || p < 1 || p > 65535 || x < 1 || x > WINDOW_SIDE_MOST || y < 1 || y > WINDOW_SIDE_MOST) {
            continue;
        }
        if (title != nil && (![title isKindOfClass:NSString.class] ||
                             [title lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > WINDOW_TITLE_MOST)) {
            continue;
        }
        if ([seen containsObject:@(n)]) {
            continue;
        }
        [seen addObject:@(n)];
        ListedWindow *listed = [ListedWindow new];
        listed.number = (uint32_t)n;
        listed.port = (uint16_t)p;
        listed.width = (int)x;
        listed.height = (int)y;
        listed.title = title ?: @"";
        listed.modal = [w[@"modal"] isKindOfClass:NSNumber.class] && [w[@"modal"] boolValue];
        listed.key = [w[@"key"] isKindOfClass:NSNumber.class] && [w[@"key"] boolValue];
        listed.attention = [w[@"attention"] isKindOfClass:NSNumber.class] && [w[@"attention"] boolValue];
        [out addObject:listed];
    }
    return out;
}

// What changes between the windows shown now (their numbers and the port each one's picture comes
// from) and a new list: windows to open, numbers to close (gone from the list, or listed with
// another port: their picture is another stream now, opened again).
typedef struct {
    NSArray<ListedWindow *> *open;
    NSArray<NSNumber *> *close;
} WindowChanges;

static WindowChanges window_changes(NSDictionary<NSNumber *, NSNumber *> *shown_ports, NSArray<ListedWindow *> *listed) {
    NSMutableArray<ListedWindow *> *open = [NSMutableArray array];
    NSMutableArray<NSNumber *> *close = [NSMutableArray array];
    NSMutableDictionary<NSNumber *, NSNumber *> *wanted = [NSMutableDictionary dictionary];
    for (ListedWindow *w in listed) {
        wanted[@(w.number)] = @(w.port);
    }
    for (NSNumber *number in shown_ports) {
        NSNumber *port = wanted[number];
        if (port == nil || ![port isEqualToNumber:shown_ports[number]]) {
            [close addObject:number];
        }
    }
    for (ListedWindow *w in listed) {
        NSNumber *port = shown_ports[@(w.number)];
        if (port == nil || port.unsignedShortValue != w.port) {
            [open addObject:w];
        }
    }
    [close sortUsingSelector:@selector(compare:)];
    return (WindowChanges){open, close};
}

// The address of a window's picture: the host of ISTESSO_CHANNEL ("192.0.2.1:47911",
// "[fd00::1]:47911", "name:47911") with the listed port. false when the address has no port.
static bool window_address(const char *channel, uint16_t port, char *out, size_t room) {
    if (channel == NULL || port == 0) {
        return false;
    }
    const char *colon = strrchr(channel, ':');
    if (colon == NULL || colon == channel || colon[1] == '\0') {
        return false;
    }
    // An IPv6 address without brackets has its own colons: not taken.
    if (channel[0] != '[' && memchr(channel, ':', (size_t)(colon - channel)) != NULL) {
        return false;
    }
    int length = snprintf(out, room, "%.*s:%u", (int)(colon - channel), channel, (unsigned)port);
    return length > 0 && (size_t)length < room;
}

// The focus told again when the host is told that windows are shown one by one (a new connection
// of the relay or of the picture: the host may have lost which window keys go to). Only while
// this application is the active one: `key` is the number of its window typed into (1: the
// first), or 0 for none. false: nothing to tell.
static bool window_focus_line(bool active, uint32_t key, char *out, size_t room) {
    if (!active || key == 0) {
        return false;
    }
    int length = key == 1 ? snprintf(out, room, "{\"kind\":\"focus\"}") : snprintf(out, room, "{\"kind\":\"focus\",\"window\":%u}", key);
    return length > 0 && (size_t)length < room;
}

// Where a new window goes (the top left of its frame, in screen points from the bottom left, as
// AppKit counts): a little below and to the right of the first window for the first one, one step
// more for each one after it, and kept inside `room` (the visible part of the screen).
static NSPoint window_top_left(NSRect first, NSSize size, unsigned index, NSRect room) {
    double step = 28.0 * (index + 1);
    double x = NSMinX(first) + step, top = NSMaxY(first) - step;
    x = fmax(NSMinX(room), fmin(x, NSMaxX(room) - size.width));
    top = fmin(NSMaxY(room), fmax(top, NSMinY(room) + size.height));
    return NSMakePoint(x, top);
}

#endif

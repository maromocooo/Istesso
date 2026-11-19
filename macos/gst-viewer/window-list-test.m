// Windowless tests for window-list.h. Link with Foundation.
#include "window-list.h"
#include <stdio.h>

static int failures;

static void check(bool ok, const char *what) {
    if (!ok) {
        fprintf(stderr, "FAIL: %s\n", what);
        failures++;
    }
}

static NSArray<ListedWindow *> *list(NSString *json) {
    return windows_from_list([json dataUsingEncoding:NSUTF8StringEncoding]);
}

int main(void) {
    @autoreleasepool {
        // Reading the list.
        NSArray<ListedWindow *> *two = list(@"{\"serial\":3,\"windows\":[{\"id\":2,\"port\":50001,\"width\":800,\"height\":600,"
                                             "\"title\":\"Untitled 2\",\"modal\":false,\"key\":true,\"attention\":false,\"later\":1}]}");
        check(two.count == 1, "one window is read");
        check(two.firstObject.number == 2 && two.firstObject.port == 50001 && two.firstObject.width == 800 &&
                  two.firstObject.height == 600 && two.firstObject.key && !two.firstObject.modal,
              "its numbers are read; a field of a later version is passed over");
        check([two.firstObject.title isEqualToString:@"Untitled 2"], "its title is read");
        check(list(@"not json") == nil && list(@"[]") == nil && list(@"{\"windows\":3}") == nil, "a list that cannot be read is nil");
        check(list(@"{\"serial\":1,\"windows\":[]}").count == 0, "an empty list is empty, not nil");
        check(list(@"{\"windows\":[{\"id\":1,\"port\":5,\"width\":8,\"height\":8}]}").count == 0, "the first window is never listed");
        check(list(@"{\"windows\":[{\"id\":2,\"port\":0,\"width\":8,\"height\":8}]}").count == 0, "a window without a picture is not shown");
        check(list(@"{\"windows\":[{\"id\":2,\"port\":70000,\"width\":8,\"height\":8}]}").count == 0, "no port above 65535");
        check(list(@"{\"windows\":[{\"id\":2,\"port\":5,\"width\":9000,\"height\":8}]}").count == 0, "no side above 8192");
        check(list(@"{\"windows\":[{\"id\":2,\"port\":5,\"width\":8,\"height\":8,\"title\":7}]}").count == 0, "a title is text");
        NSString *long_title = [@"" stringByPaddingToLength:257 withString:@"a" startingAtIndex:0];
        check(list([NSString stringWithFormat:@"{\"windows\":[{\"id\":2,\"port\":5,\"width\":8,\"height\":8,\"title\":\"%@\"}]}", long_title])
                      .count == 0,
              "no title above 256 bytes");
        NSArray<ListedWindow *> *many = list(@"{\"windows\":[{\"id\":2,\"port\":5,\"width\":8,\"height\":8},{\"id\":2,\"port\":6,\"width\":8,\"height\":8},"
                                              "{\"id\":3,\"port\":7,\"width\":8,\"height\":8},{\"id\":4,\"port\":8,\"width\":8,\"height\":8},"
                                              "{\"id\":5,\"port\":9,\"width\":8,\"height\":8}]}");
        check(many.count == 3 && many[0].port == 5 && many[1].number == 3 && many[2].number == 4,
              "a number counts once; three windows at most besides the first");
        NSMutableData *big = [NSMutableData dataWithLength:8193];
        check(windows_from_list(big) == nil, "no list above 8 KiB");

        // What changes.
        WindowChanges first = window_changes(@{}, two);
        check(first.open.count == 1 && first.close.count == 0, "a new window is opened");
        WindowChanges same = window_changes(@{@2 : @50001}, two);
        check(same.open.count == 0 && same.close.count == 0, "the same list changes nothing");
        WindowChanges moved = window_changes(@{@2 : @50000}, two);
        check(moved.open.count == 1 && moved.close.count == 1 && moved.close[0].intValue == 2,
              "a window whose picture moved to another port is opened again");
        WindowChanges gone = window_changes(@{@2 : @50001, @3 : @50002}, list(@"{\"windows\":[]}"));
        check(gone.open.count == 0 && gone.close.count == 2 && gone.close[0].intValue == 2 && gone.close[1].intValue == 3,
              "windows gone from the list are closed, in order");

        // Where the picture of a window is.
        char out[128];
        check(window_address("192.0.2.1:47911", 50001, out, sizeof out) && strcmp(out, "192.0.2.1:50001") == 0, "IPv4");  // check-private: ok (documentation address, RFC 5737)
        check(window_address("[fd00::1]:47911", 50001, out, sizeof out) && strcmp(out, "[fd00::1]:50001") == 0, "IPv6");
        check(window_address("host.example:47911", 9, out, sizeof out) && strcmp(out, "host.example:9") == 0, "a name");
        check(!window_address("fd00::1", 9, out, sizeof out), "an IPv6 address without brackets or port");
        check(!window_address("nothing", 9, out, sizeof out) && !window_address(NULL, 9, out, sizeof out), "no port in the address");
        check(!window_address("192.0.2.1:47911", 0, out, sizeof out), "no port listed");  // check-private: ok (documentation address)
        check(!window_address("192.0.2.1:47911", 50001, out, 8), "no room");  // check-private: ok (documentation address)

        // Where a new window goes.
        NSRect room = NSMakeRect(0, 0, 1470, 919);
        NSPoint at = window_top_left(NSMakeRect(100, 200, 800, 600), NSMakeSize(400, 300), 0, room);
        check(at.x == 128 && at.y == 772, "below and to the right of the first window");
        at = window_top_left(NSMakeRect(1200, 200, 800, 600), NSMakeSize(400, 300), 1, room);
        check(at.x == 1070 && at.y == 744, "kept inside the screen on the right");
        at = window_top_left(NSMakeRect(0, 0, 400, 300), NSMakeSize(400, 300), 2, room);
        check(at.x == 84 && at.y == 300, "kept inside the screen at the bottom");

        if (failures == 0) {
            printf("window-list-test: all passed\n");
        }
        return failures == 0 ? 0 : 1;
    }
}

// Tests of keys.h: cc -Wall -o build/keys-test keys-test.c && build/keys-test
#include <stdio.h>
#include <string.h>

#include "keys.h"

static int failures = 0;

// Compares what an event caused with what it should have: codes, negative for "up".
static void expect(const char *what, const KeyChange *got, size_t count, const int *want, size_t wanted) {
    bool same = count == wanted;
    for (size_t i = 0; same && i < count; i++) {
        same = (int)got[i].code == (want[i] < 0 ? -want[i] : want[i]) && got[i].pressed == (want[i] > 0);
    }
    if (!same) {
        failures++;
        printf("FAILED: %s\n  got:", what);
        for (size_t i = 0; i < count; i++) {
            printf(" %s%u", got[i].pressed ? "+" : "-", got[i].code);
        }
        printf("\n  want:");
        for (size_t i = 0; i < wanted; i++) {
            printf(" %s%d", want[i] > 0 ? "+" : "-", want[i] < 0 ? -want[i] : want[i]);
        }
        printf("\n");
    }
}

#define EXPECT(what, count, ...)                                         \
    do {                                                                 \
        const int want[] = {__VA_ARGS__};                                \
        expect(what, out, count, want, sizeof want / sizeof want[0]);    \
    } while (0)
#define EXPECT_NOTHING(what, count) expect(what, out, count, NULL, 0)

enum { A = 0x00, COMMAND_KEY = 0x37, SHIFT_KEY = 0x38, CAPS = 0x39 };

int main(void) {
    KeyChange out[256];
    Keys keys;

    // A hotkey button sends Command-A as one key with the Command flag and no event for the
    // modifier. The other side has to see Control go down around the A, not a bare "a".
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("a hotkey without a modifier event, Command as Control",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), 29, 30, -30, -29);
    EXPECT_NOTHING("nothing stays down after it", keys_let_go(&keys, out));

    memset(&keys, 0, sizeof keys);
    keys.command_is_control = false;
    EXPECT("the same with Command as Super",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), 125, 30, -30, -125);

    // Command-Shift-A from a program: both modifiers are lent, in order, and returned in reverse.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("two lent modifiers",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT, false, out), 42, 29, 30, -30, -29, -42);

    // Control and Command together are one key on the other side when Command goes as Control.
    EXPECT("Control and Command are lent once",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND | KEYS_FLAG_CONTROL, false, out), 29, 30, -30, -29);

    // A person's hands: the modifier has an event of its own, and nothing is lent.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Command pressed by hand",
           keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("A while Command is held by hand",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), 30, -30);
    EXPECT("a repeat while Command is held is another press",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND, true, out), 30, -30);
    EXPECT("Command let go by hand", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);

    // A capital letter from a program: Shift is lent for the press only; the letter stays down
    // until its own key comes up.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Shift lent to a letter", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 42, 30, -42);
    EXPECT_NOTHING("a held key is repeated on the other side", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, true, out));
    EXPECT("the letter comes up", keys_key_up(&keys, A, out), -30);

    // Shift by hand, then a letter: plain.
    EXPECT("Shift pressed by hand",
           keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("a letter under a held Shift", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    EXPECT("losing the window lets everything go", keys_let_go(&keys, out), -30, -42);

    EXPECT("Caps Lock is a press and a release", keys_flags_changed(&keys, CAPS, 0, out), 58, -58);
    EXPECT_NOTHING("a key that does not exist", keys_key_down(&keys, 0x7F, 0, false, out));

    // While the picture is being reconnected nothing is pressed on the other side.
    bool dropped = false;
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT_NOTHING("a letter typed during a cut is not passed on",
                   keys_hold_back(&keys, out, keys_key_down(&keys, A, 0, false, out), &dropped));
    if (!dropped) {
        failures++;
        printf("FAILED: a press that was not passed on is said to have been dropped\n");
    }
    EXPECT_NOTHING("and its release is no news", keys_key_up(&keys, A, out));
    EXPECT_NOTHING("a hotkey during a cut: neither the key nor the modifier lent to it",
                   keys_hold_back(&keys, out, keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), &dropped));
    EXPECT_NOTHING("nothing is down after it", keys_let_go(&keys, out));

    // A key that went down before the cut comes up during it: that is passed on.
    EXPECT("a letter goes down before the cut", keys_key_down(&keys, A, 0, false, out), 30);
    dropped = false;
    EXPECT("and comes up during it", keys_hold_back(&keys, out, keys_key_up(&keys, A, out), &dropped), -30);
    if (dropped) {
        failures++;
        printf("FAILED: letting go is not a press that was dropped\n");
    }

    // Shift pressed during the cut is not passed on; when the picture is back and Shift is
    // still held, it goes down then.
    EXPECT_NOTHING("Shift pressed during a cut",
                   keys_hold_back(&keys, out,
                                  keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), &dropped));
    EXPECT("the picture is back and Shift is held", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out), 42);
    EXPECT_NOTHING("in line already", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out));
    EXPECT("a letter under it is plain again", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    // Shift was let go while nobody was told (the window lost the keyboard meanwhile, say).
    EXPECT("Shift is not held any more: it comes up, the letter stays", keys_fall_in_line(&keys, 0, false, out), -42);
    EXPECT("the letter comes up by itself", keys_key_up(&keys, A, out), -30);

    // Command goes over as Control, and falls in line as Control.
    EXPECT("Command held when the picture is back", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, false, out), 29);
    EXPECT_NOTHING("Control and Command are one key over there", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_CONTROL, false, out));
    EXPECT("both let go", keys_fall_in_line(&keys, 0, false, out), -29);
    keys.command_is_control = false;
    EXPECT("Command as Super", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, false, out), 125);
    EXPECT("and let go", keys_fall_in_line(&keys, 0, false, out), -125);

    // A daily reconnect restores the physical side, so its ordinary release clears it.
    keys.command_is_control = true;
    unsigned long held = KEYS_FLAG_SHIFT | KEYS_DEVICE_RIGHT_SHIFT | KEYS_FLAG_COMMAND | KEYS_DEVICE_RIGHT_COMMAND;
    EXPECT("right Shift and right Command are still held", keys_fall_in_line(&keys, held, true, out), 54, 97);
    EXPECT("right Shift comes up", keys_flags_changed(&keys, 0x3C, KEYS_FLAG_COMMAND | KEYS_DEVICE_RIGHT_COMMAND, out), -54);
    EXPECT("right Command comes up", keys_flags_changed(&keys, 0x36, 0, out), -97);
    EXPECT_NOTHING("no synthetic left modifier stays down", keys_let_go(&keys, out));

    if (failures == 0) {
        printf("keys: all tests passed\n");
    }
    return failures == 0 ? 0 : 1;
}

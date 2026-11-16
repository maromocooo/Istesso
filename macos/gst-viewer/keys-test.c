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

    if (failures == 0) {
        printf("keys: all tests passed\n");
    }
    return failures == 0 ? 0 : 1;
}

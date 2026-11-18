// Tests of keys.h: cc -Wall -o build/keys-test keys-test.c && build/keys-test
#include <stdio.h>
#include <string.h>

#include "keys.h"

static int failures = 0, checks = 0;

// Compares what an event caused with what it should have: codes, negative for "up".
static void expect(const char *what, const KeyChange *got, size_t count, const int *want, size_t wanted) {
    checks++;
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

enum {
    A = 0x00,
    COMMAND_KEY = 0x37,
    SHIFT_KEY = 0x38,
    CAPS = 0x39,
    OPTION_KEY = 0x3A,
    CONTROL_KEY = 0x3B,
    RIGHT_SHIFT_KEY = 0x3C,
    RIGHT_OPTION_KEY = 0x3D,
};

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
    EXPECT("the letter comes up", keys_key_up(&keys, A, 0, out), -30);

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
    EXPECT_NOTHING("and its release is no news", keys_key_up(&keys, A, 0, out));
    EXPECT_NOTHING("a hotkey during a cut: neither the key nor the modifier lent to it",
                   keys_hold_back(&keys, out, keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), &dropped));
    EXPECT_NOTHING("nothing is down after it", keys_let_go(&keys, out));

    // A key that went down before the cut comes up during it: that is passed on.
    EXPECT("a letter goes down before the cut", keys_key_down(&keys, A, 0, false, out), 30);
    dropped = false;
    EXPECT("and comes up during it", keys_hold_back(&keys, out, keys_key_up(&keys, A, 0, out), &dropped), -30);
    if (dropped) {
        failures++;
        printf("FAILED: letting go is not a press that was dropped\n");
    }

    // Shift pressed during the cut is not passed on; when the picture is back and Shift is
    // still held, it goes down then.
    EXPECT_NOTHING("Shift pressed during a cut",
                   keys_hold_back(&keys, out,
                                  keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), &dropped));
    EXPECT("the picture is back and Shift is held", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, out), 42);
    EXPECT_NOTHING("in line already", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, out));
    EXPECT("a letter under it is plain again", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    // Shift was let go while nobody was told (the window lost the keyboard meanwhile, say).
    EXPECT("Shift is not held any more: it comes up, the letter stays", keys_fall_in_line(&keys, 0, out), -42);
    EXPECT("the letter comes up by itself", keys_key_up(&keys, A, 0, out), -30);

    // Command goes over as Control, and falls in line as Control.
    EXPECT("Command held when the picture is back", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, out), 29);
    EXPECT_NOTHING("Control and Command are one key over there", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_CONTROL, out));
    EXPECT("both let go", keys_fall_in_line(&keys, 0, out), -29);
    keys.command_is_control = false;
    EXPECT("Command as Super", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, out), 125);
    EXPECT("and let go", keys_fall_in_line(&keys, 0, out), -125);

    // (a) A modifier's event with the modifier's flag and neither of its two keys' bits: the
    // modifier is down all the same, and goes over as its left key.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Shift with no key bit goes as left Shift", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT, out), 42);
    EXPECT("and comes up when the flag goes", keys_flags_changed(&keys, SHIFT_KEY, 0, out), -42);
    EXPECT("right Shift with no key bit goes as left Shift too",
           keys_flags_changed(&keys, RIGHT_SHIFT_KEY, KEYS_FLAG_SHIFT, out), 42);
    EXPECT("and comes up", keys_flags_changed(&keys, RIGHT_SHIFT_KEY, 0, out), -42);
    EXPECT("Option with no key bit goes as left Alt", keys_flags_changed(&keys, OPTION_KEY, KEYS_FLAG_OPTION, out), 56);
    EXPECT_NOTHING("a drag under it finds it in line already", keys_fall_in_line(&keys, KEYS_FLAG_OPTION, out));
    EXPECT("Option comes up", keys_flags_changed(&keys, OPTION_KEY, 0, out), -56);
    EXPECT("Command with no key bit goes as left Control", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND, out), 29);
    EXPECT("and comes up", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);
    keys.command_is_control = false;
    EXPECT("Command with no key bit as Super", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND, out), 125);
    EXPECT("and comes up", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -125);
    keys.command_is_control = true;
    // With the bits, the key the bits name goes, as before.
    EXPECT("right Option with its bit is right Alt",
           keys_flags_changed(&keys, RIGHT_OPTION_KEY, KEYS_FLAG_OPTION | KEYS_DEVICE_RIGHT_OPTION, out), 100);
    EXPECT("and comes up", keys_flags_changed(&keys, RIGHT_OPTION_KEY, 0, out), -100);
    EXPECT("left Shift by hand", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("right Shift too",
           keys_flags_changed(&keys, RIGHT_SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT | KEYS_DEVICE_RIGHT_SHIFT, out),
           54);
    EXPECT("right Shift let go, left still held",
           keys_flags_changed(&keys, RIGHT_SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), -54);
    EXPECT("left Shift let go", keys_flags_changed(&keys, SHIFT_KEY, 0, out), -42);
    // Command and Control are one key over there: letting go of one while the other is held
    // leaves it down.
    EXPECT("Control by hand", keys_flags_changed(&keys, CONTROL_KEY, KEYS_FLAG_CONTROL | KEYS_DEVICE_LEFT_CONTROL, out), 29);
    EXPECT_NOTHING("Command by hand is the same key",
                   keys_flags_changed(&keys, COMMAND_KEY,
                                      KEYS_FLAG_CONTROL | KEYS_DEVICE_LEFT_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND,
                                      out));
    EXPECT_NOTHING("Command let go while Control is held",
                   keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_CONTROL | KEYS_DEVICE_LEFT_CONTROL, out));
    EXPECT("Control let go", keys_flags_changed(&keys, CONTROL_KEY, 0, out), -29);
    EXPECT("Command with its bit", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT_NOTHING("Control with its bit while Command is held",
                   keys_flags_changed(&keys, CONTROL_KEY,
                                      KEYS_FLAG_CONTROL | KEYS_DEVICE_LEFT_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND,
                                      out));
    EXPECT_NOTHING("Control let go while Command is held",
                   keys_flags_changed(&keys, CONTROL_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out));
    EXPECT("Command let go", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);

    // (b) Every key, button and scroll brings the modifiers in line with its flags first.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT_NOTHING("nothing comes up that was not down", keys_fall_in_line(&keys, 0, out));
    EXPECT("a letter with nothing held is only the letter", keys_key_down(&keys, A, 0, false, out), 30);
    EXPECT("its release", keys_key_up(&keys, A, 0, out), -30);
    EXPECT("Shift by hand", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    // Shift's release went somewhere else; the next letter's flags say Shift is not held.
    EXPECT("a stale Shift comes up before the letter", keys_key_down(&keys, A, 0, false, out), -42, 30);
    EXPECT("the letter comes up", keys_key_up(&keys, A, 0, out), -30);
    EXPECT("Shift by hand again", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("a letter under it", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    EXPECT("a stale Shift comes up before a letter's release", keys_key_up(&keys, A, 0, out), -42, -30);
    EXPECT("Shift by hand once more", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("a letter under it", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    EXPECT("a repeat without Shift lets the stale Shift go and is not sent", keys_key_down(&keys, A, 0, true, out), -42);
    EXPECT("the letter comes up", keys_key_up(&keys, A, 0, out), -30);
    // Before a button or a scroll the missing modifiers are pressed and stay down.
    EXPECT("Shift held for a scroll, never seen pressed", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, out), 42);
    EXPECT_NOTHING("the next scroll finds it in line", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, out));
    EXPECT("a scroll without it lets it go", keys_fall_in_line(&keys, 0, out), -42);
    EXPECT("Option held for a drag", keys_fall_in_line(&keys, KEYS_FLAG_OPTION, out), 56);
    EXPECT("Option let go by hand, with no key bit", keys_flags_changed(&keys, OPTION_KEY, 0, out), -56);
    EXPECT("Command held for a click goes as Control", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, out), 29);
    EXPECT("Command with Shift for the next", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT, out), 42);
    EXPECT("all let go", keys_fall_in_line(&keys, 0, out), -42, -29);
    // A modifier's own event brings the others in line too.
    EXPECT("Shift by hand", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("Option pressed while Shift's release was lost",
           keys_flags_changed(&keys, OPTION_KEY, KEYS_FLAG_OPTION | KEYS_DEVICE_LEFT_OPTION, out), -42, 56);
    EXPECT("Shift held again, its press lost: Option's release presses it",
           keys_flags_changed(&keys, OPTION_KEY, KEYS_FLAG_SHIFT, out), 42, -56);
    EXPECT("losing the window lets it go", keys_let_go(&keys, out), -42);
    // During a cut a stale modifier still comes up; nothing goes down.
    EXPECT("Shift by hand", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    dropped = false;
    EXPECT("a letter during a cut: only the stale Shift's release",
           keys_hold_back(&keys, out, keys_key_down(&keys, A, 0, false, out), &dropped), -42);
    EXPECT_NOTHING("Shift held for a scroll during a cut is not pressed",
                   keys_hold_back(&keys, out, keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, out), &dropped));
    EXPECT_NOTHING("nothing is down", keys_let_go(&keys, out));

    // (c) The window stops being the one typed into, and is again.
    // Command-Tab away: Command goes down here, Tab goes to the system, the window loses the
    // keyboard; Command's release goes to another application.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Command by hand", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Command-Tab away lets Control go over there", keys_let_go(&keys, out), -29);
    EXPECT_NOTHING("back with nothing held", keys_fall_in_line(&keys, 0, out));
    // Command-Tab back: Command is still held when the window is back, and let go after.
    EXPECT("back with Command still held", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Command let go in the window", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);
    // Command-Shift-4: both go down here, 4 goes to the system.
    EXPECT("Command by hand", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Shift by hand",
           keys_flags_changed(&keys, SHIFT_KEY,
                              KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND | KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out),
           42);
    // If the window loses the keyboard to the screenshot, both come up then...
    Keys copy = keys;
    EXPECT("the screenshot takes the keyboard", keys_let_go(&keys, out), -29, -42);
    EXPECT_NOTHING("back with nothing held", keys_fall_in_line(&keys, 0, out));
    // ...and if it does not, and their releases never come, the next click lets them go first.
    keys = copy;
    EXPECT("the next click, nothing held", keys_fall_in_line(&keys, 0, out), -42, -29);
    // Back with Command and Shift held: both go down, as left keys.
    EXPECT("back with Command and Shift held",
           keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT | KEYS_DEVICE_RIGHT_SHIFT, out), 42, 29);
    EXPECT("the window lets them go", keys_let_go(&keys, out), -29, -42);

    if (failures == 0) {
        printf("keys: all %d checks passed\n", checks);
    }
    return failures == 0 ? 0 : 1;
}

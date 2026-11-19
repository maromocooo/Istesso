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
// A condition that must hold.
#define CHECK(what, condition)                    \
    do {                                          \
        checks++;                                 \
        if (!(condition)) {                       \
            failures++;                           \
            printf("FAILED: %s\n", what);         \
        }                                         \
    } while (0)

enum {
    A = 0x00,
    COMMAND_KEY = 0x37,
    SHIFT_KEY = 0x38,
    CAPS = 0x39,
    B = 0x0B,
    C = 0x08,
    V = 0x09,
    RIGHT_COMMAND_KEY = 0x36,
    OPTION_KEY = 0x3A,
    CONTROL_KEY = 0x3B,
    RIGHT_SHIFT_KEY = 0x3C,
    RIGHT_OPTION_KEY = 0x3D,
    RIGHT_CONTROL_KEY = 0x3E,
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
    EXPECT("the picture is back and Shift is held", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out), 42);
    EXPECT_NOTHING("in line already", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out));
    EXPECT("a letter under it is plain again", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 30);
    // Shift was let go while nobody was told (the window lost the keyboard meanwhile, say).
    EXPECT("Shift is not held any more: it comes up, the letter stays", keys_fall_in_line(&keys, 0, false, out), -42);
    EXPECT("the letter comes up by itself", keys_key_up(&keys, A, 0, out), -30);

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
    EXPECT_NOTHING("a drag under it finds it in line already", keys_fall_in_line(&keys, KEYS_FLAG_OPTION, false, out));
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
    EXPECT_NOTHING("nothing comes up that was not down", keys_fall_in_line(&keys, 0, false, out));
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
    EXPECT("Shift held for a scroll, never seen pressed", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out), 42);
    EXPECT_NOTHING("the next scroll finds it in line", keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out));
    EXPECT("a scroll without it lets it go", keys_fall_in_line(&keys, 0, false, out), -42);
    EXPECT("Option held for a drag", keys_fall_in_line(&keys, KEYS_FLAG_OPTION, false, out), 56);
    EXPECT("Option let go by hand, with no key bit", keys_flags_changed(&keys, OPTION_KEY, 0, out), -56);
    EXPECT("Command held for a click goes as Control", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND, false, out), 29);
    EXPECT("Command with Shift for the next", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT, false, out), 42);
    EXPECT("all let go", keys_fall_in_line(&keys, 0, false, out), -42, -29);
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
                   keys_hold_back(&keys, out, keys_fall_in_line(&keys, KEYS_FLAG_SHIFT, false, out), &dropped));
    EXPECT_NOTHING("nothing is down", keys_let_go(&keys, out));

    // (c) The window stops being the one typed into, and is again.
    // Command-Tab away: Command goes down here, Tab goes to the system, the window loses the
    // keyboard; Command's release goes to another application.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Command by hand", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Command-Tab away lets Control go over there", keys_let_go(&keys, out), -29);
    EXPECT_NOTHING("back with nothing held", keys_fall_in_line(&keys, 0, false, out));
    // Command-Tab back: Command is still held when the window is back, and let go after.
    EXPECT("back with Command still held", keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, false, out), 29);
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
    EXPECT_NOTHING("back with nothing held", keys_fall_in_line(&keys, 0, false, out));
    // ...and if it does not, and their releases never come, the next click lets them go first.
    keys = copy;
    EXPECT("the next click, nothing held", keys_fall_in_line(&keys, 0, false, out), -42, -29);
    // Back with Command and Shift held: both go down, as left keys.
    EXPECT("back with Command and Shift held",
           keys_fall_in_line(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT | KEYS_DEVICE_RIGHT_SHIFT, false, out), 42, 29);
    EXPECT("the window lets them go", keys_let_go(&keys, out), -29, -42);

    // Control with no key bit goes as left Control.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Control with no key bit goes as left Control", keys_flags_changed(&keys, CONTROL_KEY, KEYS_FLAG_CONTROL, out), 29);
    EXPECT("and comes up", keys_flags_changed(&keys, CONTROL_KEY, 0, out), -29);
    keys.command_is_control = false;
    EXPECT("the same when Command goes as Super", keys_flags_changed(&keys, CONTROL_KEY, KEYS_FLAG_CONTROL, out), 29);
    EXPECT("and comes up", keys_flags_changed(&keys, CONTROL_KEY, 0, out), -29);

    // A stale modifier comes up before the ones lent to a key go down.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Option by hand", keys_flags_changed(&keys, OPTION_KEY, KEYS_FLAG_OPTION | KEYS_DEVICE_LEFT_OPTION, out), 56);
    EXPECT("A with Shift's flag after Option's release was lost: Option up, Shift lent",
           keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), -56, 42, 30, -42);
    EXPECT("the letter comes up", keys_key_up(&keys, A, KEYS_FLAG_SHIFT, out), -30);

    // Command-C, X and V taken for copy and paste (ADR 0022): the viewer sends nothing for the
    // letter's press and passes every release on to keys_key_up(), which sends a release only for
    // a key that is down over there. While Command is held no release comes, so a C let go before
    // Command stays taken in the viewer, and the release of its next plain press must go.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Command by hand", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    // C goes down and is taken: nothing is sent, nothing is noted.
    EXPECT_NOTHING("a taken C's release with Command held sends nothing",
                   keys_key_up(&keys, C, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out));
    EXPECT("Command let go", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);
    EXPECT_NOTHING("a taken C's release after Command's sends nothing", keys_key_up(&keys, C, 0, out));
    // Command-C again, and C let go first: its release never comes.
    EXPECT("Command by hand again", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Command let go after C", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);
    EXPECT("the next plain C goes down", keys_key_down(&keys, C, 0, false, out), 46);
    EXPECT_NOTHING("its repeats are the host's", keys_key_down(&keys, C, 0, true, out));
    EXPECT("its release goes: it went down over there", keys_key_up(&keys, C, 0, out), -46);
    EXPECT("the press after it is not lost", keys_key_down(&keys, C, 0, false, out), 46);
    EXPECT("nor its release", keys_key_up(&keys, C, 0, out), -46);
    // V held down plain, then Command: V's repeat under Command is taken for a paste. When Command
    // comes up first, V's release comes, and lets go of the V that is down over there.
    EXPECT("V held down plain", keys_key_down(&keys, V, 0, false, out), 47);
    EXPECT("Command by hand under it", keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_COMMAND, out), 29);
    EXPECT("Command let go before V", keys_flags_changed(&keys, COMMAND_KEY, 0, out), -29);
    EXPECT("V's release lets it go over there", keys_key_up(&keys, V, 0, out), -47);
    EXPECT_NOTHING("nothing stays down", keys_let_go(&keys, out));

    // Only the key itself makes a press to count.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    size_t n;
    keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out);
    n = keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out);
    CHECK("a letter's press counts", keys_presses_key(&keys, A, out, n));
    n = keys_key_down(&keys, A, 0, true, out);
    EXPECT("a repeat after Shift's release was lost", n, -42);
    CHECK("only lets Shift go and does not count", !keys_presses_key(&keys, A, out, n));
    keys_key_up(&keys, A, 0, out);
    n = keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out);
    CHECK("a hotkey counts once, the lent modifier not by itself", n == 4 && keys_presses_key(&keys, A, out, n));

    // Around a button or a scroll.
    size_t before = 0;
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    n = keys_around_pointer(&keys, KEYS_FLAG_COMMAND, false, false, out, &before);
    EXPECT("a scroll over the window while another application is in front, Command held there: lent", n, 29, -29);
    CHECK("  pressed before the scroll, let go after it", before == 1);
    EXPECT_NOTHING("nothing stays down over there", keys_let_go(&keys, out));
    n = keys_around_pointer(&keys, KEYS_FLAG_COMMAND | KEYS_FLAG_SHIFT, false, false, out, &before);
    EXPECT("a Command-Shift click on it: both lent", n, 42, 29, -29, -42);
    CHECK("  both before, both after", before == 2);
    n = keys_around_pointer(&keys, KEYS_FLAG_COMMAND, true, false, out, &before);
    EXPECT("the same in the window typed into: Command goes down and stays", n, 29);
    CHECK("  before the click", before == 1);
    n = keys_around_pointer(&keys, KEYS_FLAG_COMMAND, false, false, out, &before);
    EXPECT_NOTHING("lent nothing when it is down already", n);
    n = keys_around_pointer(&keys, 0, false, false, out, &before);
    EXPECT("a stale one comes up before, typed into or not", n, -29);
    CHECK("  before", before == 1);
    n = keys_around_pointer(&keys, KEYS_FLAG_SHIFT, true, true, out, &before);
    EXPECT_NOTHING("while the picture is being reconnected nothing goes down", n);
    keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out);
    n = keys_around_pointer(&keys, 0, false, true, out, &before);
    EXPECT("but a stale one comes up", n, -42);
    CHECK("  before", before == 1);

    // A pointer's move only lets go, and sends nothing when all is in line.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT_NOTHING("a move with Shift held and not down over there presses nothing", keys_releases(&keys, KEYS_FLAG_SHIFT, out));
    keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out);
    EXPECT_NOTHING("a move with Shift held and down over there: nothing", keys_releases(&keys, KEYS_FLAG_SHIFT, out));
    EXPECT("a move after Shift's release went to Spotlight lets it go", keys_releases(&keys, 0, out), -42);
    EXPECT_NOTHING("and the next move sends nothing", keys_releases(&keys, 0, out));

    // Left as they are (written down in ADR 0007):
    // A program's hotkey that carries only Command lets go of a Shift held by hand; the next keys
    // under that Shift borrow it, until a click or a scroll presses it for good.
    memset(&keys, 0, sizeof keys);
    keys.command_is_control = true;
    EXPECT("Shift by hand", keys_flags_changed(&keys, SHIFT_KEY, KEYS_FLAG_SHIFT | KEYS_DEVICE_LEFT_SHIFT, out), 42);
    EXPECT("a program's Command-A with only Command's flag lets Shift go",
           keys_key_down(&keys, A, KEYS_FLAG_COMMAND, false, out), -42, 29, 30, -30, -29);
    EXPECT("B under the Shift still held borrows it", keys_key_down(&keys, B, KEYS_FLAG_SHIFT, false, out), 42, 48, -42);
    n = keys_around_pointer(&keys, KEYS_FLAG_SHIFT, true, false, out, &before);
    EXPECT("a click presses it for good", n, 42);
    keys_let_go(&keys, out);
    // A borrowed Shift is up again while the letter is held: the other side repeats it without.
    EXPECT("A with Shift's flag and its press lost", keys_key_down(&keys, A, KEYS_FLAG_SHIFT, false, out), 42, 30, -42);
    keys_let_go(&keys, out);
    // Command as Control and Control by hand are one key over there: the side can come out
    // wrong, nothing stays down.
    EXPECT("right Control", keys_flags_changed(&keys, RIGHT_CONTROL_KEY, KEYS_FLAG_CONTROL | KEYS_DEVICE_RIGHT_CONTROL, out), 97);
    EXPECT("left Command",
           keys_flags_changed(&keys, COMMAND_KEY,
                              KEYS_FLAG_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_RIGHT_CONTROL | KEYS_DEVICE_LEFT_COMMAND, out),
           29);
    EXPECT_NOTHING("left Command up, right Control held: left Control stays",
                   keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_CONTROL | KEYS_DEVICE_RIGHT_CONTROL, out));
    EXPECT("right Control up: both", keys_flags_changed(&keys, RIGHT_CONTROL_KEY, 0, out), -29, -97);
    keys_flags_changed(&keys, CONTROL_KEY, KEYS_FLAG_CONTROL | KEYS_DEVICE_LEFT_CONTROL, out);
    keys_flags_changed(&keys, COMMAND_KEY, KEYS_FLAG_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_CONTROL | KEYS_DEVICE_LEFT_COMMAND, out);
    EXPECT("left Control, left and right Command",
           keys_flags_changed(&keys, RIGHT_COMMAND_KEY,
                              KEYS_FLAG_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_CONTROL | KEYS_DEVICE_LEFT_COMMAND
                                  | KEYS_DEVICE_RIGHT_COMMAND,
                              out),
           97);
    EXPECT("left Command up: left Control comes up while it is held, right stays",
           keys_flags_changed(&keys, COMMAND_KEY,
                              KEYS_FLAG_CONTROL | KEYS_FLAG_COMMAND | KEYS_DEVICE_LEFT_CONTROL | KEYS_DEVICE_RIGHT_COMMAND, out),
           -29);
    EXPECT("all let go", keys_flags_changed(&keys, CONTROL_KEY, 0, out), -97);

    if (failures == 0) {
        printf("keys: all %d checks passed\n", checks);
    }
    return failures == 0 ? 0 : 1;
}

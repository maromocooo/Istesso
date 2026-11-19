// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.14): FieldFollow.decide without Android.
package com.maromoco.istesso.dev.aura;

import static com.maromoco.istesso.dev.aura.FieldFollow.Act.*;

public final class FieldFollowTest {
    static int failed;

    static void expect(FieldFollow.Act want, int state, boolean became, boolean typing, boolean manualOff, boolean auto,
            boolean takes, boolean resumed, String what) {
        FieldFollow.Act got = FieldFollow.decide(state, became, typing, manualOff, auto, takes, resumed);
        if (got != want) {
            failed++;
            System.out.println("FAIL " + what + ": " + got + " (wanted " + want + ")");
        }
    }

    public static void main(String[] a) {
        expect(SHOW, 1, true, false, false, false, true, true, "a field has the keyboard: typing on");
        expect(SHOW, 2, true, false, false, false, true, true, "a password field too");
        expect(NONE, 1, true, false, true, false, true, true, "turned off by hand: stays off");
        expect(RESHOW, 1, true, true, false, true, true, true, "another field while typing: shown again");
        expect(NONE, 1, false, true, false, true, true, true, "the caret moved: nothing (a hidden keyboard stays hidden)");
        expect(ARM_CLOSE, 3, false, true, false, true, true, true, "not in a field, opened this way: close soon");
        expect(NONE, 3, false, true, false, false, true, true, "not in a field, opened by hand: stays");
        expect(NONE, 0, false, true, false, true, true, true, "nothing known: nothing changes");
        expect(NONE, 1, true, false, false, false, false, true, "the host takes no text: nothing");
        expect(NONE, 1, true, false, false, false, true, false, "the panel is in the background: nothing");
        System.out.println(failed == 0 ? "FieldFollow: all 10 cases pass" : "FieldFollow: " + failed + " failed");
        System.exit(failed == 0 ? 0 : 1);
    }
}

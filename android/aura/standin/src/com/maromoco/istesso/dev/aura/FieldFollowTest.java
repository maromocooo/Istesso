// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.14): FieldFollow.decide without Android.
package com.maromoco.istesso.dev.aura;

import static com.maromoco.istesso.dev.aura.FieldFollow.Act.*;

public final class FieldFollowTest {
    static int failed;
    static int count;

    static void changeAt(boolean want, int state, long startToNotice, String what) {
        count++;
        if (FieldFollow.holdOnFieldChange(true, false, state, startToNotice) != want) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void pieces(java.util.List<String> want, String text, String what) {
        count++;
        if (!FieldFollow.pieces(text).equals(want)) {
            failed++;
            System.out.println("FAIL " + what + ": " + FieldFollow.pieces(text).size() + " pieces");
        }
    }

    static void unplacedCase(java.util.List<String> want, java.util.List<FieldFollow.Piece> sent, int n, long now, String what) {
        count++;
        if (!FieldFollow.unplaced(sent, n, now).equals(want)) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void change(boolean want, boolean composing, boolean shown, int state, String what) {
        count++;
        if (FieldFollow.holdOnFieldChange(composing, shown, state) != want) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void commit(FieldFollow.Commit want, boolean open, boolean takes, int state, String what) {
        count++;
        if (FieldFollow.onCommit(open, takes, state) != want) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void unplaced(boolean want, int state, String what) {
        count++;
        if (FieldFollow.keepUnplaced(state) != want) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void expect(FieldFollow.Act want, int state, boolean became, boolean typing, boolean manualOff, boolean auto,
            boolean takes, boolean resumed, String what) {
        count++;
        FieldFollow.Act got = FieldFollow.decide(state, became, typing, manualOff, auto, takes, resumed);
        if (got != want) {
            failed++;
            System.out.println("FAIL " + what + ": " + got + " (wanted " + want + ")");
        }
    }

    static void hold(boolean want, boolean composing, int lastState, String what) {
        count++;
        if (FieldFollow.holdOnClose(composing, lastState) != want) {
            failed++;
            System.out.println("FAIL " + what);
        }
    }

    static void place(boolean want, boolean held, int state, boolean takes, String what) {
        count++;
        if (FieldFollow.canPlaceHeld(held, state, takes) != want) {
            failed++;
            System.out.println("FAIL " + what);
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
        hold(true, true, 1, "composing when the field goes: kept on the panel");
        hold(false, false, 1, "nothing composed: nothing kept");
        hold(false, true, 2, "a password field: nothing of it kept");
        hold(false, true, 0, "nothing known (Chrome's Tab into a password field): never kept");
        change(true, true, false, 1, "another field, composition on the strip only: kept");
        change(false, true, true, 1, "another field, composition shown in the host's app: the app commits it, not kept");
        change(false, true, false, 2, "from a password field: not kept");
        change(false, true, false, 0, "nothing known: not kept");
        change(false, false, false, 1, "nothing composed: nothing kept");
        commit(FieldFollow.Commit.SEND, true, true, 1, "open input, host takes text: sent");
        commit(FieldFollow.Commit.SEND, true, true, 0, "open input, nothing known: sent as before");
        commit(FieldFollow.Commit.HOLD, false, true, 1, "input reconnecting, plain field: kept on the panel");
        commit(FieldFollow.Commit.DROP, false, true, 2, "input reconnecting, password field: never kept");
        commit(FieldFollow.Commit.DROP, false, true, 0, "input reconnecting, nothing known: never kept");
        unplaced(true, 1, "the host could not place text from a plain field: kept on the panel");
        unplaced(false, 2, "nor from a password field: not kept");
        unplaced(false, 0, "nor when nothing was known: not kept");
        place(true, true, 1, true, "kept text, a plain field again: may be put there");
        place(false, true, 2, true, "kept text, a password field: not put there");
        place(false, true, 3, true, "kept text, not in a field: waits");
        place(false, true, 0, true, "kept text, nothing known: waits");
        place(false, true, 1, false, "kept text, the host takes no text: waits");
        place(false, false, 1, true, "nothing kept: nothing to put");
        changeAt(true, 1, 500_000_000L, "a composition well before the change: kept");
        changeAt(false, 1, 50_000_000L, "a composition just before the notice (it may be in the new password field): not kept");
        pieces(java.util.List.of("abc"), "abc", "one piece");
        pieces(java.util.List.of("abc", "def"), "abc\ndef\t", "newline and tab are keys, not pieces");
        pieces(java.util.List.of(), "\n", "a bare newline is no piece");
        pieces(java.util.List.of("あ".repeat(256), "あ".repeat(44)), "あ".repeat(300), "256 characters a piece");
        java.util.List<FieldFollow.Piece> sent = new java.util.ArrayList<>();
        long t = 10_000_000_000L;
        sent.add(new FieldFollow.Piece("一", t - 5_000_000_000L, true));
        sent.add(new FieldFollow.Piece("二", t - 2_000_000_000L, true));
        sent.add(new FieldFollow.Piece("三", t - 1_000_000_000L, true));
        sent.add(new FieldFollow.Piece("秘", t - 500_000_000L, false));
        unplacedCase(java.util.List.of(), sent, 1, t, "the last piece was not keepable: nothing kept");
        unplacedCase(java.util.List.of("三"), sent, 2, t, "two dropped: the last two, the keepable one only");
        unplacedCase(java.util.List.of("二", "三"), sent, 3, t, "three dropped: only those of the last 3 s");
        unplacedCase(java.util.List.of(), sent, 0, t, "nothing dropped: nothing kept");
        System.out.println(failed == 0 ? "FieldFollow: all " + count + " cases pass" : "FieldFollow: " + failed + " of " + count + " failed");
        System.exit(failed == 0 ? 0 : 1);
    }
}

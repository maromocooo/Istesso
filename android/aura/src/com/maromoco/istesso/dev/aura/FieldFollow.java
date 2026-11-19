// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.14).
package com.maromoco.istesso.dev.aura;

/**
 * What the panel's keyboard does when the host says what its text field is (the media's kind 5).
 * Pure, so that it is tested without Android (android/aura/standin).
 */
final class FieldFollow {
    private FieldFollow() {}

    enum Act {
        /** Turn typing on and show the keyboard. */
        SHOW,
        /** Typing is on: show the keyboard again (it may have been hidden), without restarting the input. */
        RESHOW,
        /** "Not in a field": close a keyboard that was opened this way, once that has lasted a moment. */
        ARM_CLOSE,
        NONE
    }

    /**
     * When a keyboard that was opened by the field closes because the host is "not in a field":
     * whether what is being composed is kept on the panel (never committed into whatever has the
     * keyboard over there, never dropped). Not from a password field (nothing of it is kept).
     */
    static boolean holdOnClose(boolean composing, int composeState) {
        return composing && keepable(composeState);
    }

    /**
     * Only text typed while the host said a plain field had the keyboard (state 1) is ever kept
     * on the panel. A password field (2), and nothing known (0: e.g. Chrome when Tab moves into a
     * password field, which it then does not say), are never kept: nothing typed into what may be
     * a password field lands on the bar.
     */
    static boolean keepable(int state) {
        return state == 1;
    }

    /**
     * Another field has the keyboard over there, or the field became a password field, while
     * something is composed here. Shown inside the host's application (preedit), the application
     * commits it into the old field itself (Firefox does): it is not kept, or it would be typed
     * twice. Shown only on the panel's strip, it would be lost: it is kept.
     */
    static boolean holdOnFieldChange(boolean composing, boolean preeditShownInHost, int composeState) {
        return composing && !preeditShownInHost && keepable(composeState);
    }

    /**
     * How long before a notice of a field change a composition may have begun in the new field,
     * the host's notice not having arrived yet: its state is not known, so it is never kept.
     */
    static final long UNSURE_NS = 200_000_000L;

    /**
     * The same, with the time from the composition's start to the notice: a composition that
     * began less than UNSURE_NS before it may have begun in the new field (a password field,
     * perhaps) and is never kept.
     */
    static boolean holdOnFieldChange(boolean composing, boolean preeditShownInHost, int composeState,
            long startToNoticeNs) {
        return holdOnFieldChange(composing, preeditShownInHost, composeState) && startToNoticeNs >= UNSURE_NS;
    }

    /** One piece of committed text as it went to the host, and whether it may be kept. */
    static final class Piece {
        final String text;
        final long atNs;
        final boolean keepable;

        Piece(String text, long atNs, boolean keepable) {
            this.text = text;
            this.atNs = atNs;
            this.keepable = keepable;
        }
    }

    /**
     * Committed text as the input library sends it (istesso-client's text_events): the text
     * pieces, newline and tab apart (they go as keys, never as text, and are not returned),
     * other control characters left out, pieces of at most 256 characters and 1024 bytes. The
     * host counts the pieces it could not place, so this says how many a commit was.
     */
    static java.util.List<String> pieces(String text) {
        java.util.List<String> out = new java.util.ArrayList<>();
        StringBuilder piece = new StringBuilder();
        int chars = 0;
        int bytes = 0;
        for (int i = 0; i < text.length(); ) {
            int c = text.codePointAt(i);
            i += Character.charCount(c);
            if (c == '\n' || c == '\r' || c == '\t') {
                if (piece.length() > 0) {
                    out.add(piece.toString());
                    piece.setLength(0);
                    chars = 0;
                    bytes = 0;
                }
                continue;
            }
            if (Character.isISOControl(c)) {
                continue;
            }
            int size = new String(Character.toChars(c)).getBytes(java.nio.charset.StandardCharsets.UTF_8).length;
            if (chars + 1 > 256 || bytes + size > 1024) {
                out.add(piece.toString());
                piece.setLength(0);
                chars = 0;
                bytes = 0;
            }
            piece.appendCodePoint(c);
            chars++;
            bytes += size;
        }
        if (piece.length() > 0) {
            out.add(piece.toString());
        }
        return out;
    }

    /**
     * The host's count of pieces it could not place went up by `count`: the pieces it means are
     * taken to be the last `count` sent (the host places them in the order they come), and of
     * those, the ones sent within the last 3 s that may be kept. Never a bare newline (no piece is).
     */
    static java.util.List<String> unplaced(java.util.List<Piece> sent, int count, long nowNs) {
        java.util.List<String> out = new java.util.ArrayList<>();
        int from = Math.max(0, sent.size() - Math.max(0, count));
        for (int i = from; i < sent.size(); i++) {
            Piece p = sent.get(i);
            if (p.keepable && nowNs - p.atNs < 3_000_000_000L && !p.text.isEmpty()) {
                out.add(p.text);
            }
        }
        return out;
    }

    enum Commit {
        /** To the host now. */
        SEND,
        /** Kept on the panel (the input is reconnecting: the input library would drop it). */
        HOLD,
        /** Neither (not a plain field: never kept). */
        DROP
    }

    /** What becomes of committed text: sent when the input is open to a host that takes text. */
    static Commit onCommit(boolean inputOpen, boolean takesText, int state) {
        if (inputOpen && takesText) {
            return Commit.SEND;
        }
        return keepable(state) ? Commit.HOLD : Commit.DROP;
    }

    /** Committed text the host had nowhere to put: kept on the panel when it was typed in a plain field. */
    static boolean keepUnplaced(int sentState) {
        return keepable(sentState);
    }

    /**
     * Whether text kept on the panel may be put into the field that has the keyboard now: a plain
     * field (not a password field, not unknown, not outside a field) of a host that takes text.
     */
    static boolean canPlaceHeld(boolean held, int state, boolean takesText) {
        return held && takesText && state == 1;
    }

    /**
     * @param state       the field's state (0 nothing known, 1 a field, 2 a password field, 3 not in a field)
     * @param becameActive whether a field has the keyboard newly (another serial, or from a state that was not one)
     * @param typing      typing is on
     * @param manualOff   typing was turned off by hand for this field
     * @param autoTyping  typing was turned on by this, not by hand
     * @param takesText   the host takes committed text
     * @param resumed     the panel is in front
     */
    static Act decide(int state, boolean becameActive, boolean typing, boolean manualOff, boolean autoTyping,
            boolean takesText, boolean resumed) {
        if (!resumed || !takesText) {
            return Act.NONE;
        }
        boolean active = state == 1 || state == 2;
        if (active) {
            if (!typing) {
                return manualOff ? Act.NONE : Act.SHOW;
            }
            // Only when a field has the keyboard newly: not on every move of its caret, so that a
            // keyboard the person hid stays hidden while they type on.
            return becameActive ? Act.RESHOW : Act.NONE;
        }
        if (state == 3 && typing && autoTyping) {
            return Act.ARM_CLOSE;
        }
        return Act.NONE;
    }
}

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
    static boolean holdOnClose(boolean composing, int lastFieldState) {
        return composing && lastFieldState != 2;
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

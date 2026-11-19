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

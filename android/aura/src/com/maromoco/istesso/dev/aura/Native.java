// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import java.nio.ByteBuffer;

/**
 * The Rust library (crates/istesso-aura), which carries pairing, the picture's connection and
 * the input's connection. Nothing in Java does cryptography or speaks a protocol of Istesso.
 * Texts from here never contain a key, a token or a pairing code.
 */
final class Native {
    static {
        System.loadLibrary("istesso_aura");
    }

    private Native() {}

    /** Which last error {@link #lastError} returns. */
    static final int ERROR_PAIRING = 0;
    static final int ERROR_PICTURE = 1;
    static final int ERROR_INPUT = 2;

    /** watchNext: what came. */
    static final int NEXT_OVER = -1;
    static final int NEXT_TOO_BIG = -2;
    static final int NEXT_NOTHING = 0;
    static final int NEXT_FRAME = 1;
    static final int NEXT_CURSOR = 2;
    static final int NEXT_CUT = 3;
    static final int NEXT_CONNECTED = 4;
    /** TRIAL ONLY: the host has said what its text input field is (watchTextInput). */
    static final int NEXT_TEXT_INPUT = 5;

    /** Input events, three ints each: kind, a, b (istesso_client.h). */
    static final int INPUT_KEY = 1;
    static final int INPUT_POINTER_AT = 2;
    static final int INPUT_BUTTON = 3;
    static final int INPUT_SCROLL = 4;

    /** {id, name} of the host joined, or null: lastError(ERROR_PAIRING) on the same thread. */
    static native String[] pairJoin(String configDir, String address, String code, String name);

    /**
     * The picture of the paired host `peer` (its id, never empty) at `address`; 0 when no host
     * is named (lastError(ERROR_PAIRING)) or it cannot be opened (lastError(ERROR_PICTURE)).
     */
    static native long watchOpen(String configDir, String address, String peer);

    /**
     * Waits up to waitMs. NEXT_FRAME: the frame is at the start of `into` and meta holds
     * {length, seq, pts_ns, key_frame, discontinuity, whole_ns, encoded_ns}. One thread calls this.
     */
    static native int watchNext(long watch, int waitMs, ByteBuffer into, long[] meta);

    /** {codec, width, height, fps, chroma, colour}, or null without a connection. */
    static native int[] watchInfo(long watch);

    /** {cuts, silent_ms, again, why, failure}. */
    static native long[] watchState(long watch);

    static native void watchAskKeyFrame(long watch);

    /**
     * TRIAL ONLY: what the host said last about its text input field, {state, serial, dropped, x,
     * y, width, height} (state 0 nothing known, 1 a field, 2 a password field, 3 not in a field),
     * or null. The thread that calls watchNext.
     */
    static native int[] watchTextInput(long watch);

    static native void watchClose(long watch);

    /** The input for the paired host `peer` (its id, never empty); 0 as for watchOpen. */
    static native long inputOpen(String configDir, String peer, String address);

    static native void inputSend(long input, int[] events, int count);

    /**
     * TRIAL ONLY: text the input method (the XR keyboard) has committed, in order with the events:
     * cut into pieces of at most 256 characters and 1024 bytes, newline and tab as keys. Never logged.
     */
    static native void inputText(long input, String text);

    /**
     * TRIAL ONLY: what the input method is composing (empty clears it), with the selected part in
     * the string's own positions; only to a host that takes it (inputState's preedit). Never logged.
     */
    static native void inputPreedit(long input, String text, int selectionStart, int selectionEnd);

    /** inputScroll's source: a wheel of notches, or fingers on a touchpad. */
    static final int SCROLL_WHEEL = 1;
    static final int SCROLL_FINGER = 2;

    /**
     * TRIAL ONLY: a scroll that says what scrolled (ADR 0007, スクロールの種類と段階). dx, dy as for
     * INPUT_SCROLL; a wheel: v120 (120 to a notch, right and down); fingers: pixels of the picture
     * (right and down, not rounded). No phase: the host stops after 100 ms of nothing.
     */
    static native void inputScroll(long input, int dx, int dy, int source, int v120H, int v120V,
            double pixelsH, double pixelsV);

    /**
     * {cuts, open, answered, over, failure, text, scroll, preedit}; text 1 when the host said it
     * takes committed text, scroll the level of scrolls it takes (0: dx and dy only), preedit 1
     * when it takes composing text.
     */
    static native long[] inputState(long input);

    static native void inputClose(long input);

    /** Why the input's connection is over (the host refused it for good), or null while it is not. */
    static native String inputWhy(long input);

    static native String lastError(int which);

    /** TRIAL ONLY: how often quick acknowledgements were set on the picture's socket; -1 without. */
    static native long quickackRearms();
}

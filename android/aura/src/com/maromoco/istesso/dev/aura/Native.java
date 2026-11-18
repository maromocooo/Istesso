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

    /** Input events, three ints each: kind, a, b (istesso_client.h). */
    static final int INPUT_KEY = 1;
    static final int INPUT_POINTER_AT = 2;
    static final int INPUT_BUTTON = 3;
    static final int INPUT_SCROLL = 4;

    /** {id, name} of the host joined, or null: lastError(ERROR_PAIRING) on the same thread. */
    static native String[] pairJoin(String configDir, String address, String code, String name);

    static native long watchOpen(String configDir, String address);

    /**
     * Waits up to waitMs. NEXT_FRAME: the frame is at the start of `into` and meta holds
     * {length, seq, pts_ns, key_frame, discontinuity, whole_ns}. One thread calls this.
     */
    static native int watchNext(long watch, int waitMs, ByteBuffer into, long[] meta);

    /** {codec, width, height, fps, chroma, colour}, or null without a connection. */
    static native int[] watchInfo(long watch);

    /** {cuts, silent_ms, again, why, failure}. */
    static native long[] watchState(long watch);

    static native void watchAskKeyFrame(long watch);

    static native void watchClose(long watch);

    static native long inputOpen(String configDir, String peer, String address);

    static native void inputSend(long input, int[] events, int count);

    /** {cuts, open, answered, over, failure}. */
    static native long[] inputState(long input);

    static native void inputClose(long input);

    static native String lastError(int which);
}

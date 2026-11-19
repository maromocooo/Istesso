// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.8).
package com.maromoco.istesso.dev.aura;

import android.content.Context;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.SurfaceView;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;

/**
 * The picture's surface, which can also take text from the system's input method (the XR
 * keyboard) while typing is switched on (TRIAL ONLY; ADR 0007 "確定した文字"). The input method
 * converts on this device: what it is still composing is shown here only, and only what it has
 * committed goes to the host, as text. While typing is off this is the surface it always was, and
 * keys go to the host as keys.
 */
final class TypingSurface extends SurfaceView {
    /** What typing brings about. Called on the main thread. */
    interface Typed {
        /** Text the input method committed. */
        void text(String text);
        /**
         * What the input method is composing now ("" when nothing), and where its cursor is in
         * it (as setComposingText's newCursorPosition says it; the part the input method is
         * converting is not told through this interface).
         */
        void composing(String text, int cursor);
        /** A key the input method sends (Backspace, Enter, ...): as from a keyboard. */
        void key(KeyEvent event);
    }

    private final Typed typed;
    private boolean typing;
    /** A password field on the host: the input method is told so (no composing, no learning). */
    private boolean password;
    /** The composition is being cancelled: what the input method finishes is dropped. */
    private boolean cancelling;
    private Connection connection;

    TypingSurface(Context context, Typed typed) {
        super(context);
        this.typed = typed;
    }

    void setTyping(boolean on) {
        typing = on;
    }

    boolean typing() {
        return typing;
    }

    /** Whether the host's field is a password field; the caller restarts the input after a change. */
    void setPassword(boolean on) {
        password = on;
    }

    /** Whether something is being composed. */
    boolean composing() {
        return connection != null && !connection.composing.isEmpty();
    }

    /** Drops what is being composed (another field has the keyboard on the host). */
    void cancelComposing(android.view.inputmethod.InputMethodManager imm) {
        if (connection != null) {
            connection.composing = "";
        }
        typed.composing("", 0);
        cancelling = true;
        if (imm != null) {
            imm.restartInput(this);
        }
        cancelling = false;
    }

    /** Takes what is being composed as it stands (a click goes after it, as on the Mac). */
    void commitComposing(android.view.inputmethod.InputMethodManager imm) {
        if (connection == null || connection.composing.isEmpty()) {
            return;
        }
        String done = connection.composing;
        connection.composing = "";
        typed.composing("", 0);
        typed.text(done);
        cancelling = true;
        if (imm != null) {
            imm.restartInput(this);
        }
        cancelling = false;
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return typing;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        if (!typing) {
            return null;
        }
        // Several lines, and no action on Enter: the keyboard's Enter key is a key for the host's
        // application (a newline, a confirmation), never an action after which a keyboard may
        // close itself. A password field is one line, as such fields are.
        out.inputType = password ? InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD
                : InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE;
        out.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_ACTION_NONE | EditorInfo.IME_FLAG_NO_ENTER_ACTION;
        if (password) {
            out.imeOptions |= EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        }
        out.initialSelStart = 0;
        out.initialSelEnd = 0;
        connection = new Connection();
        return connection;
    }

    /** An input connection without a text of its own: the text lives in the host's application. */
    private final class Connection extends BaseInputConnection {
        private String composing = "";

        Connection() {
            super(TypingSurface.this, false);
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            composing = "";
            typed.composing("", 0);
            if (cancelling) {
                return true;
            }
            // Typing switched off (also when the panel goes to the background): the framework
            // finishes the old connection, and what was not converted yet is not sent.
            if (typing && text != null && text.length() > 0) {
                typed.text(text.toString());
            }
            return true;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            composing = text == null ? "" : text.toString();
            // newCursorPosition (InputConnection): above 0, after the end of the text (1: right
            // after it); otherwise from its start (0: at the start). Within the text here.
            int n = composing.length();
            int cursor = newCursorPosition > 0 ? n + newCursorPosition - 1 : newCursorPosition;
            typed.composing(composing, Math.max(0, Math.min(n, cursor)));
            return true;
        }

        @Override
        public boolean finishComposingText() {
            // What was being composed is taken as it stands.
            String done = composing;
            composing = "";
            typed.composing("", 0);
            if (typing && !cancelling && !done.isEmpty()) {
                typed.text(done);
            }
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            if (!typing) {
                return true;
            }
            // The text is the host application's: deleting before the cursor is Backspace there.
            for (int i = 0; i < Math.min(beforeLength, 64); i++) {
                press(KeyEvent.KEYCODE_DEL);
            }
            for (int i = 0; i < Math.min(afterLength, 64); i++) {
                press(KeyEvent.KEYCODE_FORWARD_DEL);
            }
            return true;
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
            return deleteSurroundingText(beforeLength, afterLength);
        }

        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            typed.key(event);
            return true;
        }

        @Override
        public boolean performEditorAction(int action) {
            press(KeyEvent.KEYCODE_ENTER);
            return true;
        }

        private void press(int code) {
            typed.key(new KeyEvent(KeyEvent.ACTION_DOWN, code));
            typed.key(new KeyEvent(KeyEvent.ACTION_UP, code));
        }
    }
}

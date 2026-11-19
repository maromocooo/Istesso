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
        /** What the input method is composing now ("" when nothing). Shown here, never sent. */
        void composing(String text);
        /** A key the input method sends (Backspace, Enter, ...): as from a keyboard. */
        void key(KeyEvent event);
    }

    private final Typed typed;
    private boolean typing;

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

    @Override
    public boolean onCheckIsTextEditor() {
        return typing;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        if (!typing) {
            return null;
        }
        out.inputType = InputType.TYPE_CLASS_TEXT;
        out.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_ACTION_NONE;
        out.initialSelStart = 0;
        out.initialSelEnd = 0;
        return new Connection();
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
            typed.composing("");
            if (text != null && text.length() > 0) {
                typed.text(text.toString());
            }
            return true;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            composing = text == null ? "" : text.toString();
            typed.composing(composing);
            return true;
        }

        @Override
        public boolean finishComposingText() {
            // What was being composed is taken as it stands.
            String done = composing;
            composing = "";
            typed.composing("");
            if (!done.isEmpty()) {
                typed.text(done);
            }
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
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

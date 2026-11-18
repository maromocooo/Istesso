// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.util.SparseIntArray;
import android.view.KeyEvent;

/**
 * Which key was pressed, as the host wants it: its Linux number (input-event-codes.h). Only the
 * key's position is sent; converting to text happens on the host (ADR 0007, ADR 0020 route A).
 *
 * <p>A key of a hardware keyboard carries the kernel's number already ({@link
 * KeyEvent#getScanCode()}). Only when it has none (a key made by software) is the Android key
 * code looked up here.
 */
final class KeyMap {
    private KeyMap() {}

    /** Keys the system acts on itself, which are never sent to the host. */
    static boolean forTheSystem(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_VOLUME_UP:
            case KeyEvent.KEYCODE_VOLUME_DOWN:
            case KeyEvent.KEYCODE_VOLUME_MUTE:
            case KeyEvent.KEYCODE_BRIGHTNESS_UP:
            case KeyEvent.KEYCODE_BRIGHTNESS_DOWN:
            case KeyEvent.KEYCODE_POWER:
            case KeyEvent.KEYCODE_HOME:
            case KeyEvent.KEYCODE_APP_SWITCH:
            case KeyEvent.KEYCODE_SLEEP:
            case KeyEvent.KEYCODE_WAKEUP:
            case KeyEvent.KEYCODE_ASSIST:
            case KeyEvent.KEYCODE_VOICE_ASSIST:
                return true;
            default:
                return false;
        }
    }

    /** The Linux number of the key, or 0 when it is not known. */
    static int linux(KeyEvent event) {
        int scan = event.getScanCode();
        if (scan > 0 && scan < 0x300) {
            return scan;
        }
        return BY_KEY_CODE.get(event.getKeyCode(), 0);
    }

    private static final SparseIntArray BY_KEY_CODE = new SparseIntArray();

    private static void put(int keyCode, int linux) {
        BY_KEY_CODE.put(keyCode, linux);
    }

    static {
        // Letters, in the order of the alphabet: KEY_A ... KEY_Z.
        int[] letters = {30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44};
        for (int i = 0; i < 26; i++) {
            put(KeyEvent.KEYCODE_A + i, letters[i]);
        }
        // Digits: KEY_1 is 2 ... KEY_9 is 10, KEY_0 is 11.
        put(KeyEvent.KEYCODE_0, 11);
        for (int i = 1; i <= 9; i++) {
            put(KeyEvent.KEYCODE_0 + i, 1 + i);
        }
        put(KeyEvent.KEYCODE_ESCAPE, 1);
        put(KeyEvent.KEYCODE_MINUS, 12);
        put(KeyEvent.KEYCODE_EQUALS, 13);
        put(KeyEvent.KEYCODE_DEL, 14);
        put(KeyEvent.KEYCODE_TAB, 15);
        put(KeyEvent.KEYCODE_LEFT_BRACKET, 26);
        put(KeyEvent.KEYCODE_RIGHT_BRACKET, 27);
        put(KeyEvent.KEYCODE_ENTER, 28);
        put(KeyEvent.KEYCODE_CTRL_LEFT, 29);
        put(KeyEvent.KEYCODE_SEMICOLON, 39);
        put(KeyEvent.KEYCODE_APOSTROPHE, 40);
        put(KeyEvent.KEYCODE_GRAVE, 41);
        put(KeyEvent.KEYCODE_SHIFT_LEFT, 42);
        put(KeyEvent.KEYCODE_BACKSLASH, 43);
        put(KeyEvent.KEYCODE_COMMA, 51);
        put(KeyEvent.KEYCODE_PERIOD, 52);
        put(KeyEvent.KEYCODE_SLASH, 53);
        put(KeyEvent.KEYCODE_SHIFT_RIGHT, 54);
        put(KeyEvent.KEYCODE_ALT_LEFT, 56);
        put(KeyEvent.KEYCODE_SPACE, 57);
        put(KeyEvent.KEYCODE_CAPS_LOCK, 58);
        for (int i = 0; i < 10; i++) {
            put(KeyEvent.KEYCODE_F1 + i, 59 + i);
        }
        put(KeyEvent.KEYCODE_F11, 87);
        put(KeyEvent.KEYCODE_F12, 88);
        put(KeyEvent.KEYCODE_ZENKAKU_HANKAKU, 85);
        put(KeyEvent.KEYCODE_RO, 89);
        put(KeyEvent.KEYCODE_HENKAN, 92);
        put(KeyEvent.KEYCODE_KATAKANA_HIRAGANA, 93);
        put(KeyEvent.KEYCODE_MUHENKAN, 94);
        put(KeyEvent.KEYCODE_CTRL_RIGHT, 97);
        put(KeyEvent.KEYCODE_ALT_RIGHT, 100);
        put(KeyEvent.KEYCODE_MOVE_HOME, 102);
        put(KeyEvent.KEYCODE_DPAD_UP, 103);
        put(KeyEvent.KEYCODE_PAGE_UP, 104);
        put(KeyEvent.KEYCODE_DPAD_LEFT, 105);
        put(KeyEvent.KEYCODE_DPAD_RIGHT, 106);
        put(KeyEvent.KEYCODE_MOVE_END, 107);
        put(KeyEvent.KEYCODE_DPAD_DOWN, 108);
        put(KeyEvent.KEYCODE_PAGE_DOWN, 109);
        put(KeyEvent.KEYCODE_INSERT, 110);
        put(KeyEvent.KEYCODE_FORWARD_DEL, 111);
        put(KeyEvent.KEYCODE_YEN, 124);
        put(KeyEvent.KEYCODE_META_LEFT, 125);
        put(KeyEvent.KEYCODE_META_RIGHT, 126);
    }
}

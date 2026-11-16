// Which keys go to the other side for what happens on this Mac's keyboard.
//
// Kept apart from the window so that it can be tested without one (keys-test.c): what arrives
// here is a key's number on macOS and the event's modifier flags, what comes out are Linux key
// numbers going down and up (linux/input-event-codes.h), as istesso_proto::InputEvent carries them.
//
// Two things make this more than a table:
// - macOS reports a modifier key by an event of its own when a person presses it. A program that
//   sends keys (a Stream Deck's hotkey button, a macro tool) often sends none: the key just comes
//   with the modifier's flag set. A modifier the flags name and nobody pressed is therefore lent
//   to the key: pressed before it and let go after it.
// - While Command is held macOS does not say when another key comes up again, so such a key is
//   pressed and let go at once.
#ifndef ISTESSO_KEYS_H
#define ISTESSO_KEYS_H

#include <stdbool.h>
#include <stddef.h>

// The modifier flags of an event, as AppKit numbers them (NSEventModifierFlag...).
#define KEYS_FLAG_SHIFT (1UL << 17)
#define KEYS_FLAG_CONTROL (1UL << 18)
#define KEYS_FLAG_OPTION (1UL << 19)
#define KEYS_FLAG_COMMAND (1UL << 20)
// The bits that say which of the two keys of a modifier is down (NX_DEVICE...KEYMASK).
#define KEYS_DEVICE_LEFT_CONTROL 0x0001UL
#define KEYS_DEVICE_LEFT_SHIFT 0x0002UL
#define KEYS_DEVICE_RIGHT_SHIFT 0x0004UL
#define KEYS_DEVICE_LEFT_COMMAND 0x0008UL
#define KEYS_DEVICE_RIGHT_COMMAND 0x0010UL
#define KEYS_DEVICE_LEFT_OPTION 0x0020UL
#define KEYS_DEVICE_RIGHT_OPTION 0x0040UL
#define KEYS_DEVICE_RIGHT_CONTROL 0x2000UL

// The most changes one event can cause: four lent modifiers down and up, and the key down and up.
#define KEYS_MOST 10

typedef struct {
    unsigned code; // Linux's number for the key
    bool pressed;
} KeyChange;

typedef struct {
    bool down[256]; // keys that are down on the other side, by Linux's number
    // The Command keys go over as Control (what Command does on a Mac is done with Control in
    // applications on Linux) or, when false, as the Super keys.
    bool command_is_control;
} Keys;

// Linux's number for a key, by macOS's number for the same place on the keyboard. 0 where there
// is no such key.
static const unsigned short keys_linux[128] = {
    [0x00] = 30,  [0x01] = 31,  [0x02] = 32,  [0x03] = 33,  [0x04] = 35,  [0x05] = 34,  [0x06] = 44,  [0x07] = 45,
    [0x08] = 46,  [0x09] = 47,  [0x0A] = 86,  [0x0B] = 48,  [0x0C] = 16,  [0x0D] = 17,  [0x0E] = 18,  [0x0F] = 19,
    [0x10] = 21,  [0x11] = 20,  [0x12] = 2,   [0x13] = 3,   [0x14] = 4,   [0x15] = 5,   [0x16] = 7,   [0x17] = 6,
    [0x18] = 13,  [0x19] = 10,  [0x1A] = 8,   [0x1B] = 12,  [0x1C] = 9,   [0x1D] = 11,  [0x1E] = 27,  [0x1F] = 24,
    [0x20] = 22,  [0x21] = 26,  [0x22] = 23,  [0x23] = 25,  [0x24] = 28,  [0x25] = 38,  [0x26] = 36,  [0x27] = 40,
    [0x28] = 37,  [0x29] = 39,  [0x2A] = 43,  [0x2B] = 51,  [0x2C] = 53,  [0x2D] = 49,  [0x2E] = 50,  [0x2F] = 52,
    [0x30] = 15,  [0x31] = 57,  [0x32] = 41,  [0x33] = 14,  [0x35] = 1,   [0x36] = 126, [0x37] = 125, [0x38] = 42,
    [0x39] = 58,  [0x3A] = 56,  [0x3B] = 29,  [0x3C] = 54,  [0x3D] = 100, [0x3E] = 97,  [0x40] = 187, [0x41] = 83,
    [0x43] = 55,  [0x45] = 78,  [0x47] = 69,  [0x4B] = 98,  [0x4C] = 96,  [0x4E] = 74,  [0x4F] = 188, [0x50] = 189,
    [0x51] = 117, [0x52] = 82,  [0x53] = 79,  [0x54] = 80,  [0x55] = 81,  [0x56] = 75,  [0x57] = 76,  [0x58] = 77,
    [0x59] = 71,  [0x5A] = 190, [0x5B] = 72,  [0x5C] = 73,  [0x5D] = 124, [0x5E] = 89,  [0x5F] = 95,  [0x60] = 63,
    [0x61] = 64,  [0x62] = 65,  [0x63] = 61,  [0x64] = 66,  [0x65] = 67,  [0x66] = 123, [0x67] = 87,  [0x68] = 122,
    [0x69] = 183, [0x6A] = 186, [0x6B] = 184, [0x6D] = 68,  [0x6F] = 88,  [0x71] = 185, [0x72] = 110, [0x73] = 102,
    [0x74] = 104, [0x75] = 111, [0x76] = 62,  [0x77] = 107, [0x78] = 60,  [0x79] = 109, [0x7A] = 59,  [0x7B] = 105,
    [0x7C] = 106, [0x7D] = 108, [0x7E] = 103,
};

// Linux's number for the key at this place of the Mac's keyboard, as it is sent.
static inline unsigned keys_code(const Keys *keys, unsigned short key) {
    if (key >= 128) {
        return 0;
    }
    if (keys->command_is_control && key == 0x37) {
        return 29; // left Command as left Control
    }
    if (keys->command_is_control && key == 0x36) {
        return 97; // right Command as right Control
    }
    return keys_linux[key];
}

// Notes that a key went down or up, when that is news, and puts it on the list.
static inline size_t keys_set(Keys *keys, unsigned code, bool pressed, KeyChange *out, size_t count) {
    if (code == 0 || code >= 256 || keys->down[code] == pressed) {
        return count;
    }
    keys->down[code] = pressed;
    out[count] = (KeyChange){code, pressed};
    return count + 1;
}

// A key went down while the event's flags say which modifiers are held. `out` has room for
// KEYS_MOST changes; returns how many there are.
static inline size_t keys_key_down(Keys *keys, unsigned short key, unsigned long flags, bool repeat, KeyChange *out) {
    // The modifiers the flags name, as their left and right keys on the other side.
    const struct {
        unsigned long flag;
        unsigned left, right;
    } modifiers[4] = {
        {KEYS_FLAG_SHIFT, 42, 54},
        {KEYS_FLAG_CONTROL, 29, 97},
        {KEYS_FLAG_OPTION, 56, 100},
        {KEYS_FLAG_COMMAND, keys->command_is_control ? 29U : 125U, keys->command_is_control ? 97U : 126U},
    };
    size_t count = 0;
    unsigned code = keys_code(keys, key);
    if (code == 0) {
        return 0;
    }
    bool command = (flags & KEYS_FLAG_COMMAND) != 0;
    if (repeat && !command) {
        // The application on the other side repeats a held key itself.
        return 0;
    }
    unsigned lent[4] = {0, 0, 0, 0};
    for (int i = 0; i < 4; i++) {
        if ((flags & modifiers[i].flag) && !keys->down[modifiers[i].left] && !keys->down[modifiers[i].right]) {
            lent[i] = modifiers[i].left;
            count = keys_set(keys, lent[i], true, out, count);
        }
    }
    count = keys_set(keys, code, true, out, count);
    if (command) {
        count = keys_set(keys, code, false, out, count);
    }
    for (int i = 3; i >= 0; i--) {
        if (lent[i] != 0) {
            count = keys_set(keys, lent[i], false, out, count);
        }
    }
    return count;
}

// A key came up.
static inline size_t keys_key_up(Keys *keys, unsigned short key, KeyChange *out) {
    return keys_set(keys, keys_code(keys, key), false, out, 0);
}

// A modifier key, or Caps Lock, changed: macOS says which key and what is held now.
static inline size_t keys_flags_changed(Keys *keys, unsigned short key, unsigned long flags, KeyChange *out) {
    unsigned long bit = 0;
    switch (key) {
        case 0x38: bit = KEYS_DEVICE_LEFT_SHIFT; break;
        case 0x3C: bit = KEYS_DEVICE_RIGHT_SHIFT; break;
        case 0x3B: bit = KEYS_DEVICE_LEFT_CONTROL; break;
        case 0x3E: bit = KEYS_DEVICE_RIGHT_CONTROL; break;
        case 0x3A: bit = KEYS_DEVICE_LEFT_OPTION; break;
        case 0x3D: bit = KEYS_DEVICE_RIGHT_OPTION; break;
        case 0x37: bit = KEYS_DEVICE_LEFT_COMMAND; break;
        case 0x36: bit = KEYS_DEVICE_RIGHT_COMMAND; break;
        default: break;
    }
    unsigned code = keys_code(keys, key);
    if (bit != 0) {
        return keys_set(keys, code, (flags & bit) != 0, out, 0);
    }
    if (key == 0x39) {
        // Caps Lock reports each switch once.
        size_t count = keys_set(keys, code, true, out, 0);
        return keys_set(keys, code, false, out, count);
    }
    return 0;
}

// The window is no longer the one that is typed into: no key stays down on the other side.
// `out` has room for 256 changes.
static inline size_t keys_let_go(Keys *keys, KeyChange *out) {
    size_t count = 0;
    for (unsigned code = 1; code < 256; code++) {
        count = keys_set(keys, code, false, out, count);
    }
    return count;
}

#endif

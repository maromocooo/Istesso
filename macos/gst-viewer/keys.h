// Which keys go to the other side for what happens on this Mac's keyboard.
//
// Kept apart from the window so that it can be tested without one (keys-test.c): what arrives
// here is a key's number on macOS and the event's modifier flags, what comes out are Linux key
// numbers going down and up (linux/input-event-codes.h), as istesso_proto::InputEvent carries them.
//
// Some things make this more than a table:
// - macOS reports a modifier key by an event of its own when a person presses it. A program that
//   sends keys (a Stream Deck's hotkey button, a macro tool) often sends none: the key just comes
//   with the modifier's flag set. A modifier the flags name and nobody pressed is therefore lent
//   to the key: pressed before it and let go after it.
// - While Command is held macOS does not say when another key comes up again, so such a key is
//   pressed and let go at once.
// - Which of the two keys of a modifier is down comes in bits of their own. An event that has the
//   modifier's flag and neither of those bits still means the modifier is down: it goes over as
//   its left key.
// - Every key, modifier, button and scroll that is sent first brings the modifiers over there in
//   line with the event's flags: one that is down over there and not held any more comes up; for a
//   modifier's own event, a button or a scroll, one that is held and not down over there goes down
//   (its left key; lent around the button or scroll when the window is not the one typed into).
//   A pointer's move only lets go (keys_releases()). An event of a modifier that was lost (it went
//   to another application, say) is made good by the next one of these.
// - When the window stops being the one typed into, every key down over there comes up
//   (keys_let_go()); when it is that window again, the modifiers held at that moment go down
//   (keys_fall_in_line()).
//
// And one thing is here because it is about the same keys: what is passed on while the picture is
// not the other side's picture of this moment (its connection was cut and is being made again).
// Nothing is pressed over there then: keys_hold_back() takes the presses out of what an event
// caused and leaves the releases. When the picture is back, keys_fall_in_line() brings the
// modifiers over there in line with what is held on this keyboard (docs/adr/0015-media-transport.md).
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

// The most changes one event can cause: eight modifier keys brought in line, four lent modifiers
// down and up, and the key down and up.
#define KEYS_MOST 18

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

// The modifiers as the other side has them, Shift, Control, Alt and Super, each with its left and
// right key, and whether the event's `flags` say that it is held. When Command goes over as
// Control the two are one modifier over there, held when either is.
typedef struct {
    bool held;
    unsigned left, right;
} KeysModifier;

static inline void keys_modifiers(const Keys *keys, unsigned long flags, KeysModifier out[4]) {
    bool command = (flags & KEYS_FLAG_COMMAND) != 0;
    out[0] = (KeysModifier){(flags & KEYS_FLAG_SHIFT) != 0, 42, 54};
    out[1] = (KeysModifier){(flags & KEYS_FLAG_CONTROL) != 0 || (command && keys->command_is_control), 29, 97};
    out[2] = (KeysModifier){(flags & KEYS_FLAG_OPTION) != 0, 56, 100};
    out[3] = (KeysModifier){command && !keys->command_is_control, 125, 126};
}

// Brings the modifiers over there in line with the event's `flags`, all but the one whose left key
// is `skip`: one that is down over there and not held comes up (whichever of its keys is down);
// with `press`, one that is held and has neither key down over there goes down by its left key.
// Only what is news is sent: nothing comes up over there that was not noted as down here. Adds
// to `out` from `count` (at most 8 changes) and returns the new count.
static inline size_t keys_line_up(Keys *keys, unsigned long flags, bool press, unsigned skip, KeyChange *out,
                                  size_t count) {
    KeysModifier modifiers[4];
    keys_modifiers(keys, flags, modifiers);
    for (int i = 0; i < 4; i++) {
        if (modifiers[i].left == skip) {
            continue;
        }
        if (!modifiers[i].held) {
            count = keys_set(keys, modifiers[i].left, false, out, count);
            count = keys_set(keys, modifiers[i].right, false, out, count);
        } else if (press && !keys->down[modifiers[i].left] && !keys->down[modifiers[i].right]) {
            count = keys_set(keys, modifiers[i].left, true, out, count);
        }
    }
    return count;
}

// Only the modifiers that are down over there and not held any more come up (whichever of their
// keys is down). Nothing goes down. Adds to `out` from `count` (at most 8 changes).
static inline size_t keys_releases(Keys *keys, unsigned long flags, KeyChange *out) {
    return keys_line_up(keys, flags, false, 0, out, 0);
}

// Lends the modifiers that `flags` hold and that have neither key down over there: their left keys
// go down, and are noted in `lent` (4 places) to be given back by keys_give_back().
static inline size_t keys_lend(Keys *keys, unsigned long flags, unsigned lent[4], KeyChange *out, size_t count) {
    KeysModifier modifiers[4];
    keys_modifiers(keys, flags, modifiers);
    for (int i = 0; i < 4; i++) {
        lent[i] = 0;
        if (modifiers[i].held && !keys->down[modifiers[i].left] && !keys->down[modifiers[i].right]) {
            lent[i] = modifiers[i].left;
            count = keys_set(keys, lent[i], true, out, count);
        }
    }
    return count;
}

// The lent modifiers come up again, in the reverse order.
static inline size_t keys_give_back(Keys *keys, const unsigned lent[4], KeyChange *out, size_t count) {
    for (int i = 3; i >= 0; i--) {
        if (lent[i] != 0) {
            count = keys_set(keys, lent[i], false, out, count);
        }
    }
    return count;
}

// A key went down while the event's flags say which modifiers are held. `out` has room for
// KEYS_MOST changes; returns how many there are. A modifier that is down over there and not held
// any more comes up first; one that is held and not down over there is lent to the key.
static inline size_t keys_key_down(Keys *keys, unsigned short key, unsigned long flags, bool repeat, KeyChange *out) {
    size_t count = keys_releases(keys, flags, out);
    unsigned code = keys_code(keys, key);
    if (code == 0) {
        return count;
    }
    bool command = (flags & KEYS_FLAG_COMMAND) != 0;
    if (repeat && !command) {
        // The application on the other side repeats a held key itself.
        return count;
    }
    unsigned lent[4];
    count = keys_lend(keys, flags, lent, out, count);
    count = keys_set(keys, code, true, out, count);
    if (command) {
        count = keys_set(keys, code, false, out, count);
    }
    return keys_give_back(keys, lent, out, count);
}

// Whether the key itself went down among what a key press caused: a press to count as one.
// Modifiers that came up or were lent around it do not count by themselves.
static inline bool keys_presses_key(const Keys *keys, unsigned short key, const KeyChange *out, size_t count) {
    unsigned code = keys_code(keys, key);
    for (size_t i = 0; i < count; i++) {
        if (code != 0 && out[i].code == code && out[i].pressed) {
            return true;
        }
    }
    return false;
}

// Around a button or a scroll. The modifiers over there are brought in line with the event's
// flags. One that is down over there and not held comes up before. One that is held and not down
// over there:
// - when this window is the one typed into (`typed_into`: the key window of the active
//   application), goes down before and stays down: its release comes to this window;
// - when it is not (macOS sends scrolls, and clicks that also activate, to the window under the
//   pointer), is lent: down before, up after, because its release would go to another
//   application and nothing would bring it up over there.
// With `held_back` (the picture is being reconnected) nothing goes down: only releases.
// The changes to send before the button or scroll are the first `*before` of `out`; the rest are
// sent after it. Returns how many there are in all (at most 16).
static inline size_t keys_around_pointer(Keys *keys, unsigned long flags, bool typed_into, bool held_back, KeyChange *out,
                                         size_t *before) {
    size_t count = keys_releases(keys, flags, out);
    if (held_back) {
        *before = count;
        return count;
    }
    if (typed_into) {
        count = keys_line_up(keys, flags, true, 0, out, count);
        *before = count;
        return count;
    }
    unsigned lent[4];
    count = keys_lend(keys, flags, lent, out, count);
    *before = count;
    return keys_give_back(keys, lent, out, count);
}

// A key came up. A modifier that is down over there and not held any more comes up first.
static inline size_t keys_key_up(Keys *keys, unsigned short key, unsigned long flags, KeyChange *out) {
    size_t count = keys_releases(keys, flags, out);
    return keys_set(keys, keys_code(keys, key), false, out, count);
}

// A modifier key, or Caps Lock, changed: macOS says which key and what is held now. The other
// modifiers are brought in line with the flags first.
static inline size_t keys_flags_changed(Keys *keys, unsigned short key, unsigned long flags, KeyChange *out) {
    unsigned long bit = 0, both = 0;
    int which = -1; // which of keys_modifiers() the key is
    switch (key) {
        case 0x38: bit = KEYS_DEVICE_LEFT_SHIFT; which = 0; break;
        case 0x3C: bit = KEYS_DEVICE_RIGHT_SHIFT; which = 0; break;
        case 0x3B: bit = KEYS_DEVICE_LEFT_CONTROL; which = 1; break;
        case 0x3E: bit = KEYS_DEVICE_RIGHT_CONTROL; which = 1; break;
        case 0x3A: bit = KEYS_DEVICE_LEFT_OPTION; which = 2; break;
        case 0x3D: bit = KEYS_DEVICE_RIGHT_OPTION; which = 2; break;
        case 0x37: bit = KEYS_DEVICE_LEFT_COMMAND; which = keys->command_is_control ? 1 : 3; break;
        case 0x36: bit = KEYS_DEVICE_RIGHT_COMMAND; which = keys->command_is_control ? 1 : 3; break;
        default: break;
    }
    switch (key) {
        case 0x38: case 0x3C: both = KEYS_DEVICE_LEFT_SHIFT | KEYS_DEVICE_RIGHT_SHIFT; break;
        case 0x3B: case 0x3E: both = KEYS_DEVICE_LEFT_CONTROL | KEYS_DEVICE_RIGHT_CONTROL; break;
        case 0x3A: case 0x3D: both = KEYS_DEVICE_LEFT_OPTION | KEYS_DEVICE_RIGHT_OPTION; break;
        case 0x37: case 0x36: both = KEYS_DEVICE_LEFT_COMMAND | KEYS_DEVICE_RIGHT_COMMAND; break;
        default: break;
    }
    unsigned code = keys_code(keys, key);
    if (which < 0) {
        size_t count = keys_line_up(keys, flags, true, 0, out, 0);
        if (key == 0x39) {
            // Caps Lock reports each switch once.
            count = keys_set(keys, code, true, out, count);
            count = keys_set(keys, code, false, out, count);
        }
        return count;
    }
    KeysModifier modifiers[4];
    keys_modifiers(keys, flags, modifiers);
    KeysModifier modifier = modifiers[which];
    size_t count = keys_line_up(keys, flags, true, modifier.left, out, 0);
    if ((flags & both) != 0) {
        // The bits say which of the two keys is down.
        bool pressed = (flags & bit) != 0;
        unsigned other = code == modifier.left ? modifier.right : modifier.left;
        if (!pressed && modifier.held && !keys->down[other]) {
            // The flags still have the modifier held (by the other key of it, or by Control when
            // Command goes over as Control) and nothing else of it is down over there: it stays.
            return count;
        }
        return keys_set(keys, code, pressed, out, count);
    }
    // Neither bit: the flag alone says whether the modifier is held, and it is the left key.
    if (modifier.held) {
        if (!keys->down[modifier.left] && !keys->down[modifier.right]) {
            count = keys_set(keys, modifier.left, true, out, count);
        }
        return count;
    }
    count = keys_set(keys, modifier.left, false, out, count);
    return keys_set(keys, modifier.right, false, out, count);
}

// The picture is not the other side's of this moment: of the `count` changes an event caused,
// only letting go is passed on. A key that was to go down stays up over there, and is noted as
// up here, so that its release is no news later; a key that was lent to such a press (down and
// up within the same event) is not let go either, having never gone down. Returns how many
// changes are left in `out`; `*dropped` is set when a press was among those taken out.
static inline size_t keys_hold_back(Keys *keys, KeyChange *out, size_t count, bool *dropped) {
    unsigned taken[KEYS_MOST];
    size_t takens = 0, left = 0;
    for (size_t i = 0; i < count; i++) {
        bool was_taken = false;
        for (size_t j = 0; j < takens; j++) {
            was_taken = was_taken || taken[j] == out[i].code;
        }
        if (out[i].pressed) {
            if (out[i].code < 256) {
                keys->down[out[i].code] = false;
            }
            if (!was_taken && takens < KEYS_MOST) {
                taken[takens++] = out[i].code;
            }
            *dropped = true;
        } else if (!was_taken) {
            out[left++] = out[i];
        }
    }
    return left;
}

// The modifiers on the other side are brought in line with the ones held on this keyboard now
// (`flags` as an event has them): one that is held and is not down over there goes down (its left
// key), one that is down over there and is not held comes up. Other keys are left as they are:
// pressing a letter again would type it. Called when the picture is back and when the window is
// the one typed into again, both only while it is the one typed into. `out` has room for 8 changes.
// With `match_sides` (the everyday trial's reconnection), a held modifier whose side the flags name
// goes down on that side, so that its later release (flagsChanged names the side) lets go of the
// key that is down over there.
static inline size_t keys_fall_in_line(Keys *keys, unsigned long flags, bool match_sides, KeyChange *out) {
    if (!match_sides) {
        return keys_line_up(keys, flags, true, 0, out, 0);
    }
    bool as_control = keys->command_is_control;
    const struct {
        unsigned long left_flag, right_flag;
    } sides[4] = {
        {KEYS_DEVICE_LEFT_SHIFT, KEYS_DEVICE_RIGHT_SHIFT},
        {KEYS_DEVICE_LEFT_CONTROL | (as_control ? KEYS_DEVICE_LEFT_COMMAND : 0),
         KEYS_DEVICE_RIGHT_CONTROL | (as_control ? KEYS_DEVICE_RIGHT_COMMAND : 0)},
        {KEYS_DEVICE_LEFT_OPTION, KEYS_DEVICE_RIGHT_OPTION},
        {KEYS_DEVICE_LEFT_COMMAND, KEYS_DEVICE_RIGHT_COMMAND},
    };
    KeysModifier modifiers[4];
    keys_modifiers(keys, flags, modifiers);
    size_t count = 0;
    for (int i = 0; i < 4; i++) {
        unsigned left = modifiers[i].left, right = modifiers[i].right;
        if (!modifiers[i].held) {
            count = keys_set(keys, left, false, out, count);
            count = keys_set(keys, right, false, out, count);
        } else if (flags & (sides[i].left_flag | sides[i].right_flag)) {
            // Restore the same side its later flagsChanged release will name.
            count = keys_set(keys, left, (flags & sides[i].left_flag) != 0, out, count);
            count = keys_set(keys, right, (flags & sides[i].right_flag) != 0, out, count);
        } else if (!keys->down[left] && !keys->down[right]) {
            count = keys_set(keys, left, true, out, count);
        }
    }
    return count;
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

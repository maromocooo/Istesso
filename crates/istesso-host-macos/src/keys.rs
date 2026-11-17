//! From the key numbers input travels with to the Mac's own.
//!
//! Input names a key by the number Linux gives its place on the keyboard
//! (`istesso_proto::InputEvent` on the branch of ADR 0007), whatever the
//! sender runs on. The Mac wants its virtual key code for the same place.

/// The Mac's virtual key code for the key Linux numbers `code`. `None` for a
/// key the Mac has no place for.
pub fn virtual_key(code: u32) -> Option<u16> {
    let key = match code {
        1 => 0x35,   // Escape
        2 => 0x12,   // 1
        3 => 0x13,   // 2
        4 => 0x14,   // 3
        5 => 0x15,   // 4
        6 => 0x17,   // 5
        7 => 0x16,   // 6
        8 => 0x1a,   // 7
        9 => 0x1c,   // 8
        10 => 0x19,  // 9
        11 => 0x1d,  // 0
        12 => 0x1b,  // minus
        13 => 0x18,  // equal
        14 => 0x33,  // delete backwards
        15 => 0x30,  // tab
        16 => 0x0c,  // Q
        17 => 0x0d,  // W
        18 => 0x0e,  // E
        19 => 0x0f,  // R
        20 => 0x11,  // T
        21 => 0x10,  // Y
        22 => 0x20,  // U
        23 => 0x22,  // I
        24 => 0x1f,  // O
        25 => 0x23,  // P
        26 => 0x21,  // left bracket
        27 => 0x1e,  // right bracket
        28 => 0x24,  // return
        29 => 0x3b,  // left control
        30 => 0x00,  // A
        31 => 0x01,  // S
        32 => 0x02,  // D
        33 => 0x03,  // F
        34 => 0x05,  // G
        35 => 0x04,  // H
        36 => 0x26,  // J
        37 => 0x28,  // K
        38 => 0x25,  // L
        39 => 0x29,  // semicolon
        40 => 0x27,  // quote
        41 => 0x32,  // grave
        42 => 0x38,  // left shift
        43 => 0x2a,  // backslash
        44 => 0x06,  // Z
        45 => 0x07,  // X
        46 => 0x08,  // C
        47 => 0x09,  // V
        48 => 0x0b,  // B
        49 => 0x2d,  // N
        50 => 0x2e,  // M
        51 => 0x2b,  // comma
        52 => 0x2f,  // period
        53 => 0x2c,  // slash
        54 => 0x3c,  // right shift
        55 => 0x43,  // keypad multiply
        56 => 0x3a,  // left option
        57 => 0x31,  // space
        58 => 0x39,  // caps lock
        59 => 0x7a,  // F1
        60 => 0x78,  // F2
        61 => 0x63,  // F3
        62 => 0x76,  // F4
        63 => 0x60,  // F5
        64 => 0x61,  // F6
        65 => 0x62,  // F7
        66 => 0x64,  // F8
        67 => 0x65,  // F9
        68 => 0x6d,  // F10
        71 => 0x59,  // keypad 7
        72 => 0x5b,  // keypad 8
        73 => 0x5c,  // keypad 9
        74 => 0x4e,  // keypad minus
        75 => 0x56,  // keypad 4
        76 => 0x57,  // keypad 5
        77 => 0x58,  // keypad 6
        78 => 0x45,  // keypad plus
        79 => 0x53,  // keypad 1
        80 => 0x54,  // keypad 2
        81 => 0x55,  // keypad 3
        82 => 0x52,  // keypad 0
        83 => 0x41,  // keypad decimal
        86 => 0x0a,  // the key next to left shift on ISO keyboards
        87 => 0x67,  // F11
        88 => 0x6f,  // F12
        89 => 0x5e,  // JIS underscore (ro)
        96 => 0x4c,  // keypad enter
        97 => 0x3e,  // right control
        98 => 0x4b,  // keypad divide
        100 => 0x3d, // right option
        102 => 0x73, // home
        103 => 0x7e, // up
        104 => 0x74, // page up
        105 => 0x7b, // left
        106 => 0x7c, // right
        107 => 0x77, // end
        108 => 0x7d, // down
        109 => 0x79, // page down
        111 => 0x75, // delete forwards
        117 => 0x51, // keypad equals
        122 => 0x68, // JIS kana
        123 => 0x66, // JIS eisu
        124 => 0x5d, // JIS yen
        125 => 0x37, // left command
        126 => 0x36, // right command
        _ => return None,
    };
    Some(key)
}

/// The modifier a key is, as the bit the Mac's events carry for it.
pub fn modifier_flag(code: u32) -> Option<u64> {
    match code {
        42 | 54 => Some(FLAG_SHIFT),
        29 | 97 => Some(FLAG_CONTROL),
        56 | 100 => Some(FLAG_OPTION),
        125 | 126 => Some(FLAG_COMMAND),
        _ => None,
    }
}

pub const FLAG_SHIFT: u64 = 1 << 17;
pub const FLAG_CONTROL: u64 = 1 << 18;
pub const FLAG_OPTION: u64 = 1 << 19;
pub const FLAG_COMMAND: u64 = 1 << 20;

/// The left, right and middle buttons of the pointer, as input names them.
pub const BUTTON_LEFT: u32 = 0x110;
pub const BUTTON_RIGHT: u32 = 0x111;
pub const BUTTON_MIDDLE: u32 = 0x112;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_have_their_places() {
        assert_eq!(virtual_key(30), Some(0x00), "A");
        assert_eq!(virtual_key(16), Some(0x0c), "Q");
        assert_eq!(virtual_key(2), Some(0x12), "1");
        assert_eq!(virtual_key(57), Some(0x31), "space");
        assert_eq!(virtual_key(28), Some(0x24), "return");
    }

    #[test]
    fn no_two_keys_share_a_place() {
        let mut seen = std::collections::BTreeMap::new();
        for code in 0..256 {
            if let Some(key) = virtual_key(code) {
                if let Some(other) = seen.insert(key, code) {
                    panic!("keys {other} and {code} both map to {key:#x}");
                }
            }
        }
        assert!(seen.len() > 100);
    }

    #[test]
    fn unknown_keys_have_none() {
        assert_eq!(virtual_key(0), None);
        assert_eq!(virtual_key(240), None);
        assert_eq!(virtual_key(0x110), None, "a button is not a key");
    }

    #[test]
    fn every_modifier_is_also_a_key() {
        for code in [42, 54, 29, 97, 56, 100, 125, 126] {
            assert!(modifier_flag(code).is_some());
            assert!(virtual_key(code).is_some());
        }
        assert_eq!(modifier_flag(30), None);
    }
}

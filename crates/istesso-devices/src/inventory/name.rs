//! Privacy rules for the USB device descriptor's product string.

// Firmware and provisioning tools let users set descriptors on these vendors' devices.
const WRITABLE_VENDORS: &[u16] = &[
    0x1d6b, // Linux Foundation USB gadgets
    0x0525, // NetChip USB gadgets
    0x1209, // pid.codes projects
    0x16c0, // Shared V-USB firmware IDs
    0x2341, // Arduino
    0x2a03, // Arduino boards
    0x239a, // Adafruit
    0x2e8a, // Raspberry Pi microcontrollers
    0x303a, // Espressif
    0xcafe, // TinyUSB examples
    0xfeed, // Custom keyboard firmware
    0x1915, // Nordic Semiconductor
    0x0483, // STMicroelectronics
    0x0403, // FTDI bridges
    0x10c4, // Silicon Labs bridges
    0x1a86, // WCH bridges
];

fn hex_groups(text: &[u8], separator: u8, widths: &[usize]) -> bool {
    let length = widths.iter().sum::<usize>() + widths.len() - 1;
    text.windows(length).any(|window| {
        let mut groups = window.split(|&b| b == separator);
        widths.iter().all(|width| {
            groups.next().is_some_and(|group| {
                group.len() == *width && group.iter().all(u8::is_ascii_hexdigit)
            })
        })
    })
}

fn identifier(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(6).any(|s| s.iter().all(u8::is_ascii_digit))
        || bytes.split(|b| !b.is_ascii_hexdigit()).any(|run| {
            run.len() >= 8
                && run.iter().any(u8::is_ascii_digit)
                && run.iter().any(u8::is_ascii_alphabetic)
        })
        || hex_groups(bytes, b'-', &[8, 4, 4, 4, 12])
        || hex_groups(bytes, b':', &[2; 6])
        || hex_groups(bytes, b'-', &[2; 6])
        || hex_groups(bytes, b'.', &[4; 3])
}

pub(super) fn hidden(vendor: u16, text: &str) -> bool {
    if WRITABLE_VENDORS.contains(&vendor)
        || text.len() > 48
        || !text.bytes().all(|b| (b' '..=b'~').contains(&b))
    {
        return true;
    }
    // The printable ASCII check also hides typographic and modifier apostrophes.
    let lower = text.to_ascii_lowercase();
    ["'s", " von ", " de ", " di ", " du ", " van ", "@", "://"]
        .iter()
        .any(|marker| lower.contains(marker))
        || lower.match_indices("s'").any(|(i, _)| {
            !lower
                .as_bytes()
                .get(i + 2)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        })
        || identifier(text)
}

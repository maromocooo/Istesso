//! Lists what the Mac's screens can show. Needs no permission.
//!
//!     cargo run -p istesso-host-macos --example displays

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this lists the displays of a Mac and runs on macOS only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    let all = std::env::args().any(|argument| argument == "--all");
    let displays = istesso_host_macos::displays::list();
    if displays.is_empty() {
        println!("no display (or no way to the window server from here)");
    }
    for (index, display) in displays.iter().enumerate() {
        println!(
            "display {}: {}{}{}, {} modes",
            index + 1,
            if display.built_in {
                "built in"
            } else {
                "external"
            },
            if display.main { ", main" } else { "" },
            if display.asleep { ", asleep" } else { "" },
            display.modes.len()
        );
        let line = |mode: &istesso_host_macos::displays::Mode| {
            format!(
                "{}x{} pixels ({}x{} points) at {:.2} Hz",
                mode.pixel_width, mode.pixel_height, mode.width, mode.height, mode.refresh
            )
        };
        if let Some(mode) = display.current() {
            println!("  now:     {}", line(mode));
        }
        if let Some(mode) = display.largest() {
            println!("  largest: {}", line(mode));
        }
        if all {
            for mode in &display.modes {
                println!(
                    "  {}{}",
                    line(mode),
                    if mode.current { "  <- now" } else { "" }
                );
            }
        }
    }
}

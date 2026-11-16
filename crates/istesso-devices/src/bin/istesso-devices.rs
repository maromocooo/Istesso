//! A small tool around the device framework, for checking an adapter against
//! the real device and for making recordings. It will become part of the
//! `istesso` command.

use std::process::ExitCode;

use istesso_device_api::{ConfirmedBy, ReportKind};
use istesso_devices::{registry, udev};

const USAGE: &str = "\
Usage:
  istesso-devices adapters             the adapters that are built in, and what each declares
  istesso-devices udev-rules [GROUP]   udev rules for Linux, made from the declarations (default group: plugdev)
  istesso-devices find                 plugged-in devices that have an adapter
  istesso-devices check [OPTIONS]      talk to the first such device: connect, show pictures, listen
      --listen SECONDS                 how long to wait for buttons, dials and touches (default 0)
      --record FILE                    write the exchange down as a recording
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("adapters") => adapters(),
        Some("udev-rules") => udev_rules(args.get(1).map(String::as_str).unwrap_or("plugdev")),
        #[cfg(feature = "hid")]
        Some("find") => real::find(),
        #[cfg(feature = "hid")]
        Some("check") => real::check(&args[1..]),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn adapters() -> Result<ExitCode, String> {
    registry::check()?;
    let entries = registry::entries();
    if entries.is_empty() {
        println!("No adapter is built in.");
    }
    for (name, entry) in entries {
        let d = entry.declaration;
        println!("{} ({name})", d.name);
        for id in d.devices {
            println!("  device      {:04x}:{:04x}", id.vendor, id.product);
        }
        let confirmed = match d.confirmation.by {
            ConfirmedBy::Author => "confirmed by the author",
            ConfirmedBy::Contributor => "confirmed by the contributor",
            ConfirmedBy::NotYet => "not yet run against a real device",
        };
        if d.confirmation.by == ConfirmedBy::NotYet {
            println!("  confirmed   {confirmed}");
        } else {
            println!(
                "  confirmed   {confirmed}, {}, on {}",
                d.confirmation.date,
                d.confirmation.systems.join(", ")
            );
        }
        for software in d.other_software {
            println!("  also used by {}", software.name);
        }
        for app in d.used_by_apps {
            println!("  application {app}");
        }
        for rule in d.reports {
            let kind = match rule.kind {
                ReportKind::Input => "reads input  ",
                ReportKind::Output => "writes output",
                ReportKind::FeatureSet => "sets feature ",
                ReportKind::FeatureGet => "gets feature ",
            };
            match rule.command {
                Some(command) => {
                    println!("  {kind} {:#04x} {command:#04x}  {}", rule.id, rule.purpose)
                }
                None => println!("  {kind} {:#04x}       {}", rule.id, rule.purpose),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn udev_rules(group: &str) -> Result<ExitCode, String> {
    registry::check()?;
    print!("{}", udev::rules(&registry::entries(), group)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "hid")]
mod real {
    use std::process::ExitCode;
    use std::time::{Duration, Instant};

    use istesso_device_api::{Event, PictureTarget};
    use istesso_devices::hid::Hid;
    use istesso_devices::picture::Rgba;
    use istesso_devices::record::{event_text, HEADER};
    use istesso_devices::session::Session;
    use istesso_devices::{registry, software};

    pub fn find() -> Result<ExitCode, String> {
        registry::check()?;
        let hid = Hid::new()?;
        let found = hid.find(&registry::entries());
        if found.is_empty() {
            println!("No plugged-in device has an adapter.");
        }
        for device in found {
            println!(
                "{} ({:04x}:{:04x}), adapter {}",
                device.product, device.id.vendor, device.id.product, device.adapter
            );
            for name in software::running(device.entry.declaration.other_software) {
                println!("  in use by {name}: Istesso keeps its hands off while it runs");
            }
        }
        Ok(ExitCode::SUCCESS)
    }

    /// A colour that tells the buttons apart.
    fn colour(index: u8) -> [u8; 3] {
        const COLOURS: [[u8; 3]; 8] = [
            [220, 40, 40],
            [230, 140, 20],
            [220, 210, 30],
            [50, 170, 60],
            [30, 170, 190],
            [40, 80, 210],
            [140, 60, 200],
            [210, 60, 150],
        ];
        COLOURS[usize::from(index) % COLOURS.len()]
    }

    fn print_events<T: istesso_devices::transport::Transport>(session: &mut Session<T>) -> usize {
        let mut count = 0;
        while let Some(event) = session.next_event() {
            if !matches!(event, Event::Shape(_)) {
                count += 1;
            }
            println!("  {}", event_text(&event));
        }
        count
    }

    pub fn check(args: &[String]) -> Result<ExitCode, String> {
        let mut listen = 0u64;
        let mut record: Option<String> = None;
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--listen" => {
                    listen = rest
                        .next()
                        .and_then(|value| value.parse().ok())
                        .ok_or("--listen needs a number of seconds")?;
                }
                "--record" => {
                    record = Some(rest.next().ok_or("--record needs a file name")?.clone())
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }

        registry::check()?;
        let hid = Hid::new()?;
        let found = hid.find(&registry::entries());
        let Some(device) = found.first() else {
            println!("No plugged-in device has an adapter.");
            return Ok(ExitCode::from(3));
        };
        println!(
            "{} ({:04x}:{:04x}), adapter {}",
            device.product, device.id.vendor, device.id.product, device.adapter
        );

        let busy = software::running(device.entry.declaration.other_software);
        if !busy.is_empty() {
            println!(
                "In use by {}. Istesso does not touch the device while it runs.",
                busy.join(", ")
            );
            return Ok(ExitCode::from(4));
        }

        let transport = hid.open(device)?;
        let started = Instant::now();
        let mut session = Session::new(device.entry, transport);
        if record.is_some() {
            session.start_recording();
        }

        let shape = session
            .connect()
            .map_err(|error| format!("connecting: {error}"))?;
        println!("connected:");
        print_events(&mut session);

        session.set_elapsed_ms(started.elapsed().as_millis() as u64);
        session
            .brightness(60)
            .map_err(|error| format!("brightness: {error}"))?;
        println!("brightness set to 60");

        // A recording stays small: two buttons and the strip. Without one,
        // every button gets its colour.
        let buttons: Vec<u8> = if record.is_some() {
            vec![0, shape.buttons.saturating_sub(1)]
        } else {
            (0..shape.buttons).collect()
        };
        if shape.button_picture.is_some() {
            for index in &buttons {
                session.set_elapsed_ms(started.elapsed().as_millis() as u64);
                session
                    .show(
                        PictureTarget::Button(*index),
                        &Rgba::solid(64, 64, colour(*index)),
                    )
                    .map_err(|error| format!("picture on button {index}: {error}"))?;
            }
            println!("pictures shown on buttons {buttons:?}");
        }
        if shape.strip_picture.is_some() {
            // One band per dial, in the colour of the button above it.
            let bands = u32::from(shape.dials.max(1));
            let strip = Rgba::from_fn(bands * 8, 4, |x, _| colour((x / 8) as u8));
            session.set_elapsed_ms(started.elapsed().as_millis() as u64);
            session
                .show(PictureTarget::Strip, &strip)
                .map_err(|error| format!("picture on the strip: {error}"))?;
            println!("picture shown on the strip");
        }

        let mut heard = 0;
        if listen > 0 {
            println!("listening for {listen} s:");
            let until = Instant::now() + Duration::from_secs(listen);
            while Instant::now() < until {
                session.set_elapsed_ms(started.elapsed().as_millis() as u64);
                session
                    .poll(50)
                    .map_err(|error| format!("reading: {error}"))?;
                heard += print_events(&mut session);
            }
            println!("{heard} notices");
        }

        session.set_elapsed_ms(started.elapsed().as_millis() as u64);
        session.reset().map_err(|error| format!("reset: {error}"))?;
        println!("device put back as it was");
        println!(
            "reports stopped by the guard: {}; input reports dropped: {}",
            session.refused(),
            session.ignored_inputs()
        );

        if let Some(path) = record {
            let text = format!(
                "{HEADER}\nadapter {}\n{}",
                device.adapter,
                session.recording()
            );
            std::fs::write(&path, text).map_err(|error| format!("writing {path}: {error}"))?;
            println!("recording written to {path}");
        }
        Ok(ExitCode::SUCCESS)
    }
}

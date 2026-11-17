//! What the program is started with.
//!
//! As a runner it gets no arguments (ADR 0010): what it needs comes in the
//! `start` line, and the switches from the environment. For measuring it
//! takes the arguments of `scripts/m1-step3-host.py`, so that the tools
//! that started the script start this program in its place, and the
//! switches as arguments of the same names.
//!
//! The switches are the choices that may change latency (ADR 0019). All of
//! them are off unless asked for: a program started without any behaves as
//! the script that was measured at the gate.

use std::path::PathBuf;

use crate::encoder::{Chroma, Colour, Settings, Vbv};

/// Whether the threads on a frame's path are scheduled ahead of ordinary ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sched {
    #[default]
    Off,
    /// Real-time scheduling at a low priority. Needs the right to it.
    Rt,
    /// The same, and also for the thread the graphics driver runs in this
    /// process. A value of its own, so that it can be measured apart.
    RtDriver,
}

/// The longest the input's thread may be told to stay awake after an input,
/// in milliseconds. A thread that waits without sleeping keeps a processor
/// to itself, and with `sched rt` ahead of every ordinary thread: how long
/// it may do so is bounded here, by a constant, and nowhere by a setting.
pub const SPIN_AT_MOST_MS: u32 = 20;

/// Whether, and when, the machine is kept from sleeping deeply
/// (`/dev/cpu_dma_latency`). Needs the right to open that device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CpuLatency {
    #[default]
    Off,
    /// While a device is being shown the picture: no sleep that takes
    /// longer than `us` microseconds to wake from.
    Shown { us: i32 },
    /// For `ms` milliseconds after each input.
    Input { us: i32, ms: u32 },
}

/// The choices that may change latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    /// How often a second the last frame is sent again while the picture
    /// stands still and a device is shown it. 0: never.
    pub keep_awake: u32,
    /// How long after an input no frame is sent again, in milliseconds.
    pub keep_awake_hold: u32,
    pub sched: Sched,
    /// How long the input's thread stays awake after an input, in milliseconds.
    pub spin_after_input: u32,
    pub cpu_latency: CpuLatency,
}

impl Default for Switches {
    fn default() -> Self {
        Switches {
            keep_awake: 0,
            keep_awake_hold: 50,
            sched: Sched::Off,
            spin_after_input: 0,
            cpu_latency: CpuLatency::Off,
        }
    }
}

/// The names of the switches, as arguments (`--name`) and, in capitals with
/// `ISTESSO_HOST_` before them and `_` for `-`, in the environment.
pub const SWITCHES: [&str; 5] = [
    "keep-awake",
    "keep-awake-hold",
    "sched",
    "spin-after-input",
    "cpu-latency",
];

fn number<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("--{name} takes a number, not `{value}`"))
}

impl Switches {
    /// Sets one switch from its text.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        match name {
            "keep-awake" => {
                self.keep_awake = number(name, value)?;
                if self.keep_awake > 85 {
                    return Err("--keep-awake is at most 85 times a second".into());
                }
            }
            "keep-awake-hold" => self.keep_awake_hold = number(name, value)?,
            "sched" => {
                self.sched = match value {
                    "off" => Sched::Off,
                    "rt" => Sched::Rt,
                    "rt+driver" => Sched::RtDriver,
                    _ => return Err(format!("--sched is off, rt or rt+driver, not `{value}`")),
                }
            }
            "spin-after-input" => {
                self.spin_after_input = number(name, value)?;
                if self.spin_after_input > SPIN_AT_MOST_MS {
                    return Err(format!(
                        "--spin-after-input is at most {SPIN_AT_MOST_MS} milliseconds"
                    ));
                }
            }
            "cpu-latency" => {
                let parts: Vec<&str> = value.split(':').collect();
                self.cpu_latency = match parts.as_slice() {
                    ["off"] => CpuLatency::Off,
                    ["shown", us] => CpuLatency::Shown {
                        us: number(name, us)?,
                    },
                    ["input", us, ms] => CpuLatency::Input {
                        us: number(name, us)?,
                        ms: number(name, ms)?,
                    },
                    _ => {
                        return Err(format!(
                            "--cpu-latency is off, shown:MICROSECONDS or input:MICROSECONDS:MILLISECONDS, not `{value}`"
                        ))
                    }
                };
                if let CpuLatency::Shown { us } | CpuLatency::Input { us, .. } = self.cpu_latency {
                    if us < 0 {
                        return Err("--cpu-latency takes microseconds from 0 up".into());
                    }
                }
            }
            _ => return Err(format!("there is no switch called {name}")),
        }
        Ok(())
    }

    /// The switches as the environment has them (`get` reads one variable).
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut switches = Switches::default();
        for name in SWITCHES {
            let variable = format!("ISTESSO_HOST_{}", name.to_uppercase().replace('-', "_"));
            if let Some(value) = get(&variable).filter(|value| !value.is_empty()) {
                switches
                    .set(name, &value)
                    .map_err(|error| format!("{variable}: {error}"))?;
            }
        }
        Ok(switches)
    }

    /// The switches that are on, for the first line of the frame log. A
    /// measurement is read together with this.
    pub fn listed(&self) -> Vec<String> {
        let mut on = Vec::new();
        if self.keep_awake > 0 {
            on.push(format!("keep-awake={}", self.keep_awake));
            on.push(format!("keep-awake-hold={}", self.keep_awake_hold));
        }
        match self.sched {
            Sched::Off => {}
            Sched::Rt => on.push("sched=rt".to_string()),
            Sched::RtDriver => on.push("sched=rt+driver".to_string()),
        }
        if self.spin_after_input > 0 {
            on.push(format!("spin-after-input={}", self.spin_after_input));
        }
        match self.cpu_latency {
            CpuLatency::Off => {}
            CpuLatency::Shown { us } => on.push(format!("cpu-latency=shown:{us}")),
            CpuLatency::Input { us, ms } => on.push(format!("cpu-latency=input:{us}:{ms}")),
        }
        on
    }
}

/// What a measuring run is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    /// Where to listen for paired devices: an address with a port.
    pub bind: String,
    /// Where this device's identity and its list of paired devices are,
    /// when not where `istesso` keeps them.
    pub config_dir: Option<PathBuf>,
    /// The frame log to write; `None` for none.
    pub log: Option<PathBuf>,
    pub settings: Settings,
    /// Take input from the agent.
    pub input: bool,
    pub input_socket: Option<PathBuf>,
    /// Show the picture to nobody until told (`--admit nobody`).
    pub admit_nobody: bool,
    /// Leave the cursor out of the picture and tell the viewers what it looks like.
    pub cursor: bool,
    pub render_on_commit: bool,
    pub switches: Switches,
}

/// Reads the arguments of a measuring run: those of the script, and the switches.
pub fn measure(arguments: &[String]) -> Result<Measure, String> {
    let mut bind = None;
    let mut log = None;
    let mut run = Measure {
        bind: String::new(),
        config_dir: None,
        log: None,
        settings: Settings::decided(1720, 720, 85),
        input: true,
        input_socket: None,
        admit_nobody: false,
        cursor: false,
        render_on_commit: true,
        switches: Switches::default(),
    };
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let Some(name) = argument.strip_prefix("--") else {
            return Err(format!(
                "`{argument}` is not something this program is started with"
            ));
        };
        let (name, given) = match name.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (name, None),
        };
        // Those that are said or not said.
        let flag = match name {
            "input" => Some((&mut run.input, true)),
            "no-input" => Some((&mut run.input, false)),
            "cursor" => Some((&mut run.cursor, true)),
            "no-cursor" => Some((&mut run.cursor, false)),
            "render-on-commit" => Some((&mut run.render_on_commit, true)),
            "no-render-on-commit" => Some((&mut run.render_on_commit, false)),
            _ => None,
        };
        if let Some((flag, value)) = flag {
            *flag = value;
            continue;
        }
        if matches!(name, "key-requests" | "no-key-requests") {
            // Only the unencrypted stream listened for requests from anybody.
            continue;
        }
        let value = match given {
            Some(value) => value,
            None => arguments
                .next()
                .cloned()
                .ok_or(format!("--{name} needs a value"))?,
        };
        match name {
            "to-channel" => {
                // A port alone listens on every address, IPv4 and IPv6.
                let digits = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
                bind = Some(if digits {
                    format!("[::]:{value}")
                } else {
                    value
                });
            }
            "to" | "lose" => {
                return Err(format!(
                    "--{name} belongs to the unencrypted stream, which this program does not send; \
                     scripts/m1-step3-host.py still does"
                ))
            }
            // The library is part of this program.
            "media-lib" => {}
            "config-dir" => run.config_dir = Some(PathBuf::from(value)),
            "log" => log = Some(value),
            "render-node" => run.settings.render_node = Some(value),
            "size" => {
                let size = value
                    .split_once('x')
                    .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));
                let Some((width, height)) = size else {
                    return Err(format!("--size is WIDTHxHEIGHT, not `{value}`"));
                };
                (run.settings.width, run.settings.height) = (width, height);
            }
            "fps" => {
                run.settings.fps = number(name, &value)?;
                if run.settings.fps == 0 {
                    return Err("--fps is at least 1".into());
                }
            }
            "bitrate" => run.settings.bitrate = number(name, &value)?,
            "gop" => run.settings.gop = number(name, &value)?,
            "vbv" => {
                run.settings.vbv = match value.as_str() {
                    "frame" => Vbv::Frame,
                    "default" => Vbv::Default,
                    kbit => Vbv::Kbit(number(name, kbit)?),
                }
            }
            "chroma" => {
                run.settings.chroma = match value.as_str() {
                    "420" => Chroma::C420,
                    "444" => Chroma::C444,
                    _ => return Err(format!("--chroma is 420 or 444, not `{value}`")),
                }
            }
            "preset" => run.settings.preset = value,
            "colour" => {
                run.settings.colour = match value.as_str() {
                    "srgb" => Colour::Srgb,
                    "default" => Colour::Default,
                    _ => return Err(format!("--colour is srgb or default, not `{value}`")),
                }
            }
            "input-socket" => run.input_socket = Some(PathBuf::from(value)),
            "admit" => {
                run.admit_nobody = match value.as_str() {
                    "paired" => false,
                    "nobody" => true,
                    _ => return Err(format!("--admit is paired or nobody, not `{value}`")),
                }
            }
            "frames" => {
                if value != "cuda" {
                    return Err(format!(
                        "--frames {value}: this program keeps frames on the graphics card (cuda), \
                         as the gate verdict was measured"
                    ));
                }
            }
            name if SWITCHES.contains(&name) => run.switches.set(name, &value)?,
            _ => {
                return Err(format!(
                    "--{name} is not something this program is started with"
                ))
            }
        }
    }
    run.bind =
        bind.ok_or("--to-channel [ADDR:]PORT is needed: where to listen for paired devices")?;
    run.log = match log.ok_or("--log FILE is needed (`-` for none)")?.as_str() {
        "-" => None,
        file => Some(PathBuf::from(file)),
    };
    if run.settings.preset.split_whitespace().count() != 1 {
        return Err("--preset is one word".into());
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_arguments_of_the_gate_verdict_are_read_as_the_script_read_them() {
        let run = measure(&arguments(
            "--to-channel 47803 --frames cuda --chroma 444 --bitrate 30000 --gop -1 --vbv 353 \
             --size 3440x1440 --fps 85 --render-on-commit --cursor --log frames.jsonl",
        ))
        .unwrap();
        assert_eq!(run.bind, "[::]:47803");
        assert_eq!(run.log, Some(PathBuf::from("frames.jsonl")));
        assert!(run.cursor && run.render_on_commit && run.input && !run.admit_nobody);
        let mut decided = Settings::decided(3440, 1440, 85);
        decided.vbv = Vbv::Kbit(353);
        assert_eq!(run.settings, decided);
        // Nothing that may change latency is on unless it is asked for.
        assert_eq!(run.switches, Switches::default());
        assert!(run.switches.listed().is_empty());
    }

    #[test]
    fn what_is_not_said_is_as_the_script_has_it() {
        let run = measure(&arguments("--to-channel 192.0.2.7:5000 --log -")).unwrap(); // check-private: ok (a documentation address)
        assert_eq!(run.bind, "192.0.2.7:5000"); // check-private: ok (a documentation address)
        assert_eq!(run.log, None);
        assert_eq!(run.settings, Settings::decided(1720, 720, 85));
        assert!(run.input && !run.cursor && run.render_on_commit);
    }

    #[test]
    fn values_may_follow_an_equals_sign_and_flags_may_be_taken_back() {
        let run = measure(&arguments(
            "--to-channel=47803 --log=- --no-input --no-render-on-commit --no-cursor --admit=nobody \
             --input-socket=/run/x.sock --config-dir /tmp/c --vbv default --colour default --chroma 420 \
             --media-lib ignored.so --no-key-requests",
        ))
        .unwrap();
        assert!(!run.input && !run.render_on_commit && !run.cursor && run.admit_nobody);
        assert_eq!(run.input_socket, Some(PathBuf::from("/run/x.sock")));
        assert_eq!(run.config_dir, Some(PathBuf::from("/tmp/c")));
        assert_eq!(run.settings.vbv, Vbv::Default);
        assert_eq!(run.settings.colour, Colour::Default);
        assert_eq!(run.settings.chroma, Chroma::C420);
    }

    #[test]
    fn what_this_program_does_not_do_is_refused_with_the_reason() {
        let refused = |text: &str| measure(&arguments(text)).unwrap_err();
        assert!(refused("--to 192.0.2.7 --log -").contains("unencrypted")); // check-private: ok (a documentation address)
        assert!(refused("--to-channel 1 --log - --frames memory").contains("cuda"));
        assert!(refused("--log -").contains("--to-channel"));
        assert!(refused("--to-channel 1").contains("--log"));
        assert!(refused("--to-channel 1 --log - --dance").contains("--dance"));
        assert!(refused("--to-channel 1 --log - --size big").contains("WIDTHxHEIGHT"));
        assert!(refused("--to-channel 1 --log - --fps").contains("needs a value"));
        assert!(refused("--to-channel 1 --log - pause").contains("pause"));
    }

    #[test]
    fn the_switches_are_read_from_arguments_and_listed_for_the_log() {
        let run = measure(&arguments(
            "--to-channel 1 --log - --keep-awake 20 --sched rt --spin-after-input 2 --cpu-latency input:0:100",
        ))
        .unwrap();
        assert_eq!(
            run.switches,
            Switches {
                keep_awake: 20,
                keep_awake_hold: 50,
                sched: Sched::Rt,
                spin_after_input: 2,
                cpu_latency: CpuLatency::Input { us: 0, ms: 100 },
            }
        );
        assert_eq!(
            run.switches.listed(),
            [
                "keep-awake=20",
                "keep-awake-hold=50",
                "sched=rt",
                "spin-after-input=2",
                "cpu-latency=input:0:100"
            ]
        );
    }

    #[test]
    fn a_switch_that_is_not_understood_is_an_error_and_never_silently_off() {
        let mut switches = Switches::default();
        assert!(switches.set("sched", "fast").is_err());
        assert!(switches.set("cpu-latency", "shown").is_err());
        assert!(switches.set("cpu-latency", "shown:-1").is_err());
        assert!(switches.set("keep-awake", "200").is_err());
        assert!(switches.set("keep-awake", "often").is_err());
        assert!(switches.set("warp", "9").is_err());
        // A thread may be kept from sleeping for a bounded while only.
        assert!(switches.set("spin-after-input", "20").is_ok());
        assert!(switches.set("spin-after-input", "21").is_err());
        switches.set("sched", "rt+driver").unwrap();
        assert_eq!(switches.sched, Sched::RtDriver);
        assert!(switches.listed().contains(&"sched=rt+driver".to_string()));
        switches.set("cpu-latency", "shown:2").unwrap();
        assert_eq!(switches.cpu_latency, CpuLatency::Shown { us: 2 });
        switches.set("cpu-latency", "off").unwrap();
        assert_eq!(switches.cpu_latency, CpuLatency::Off);
    }

    #[test]
    fn a_runner_gets_its_switches_from_the_environment() {
        let environment = |name: &str| match name {
            "ISTESSO_HOST_KEEP_AWAKE" => Some("20".to_string()),
            "ISTESSO_HOST_KEEP_AWAKE_HOLD" => Some("40".to_string()),
            "ISTESSO_HOST_CPU_LATENCY" => Some("shown:0".to_string()),
            "ISTESSO_HOST_SCHED" => Some(String::new()),
            _ => None,
        };
        let switches = Switches::from_env(environment).unwrap();
        assert_eq!(switches.keep_awake, 20);
        assert_eq!(switches.keep_awake_hold, 40);
        assert_eq!(switches.cpu_latency, CpuLatency::Shown { us: 0 });
        assert_eq!(switches.sched, Sched::Off);
        assert_eq!(Switches::from_env(|_| None).unwrap(), Switches::default());
        let wrong =
            Switches::from_env(|name| (name == "ISTESSO_HOST_SCHED").then(|| "fast".into()));
        assert!(wrong.unwrap_err().contains("ISTESSO_HOST_SCHED"));
    }
}

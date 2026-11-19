# Istesso

Istesso shows apps that run on your other computers as ordinary windows on the one in front of you, over its own encrypted channel. Its one rule: if it feels remote, even slightly, it has failed.

<!-- Picture to come (docs/images/): part of one screen with three windows that look the same — an app
     running here, one on a Linux host, one on a Mac host — labeled only outside the picture.
     Istesso's own test app and made-up content only. -->

> [!IMPORTANT]
> Istesso is at the trial stage. The author uses it every day to find what is still missing, and much of that work lives on branches that are not in `main` yet. There are no binaries to download: you build it from source. What works today, and on which machines, is in the table below.

## What it is

Istesso is designed to put each piece of work where it costs the least delay, in this order:

1. What can run on the machine in front of you runs there. Only its state is kept in step with the others.
2. Text work goes over a direct terminal connection.
3. Only what is left — apps that need a screen and live on another machine — is relayed as video, one app to one window, with the pointer drawn on your side.

The rule above is checked in a form that can fail: you should not be able to tell which window is remote. A relayed window is measured against the same app running locally on the same screen, and a person tries to pick out the relayed one by eye. The rule covers machines on a wired home network; Wi-Fi and connections over the internet are outside it.

The name comes from *l'istesso tempo*, a direction in music: keep the same pace even when the meter changes.

## What works today

Each column says where the app runs and where you see it.

| What | Linux host → Mac | Mac host → Mac | Linux host → Android XR headset |
|---|---|---|---|
| Pair two machines with a one-time code | `Verified` 2026-11-16 ([record](docs/milestones.md#m0-共通の土台), Japanese) | `Trial` | `Trial` |
| List every machine and its apps in one list | `Verified` 2026-11-16 ([record](docs/milestones.md#m0-共通の土台), Japanese) | `Trial` | `Planned` |
| Open an app on a host and use it as one window: picture, keys, pointer, scrolling | `Trial` | `Trial` | `Trial` |
| Copy and paste text between that window and the machine in front of you | `Trial` | `Trial` | `Planned` |
| Type Japanese through an input method | `Trial` | `Trial` | `Trial` |
| Hear a host's sound where you are | `Planned` | `Planned` | `Planned` |
| A launcher window that opens an app wherever it is installed | `Planned` | `Planned` | `Planned` |
| Hand a USB device over to another machine, so that it appears plugged in there | `Out of scope` | `Out of scope` | `Out of scope` |

- `Verified`: run from `main`, on real machines in that role, with the steps a reader would use. It always carries the date and a link to the record.
- `Trial`: works in the author's trial build, which lives on branches. You cannot reproduce it from `main` yet.
- `Planned`: being designed, or not started.
- `Out of scope`: not part of this release.

The headset connects over Wi-Fi, which is outside the latency rule (that rule covers a wired home network). In its column, `Trial` means usable in the trial build; it says nothing about meeting the rule.

The latency bar, for a relayed window against the same app run locally on the same screen (both machines wired, an 85 Hz display, a 3440×1440 picture): no more than 12 ms added at the median, 24 ms at the 95th percentile and 47 ms at the 99th percentile. The relay passed the gate on 2026-11-17, measured with a prototype viewer and a host made for measuring; it is accepted only after the final verdict, measured with Istesso's own client and host ([gate record](docs/measurements/2026-11-17-m1-gate.md), Japanese).

## What comes later

- Istesso's own client and host in `main`, then the final latency verdict
- Sound from any host to one output you choose, for hours without a gap
- A launcher across machines, which can wake a host that is asleep
- Shared storage across machines, and a dedicated user that keeps Istesso's apps and agents away from your own files
- Running builds and jobs on the machine that suits them
- Using a device across machines without moving it: every machine's devices in one list, and an app that needs a device opens on the machine the device is plugged into, while you use the app's window from where you are. The first device driven directly is a programmable keypad. (Handing the USB device itself over to another machine is out of scope, as in the table.)
- The Android XR headset client in `main`
- Signed packages for macOS and Ubuntu, updates, and an external review of pairing
- Windows, after this release

Each step and the test it has to pass: [docs/milestones.md](docs/milestones.md) (Japanese).

## Known limits

- Istesso is not protected from other programs running as the same user on the same machine. Use it on your own machines, where only programs you trust run.
- The latency rule covers a wired home network. Wi-Fi and the internet are outside it ([notes on Wi-Fi](docs/wifi.md), Japanese).
- A Linux host needs an NVIDIA GPU with NVENC, its hardware video encoder.
- After a restart, a Mac host cannot be used until someone logs in on it once.
- When a Mac is a host, macOS asks on that Mac's screen every 30 days to confirm that the app may keep capturing the screen without the system's picker. This is Apple's rule for apps that capture the screen this way, not something Istesso adds, and builds from source are asked too. No signed build is distributed; how Istesso will be distributed is still open.
- There is no installer, no automatic update and no signed binary. Pairing has not had an external review.
- The latency figures so far come from a prototype viewer. Acceptance waits for the final verdict.
- Windows is not supported in this release.

## How it differs from similar tools

| Kind of tool | What it gives you | How Istesso is built | When that kind fits better |
|---|---|---|---|
| Remote desktop (VNC, RDP and the like) | The whole desktop of another machine, including its login screen, also over the internet | Each app becomes one window on your own desktop, next to your local windows and windows from other machines | You need a whole desktop, the login screen, use over the internet, or Windows |
| A terminal over ssh, with a terminal multiplexer | Text with the least delay, over any link, in sessions that outlive a dropped connection | Istesso does not replace it: text over a terminal is the second of its three layers, and video is kept for apps that need a screen | Your work is text |
| Window forwarding (X11 over ssh, for example) | Single windows of remote X11 apps on your screen, carried inside an ssh connection | Each window is sent as hardware-encoded video and measured against a local window | Your apps are X11 apps and an ssh connection is all you want to set up |
| Game streaming | Full screen, relative mouse movement, gamepads, play over the internet | Made for app windows: absolute pointer positions, the cursor drawn on your side. Games are outside this release | You want to play games |

## Quick start

These steps use one machine. They show what Istesso finds on it — its roles, the apps installed, the devices plugged in — and nothing more. Connecting two machines and showing a remote app are not in this README yet, because those steps have not been run again from a clean clone.

You need `git`, Rust 1.85 or later, and a C toolchain (on macOS, the Xcode Command Line Tools).

```sh
git clone https://github.com/maromocooo/Istesso.git
cd Istesso
cargo build --release --locked -p istesso-cli
./target/release/istesso doctor
./target/release/istesso apps list
./target/release/istesso devices list
```

- `doctor` prints this machine's roles: for example, whether it has a hardware video encoder, can build for Apple platforms, or suits being a host that is always on. It also prints the machine's name and network addresses, so look before you paste its output anywhere.
- `apps list` prints the apps installed on this machine.
- `devices list` prints the devices plugged into this machine.

Last run from a clean clone on 2026-11-19, on macOS 26.5 (Apple silicon) ([record](docs/readme-checks.md), Japanese).

## Build from source

- Rust 1.85 or later. It builds on macOS and Linux; CI builds and tests every change on both.
- No signed binaries are published, and how Istesso will be distributed has not been decided.

Before sending a change, run these (CI runs the same checks):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

`cargo deny check` needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny).

## Security

- Pairing: one machine shows a one-time code of eight characters, and you type it on the other. The two run a PAKE (SPAKE2) on the code, then exchange and pin each other's long-term keys ([ADR 0006](docs/adr/0006-pairing.md), Japanese).
- Every later connection authenticates both sides against the pinned keys and is encrypted, using the Noise protocol framework; the picture travels under the same keys ([ADR 0007](docs/adr/0007-control-channel.md), [ADR 0015](docs/adr/0015-media-transport.md), Japanese).
- A newly paired machine may only view and control. Each permission can be taken back on the machine that gave it.

Pairing has not had an external review yet; one is planned before a release. Istesso is not protected from other programs running as the same user on the same machine.

To report a vulnerability, use GitHub's private vulnerability reporting (the Security tab, "Report a vulnerability"). Please do not open a public issue for it.

## Documentation

The longer documents are in Japanese.

- [Requirements](docs/requirements.md) (Japanese)
- [Milestones and their acceptance tests](docs/milestones.md) (Japanese)
- [Architecture](docs/architecture.md) (Japanese)
- [Decisions (ADRs)](docs/adr/) (Japanese)
- [Measurements](docs/measurements/) (Japanese)
- [Components and their licenses](docs/components.md) (Japanese)

## Contributing

- The working rules for people and coding agents are in [AGENTS.md](AGENTS.md) (Japanese). Work on a branch, and add tests for new behavior.
- No GPL or AGPL code comes in. Before using outside code, check it against [docs/components.md](docs/components.md) (Japanese); before borrowing a specification or protocol, check its patent terms as well as its license. What is unclear is left out.
- A change to the video, input or sound path carries measurements from before and after it in the pull request, or says that it could not be measured.
- Pull requests that add device adapters are not accepted yet: the interface for them is not settled ([ADR 0012](docs/adr/0012-device-framework.md), Japanese).

## License

Istesso is available under the MIT License or the Apache License 2.0, whichever you choose: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).

## Author

[maromocooo](https://github.com/maromocooo)

## Trademarks

Apple, Mac, macOS and Xcode are trademarks of Apple Inc. Ubuntu is a trademark of Canonical Ltd. Linux is a registered trademark of Linus Torvalds. NVIDIA and NVENC are trademarks of NVIDIA Corporation. Android is a trademark of Google LLC. Rust is a trademark of the Rust Foundation. Istesso is not affiliated with or endorsed by any of them.

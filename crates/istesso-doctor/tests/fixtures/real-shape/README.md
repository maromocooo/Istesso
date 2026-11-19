# Capability fixtures

Real-shape output of the tools `istesso doctor` reads to say what a machine can do, taken on
2026-11-19 on three machines: the Air (`mac-*`, macOS 26 with Xcode 26.6), the host
(`linux-*`, Ubuntu with an NVIDIA GPU) and the Studio (`studio-*`, macOS with the
Command Line Tools only, no Xcode). The raw output was never written to a file: each machine
printed it into a pipe, and a sanitizer outside the repository wrote only these files. They were
checked with `scripts/check-private.sh --file` and against the machines' host and login names
and the names of the paired devices (in memory).

Each command has `<machine>-<name>.txt` (stdout, exact bytes after sanitizing) and, when the
command wrote anything to stderr, `<machine>-<name>.stderr.txt`. Exit status, as seen:

| Fixture | Command | Exit |
|---|---|---|
| `*-xcodebuild-version` | `xcodebuild -version` | 0 on the Air; 1 on the Studio (stdout empty, the error is on stderr) |
| `*-swift-version` | `swift --version` | 0; the version line is on stdout, `swift-driver version: …` on stderr |
| `*-cargo-version` | `cargo --version` | 0 |
| `*-node-version` | `node --version` | 0 |
| `*-npm-version` | `npm --version` | 0 |
| `*-python3-version` | `python3 --version` | 0 |
| `*-git-version` | `git --version` | 0 |
| `linux-docker-version` | `docker --version` | 0 |
| `linux-nvidia-smi-query` | `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits` | 0 |
| `*-adb-version` | `adb version` | 0 |
| `mac-adb-devices-l` | `adb devices -l` (the adb server was already running; no device attached) | 0 |
| `*-xcrun-find-simctl`, `*-xcrun-find-devicectl` | `xcrun --find simctl` / `devicectl` | 0 on the Air; 72 on the Studio (stdout empty) |
| `mac-devicectl-version` | `xcrun devicectl --version` | 0 |
| `mac-devicectl-list-devices.json` | `xcrun devicectl list devices --quiet --timeout 10 --json-output <path>` (the JSON file) | 0 |
| `mac-simctl-list-devices-available.json` | `xcrun simctl list devices available --json` | 0 |
| `mac-blender-app-version` | `plutil -extract CFBundleShortVersionString raw -o - /Applications/Blender.app/Contents/Info.plist` | 0 |
| `linux-blender-dir-listing` | `ls <folder of the blender binary>` for Blender unpacked from the release archive into `~/.local/opt/blender-<version>/` (the folder named like `5.3` is Blender's version) | 0 |

What was replaced:

- Home folders in paths: `/Users/user`, `/home/user`.
- `linux-nvidia-smi-query.txt`: the GPU name and memory size are made up; the driver version is real.
- Commit hashes in version lines (`cargo 1.x (hash date)`, `Docker version x, build hash`):
  made-up hex of the same length.
- devicectl: device names, identifiers, UDIDs, ECIDs, serial numbers, host names,
  `info.arguments`, `info.environment`, the `provider` value and dates are replaced. So that
  the file does not tell which devices the owner has, every per-device value is made up too,
  and the three real devices share no made-up value with any simulator:
  - real devices: names `Example Device 1`-`3`, models `Example Phone 1` / `Example Tablet 1`
    (the third has no model, as seen), `productType` `Example9n,1`, `hardwareModel` `X9nAP`,
    and a made-up `platform` and `deviceType`;
  - simulators: names `Example Simulator n`, models `Example Model n`, `productType`
    `Examplen,1`, `hardwareModel` `XnAP`;
  - for every device: OS versions and builds `99.0` / `99A100` (`originalComponentsCount` 2),
    CPU types `exampleCPU` with one supported CPU type, device families `[1]`, biometrics
    `[exampleBiometric]`, the same three capabilities `com.example.feature.1`-`3`, CPU counts,
    storage capacity and feature flags (`hasActionButton`, `supportsSiri`,
    `supportsCheckedAllocations`) set to the same value;
  - the real devices are listed after the simulators, and UUIDs, ECIDs and host names are
    numbered in that order.
  Kept: the key names and nesting of each entry, the simulators' platforms and kinds, the
  connection and boot enums (`reality`, `visibilityClass`, connection `state`,
  `transportType`, `pairingState`, `bootState`, …) and the deprecation notice. The list holds
  14 entries: 11 simulators (`reality: simulated`, `visibilityClass: simulators`) and 3 real
  devices; one real device has no `marketingName` and no `reality`. No real device was
  connected at the time: their connection `state` is `disconnected` or `unavailable`. The
  value `state` takes for a connected device was not seen.
- simctl: simulator names (`Example Simulator n`), UDIDs (also inside `dataPath` and `logPath`),
  `lastUsedAt`, device types (`…SimDeviceType.Example-Model-n`), runtime versions (the
  platform before the first `-` is real, the version after it is made up) and path sizes. Every simulator was `Shutdown`; the value for a running one was not seen.
- adb: no device was attached on any machine, so the shape of a device line was not seen.

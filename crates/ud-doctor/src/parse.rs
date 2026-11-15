//! Pure parsers for the text that operating systems print about themselves.
//!
//! Nothing here touches the machine, so every function can be tested on any
//! platform with captured samples.

use ud_proto::{AudioServer, Gpu, GpuVendor, NetInterface};

/// Parses `/etc/os-release` and returns `(name, version)`.
pub fn os_release(text: &str) -> Option<(String, String)> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "NAME" => name = Some(value),
            "VERSION_ID" => version = Some(value),
            _ => {}
        }
    }
    Some((name?, version.unwrap_or_default()))
}

/// Parses `/proc/meminfo` and returns total memory in bytes.
pub fn meminfo_total_bytes(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Returns the first CPU model name found in `/proc/cpuinfo`.
pub fn cpuinfo_model(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Parses the output of
/// `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits`.
pub fn nvidia_smi_csv(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(',').map(str::trim);
            let name = fields.next().filter(|n| !n.is_empty())?;
            let vram_mib: Option<u64> = fields.next().and_then(|v| v.parse().ok());
            let driver = fields.next().filter(|d| !d.is_empty()).map(str::to_string);
            Some(Gpu {
                vendor: GpuVendor::Nvidia,
                name: name.to_string(),
                vram_bytes: vram_mib.map(|m| m * 1024 * 1024),
                driver,
            })
        })
        .collect()
}

/// Picks display adapters out of `lspci` output.
pub fn lspci_gpus(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter(|l| {
            l.contains("VGA compatible controller")
                || l.contains("3D controller")
                || l.contains("Display controller")
        })
        .filter_map(|l| {
            // "01:00.0 VGA compatible controller: NVIDIA Corporation ... (rev a1)"
            let name = l.splitn(3, ':').nth(2)?.trim();
            let name = name.split(" (rev").next().unwrap_or(name).trim();
            if name.is_empty() {
                return None;
            }
            Some(Gpu {
                vendor: vendor_from_name(name),
                name: name.to_string(),
                vram_bytes: None,
                driver: None,
            })
        })
        .collect()
}

/// Guesses the GPU vendor from a product or vendor string.
pub fn vendor_from_name(name: &str) -> GpuVendor {
    let lower = name.to_lowercase();
    if lower.contains("nvidia") {
        GpuVendor::Nvidia
    } else if lower.contains("amd") || lower.contains("advanced micro") || lower.contains("radeon")
    {
        GpuVendor::Amd
    } else if lower.contains("intel") {
        GpuVendor::Intel
    } else if lower.contains("apple") {
        GpuVendor::Apple
    } else {
        GpuVendor::Other
    }
}

/// Parses `system_profiler SPDisplaysDataType -json`.
pub fn system_profiler_displays(json: &str) -> Vec<Gpu> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(items) = value.get("SPDisplaysDataType").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item
                .get("sppci_model")
                .or_else(|| item.get("_name"))
                .and_then(|v| v.as_str())?;
            let vendor_hint = item
                .get("spdisplays_vendor")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let vendor = match vendor_from_name(vendor_hint) {
                GpuVendor::Other => vendor_from_name(name),
                known => known,
            };
            Some(Gpu {
                vendor,
                name: name.to_string(),
                vram_bytes: None,
                driver: None,
            })
        })
        .collect()
}

/// Splits a dotted version into numbers; missing or odd parts count as zero.
fn version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

/// True when `version` is `major.minor` or newer.
pub fn version_at_least(version: &str, major: u32, minor: u32) -> bool {
    let parts = version_parts(version);
    let have_major = parts.first().copied().unwrap_or(0);
    let have_minor = parts.get(1).copied().unwrap_or(0);
    (have_major, have_minor) >= (major, minor)
}

/// Reads the sound server out of `pactl info`.
pub fn pactl_server(text: &str) -> AudioServer {
    let Some(line) = text
        .lines()
        .find(|l| l.trim_start().starts_with("Server Name:"))
    else {
        return AudioServer::Unknown;
    };
    let lower = line.to_lowercase();
    if lower.contains("pipewire") {
        AudioServer::PipeWire
    } else if lower.contains("pulseaudio") {
        AudioServer::PulseAudio
    } else {
        AudioServer::Unknown
    }
}

/// Returns the version from the first line of `xcodebuild -version`.
pub fn xcodebuild_version(text: &str) -> Option<String> {
    let first = text.lines().next()?.trim();
    let version = first.strip_prefix("Xcode ")?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// Parses `networksetup -listallhardwareports` on macOS.
pub fn networksetup_ports(text: &str) -> Vec<NetInterface> {
    let mut out = Vec::new();
    let mut device: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(d) = line.strip_prefix("Device:") {
            device = Some(d.trim().to_string());
        } else if let Some(mac) = line.strip_prefix("Ethernet Address:") {
            let mac = mac.trim();
            if let Some(name) = device.take() {
                if is_mac_address(mac) {
                    out.push(NetInterface {
                        name,
                        mac: mac.to_lowercase(),
                    });
                }
            }
        }
    }
    out
}

/// True for strings shaped like `aa:bb:cc:dd:ee:ff` that are not all zero.
pub fn is_mac_address(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
        && parts.iter().any(|p| *p != "00")
}

/// True when `pmset -g batt` reports an internal battery.
pub fn pmset_has_battery(text: &str) -> bool {
    text.contains("InternalBattery")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ubuntu_os_release() {
        let text = "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n";
        assert_eq!(
            os_release(text),
            Some(("Ubuntu".to_string(), "24.04".to_string()))
        );
    }

    #[test]
    fn os_release_without_name_is_none() {
        assert_eq!(os_release("ID=foo\n"), None);
    }

    #[test]
    fn reads_meminfo() {
        let text = "MemTotal:       65792104 kB\nMemFree:        1234 kB\n";
        assert_eq!(meminfo_total_bytes(text), Some(65_792_104 * 1024));
    }

    #[test]
    fn reads_cpu_model() {
        let text = "processor\t: 0\nmodel name\t: AMD Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(
            cpuinfo_model(text).as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
    }

    #[test]
    fn reads_nvidia_smi() {
        let gpus = nvidia_smi_csv("NVIDIA GeForce RTX 4090, 24564, 550.120\n");
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vendor, GpuVendor::Nvidia);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].vram_bytes, Some(24564 * 1024 * 1024));
        assert_eq!(gpus[0].driver.as_deref(), Some("550.120"));
    }

    #[test]
    fn nvidia_smi_ignores_empty_output() {
        assert!(nvidia_smi_csv("").is_empty());
        assert!(nvidia_smi_csv("\n").is_empty());
    }

    #[test]
    fn reads_lspci() {
        let text = "00:02.0 VGA compatible controller: Intel Corporation UHD Graphics 770 (rev 04)\n\
                    01:00.0 VGA compatible controller: NVIDIA Corporation GA102 [GeForce RTX 3080] (rev a1)\n\
                    02:00.0 Ethernet controller: Intel Corporation I225-V\n";
        let gpus = lspci_gpus(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].vendor, GpuVendor::Intel);
        assert_eq!(gpus[1].vendor, GpuVendor::Nvidia);
        assert_eq!(gpus[1].name, "NVIDIA Corporation GA102 [GeForce RTX 3080]");
    }

    #[test]
    fn reads_system_profiler() {
        let json = r#"{"SPDisplaysDataType":[{"_name":"Apple M4","sppci_model":"Apple M4","spdisplays_vendor":"sppci_vendor_Apple"}]}"#;
        let gpus = system_profiler_displays(json);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vendor, GpuVendor::Apple);
        assert_eq!(gpus[0].name, "Apple M4");
    }

    #[test]
    fn system_profiler_tolerates_garbage() {
        assert!(system_profiler_displays("not json").is_empty());
        assert!(system_profiler_displays("{}").is_empty());
    }

    #[test]
    fn compares_versions() {
        assert!(version_at_least("14.2", 14, 2));
        assert!(version_at_least("14.2.1", 14, 2));
        assert!(version_at_least("15.0", 14, 2));
        assert!(version_at_least("26.1", 14, 2));
        assert!(!version_at_least("14.1", 14, 2));
        assert!(!version_at_least("13.6.9", 14, 2));
        assert!(!version_at_least("", 14, 2));
    }

    #[test]
    fn detects_pipewire_behind_pulse_api() {
        let text = "Server String: /run/user/1000/pulse/native\nServer Name: PulseAudio (on PipeWire 1.0.5)\n";
        assert_eq!(pactl_server(text), AudioServer::PipeWire);
        assert_eq!(
            pactl_server("Server Name: pulseaudio\n"),
            AudioServer::PulseAudio
        );
        assert_eq!(pactl_server(""), AudioServer::Unknown);
    }

    #[test]
    fn reads_xcode_version() {
        assert_eq!(
            xcodebuild_version("Xcode 16.1\nBuild version 16B40\n").as_deref(),
            Some("16.1")
        );
        assert_eq!(xcodebuild_version("xcode-select: error"), None);
    }

    #[test]
    fn reads_networksetup() {
        let text =
            "\nHardware Port: Ethernet\nDevice: en0\nEthernet Address: A0:CE:C8:11:22:33\n\n\
                    Hardware Port: Wi-Fi\nDevice: en1\nEthernet Address: a0:ce:c8:44:55:66\n\n\
                    Hardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: N/A\n";
        let ports = networksetup_ports(text);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].name, "en0");
        assert_eq!(ports[0].mac, "a0:ce:c8:11:22:33");
        assert_eq!(ports[1].name, "en1");
    }

    #[test]
    fn validates_mac_addresses() {
        assert!(is_mac_address("aa:bb:cc:dd:ee:ff"));
        assert!(!is_mac_address("00:00:00:00:00:00"));
        assert!(!is_mac_address("N/A"));
        assert!(!is_mac_address("aa:bb:cc:dd:ee"));
    }

    #[test]
    fn detects_battery() {
        assert!(pmset_has_battery(
            "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1234)\t100%; charged;"
        ));
        assert!(!pmset_has_battery("Now drawing from 'AC Power'\n"));
    }
}

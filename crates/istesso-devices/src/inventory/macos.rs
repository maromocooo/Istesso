//! USB devices, interfaces and HID capabilities join on their physical location.
//! The ISP and Core Audio supply non-USB camera and audio inputs.

use super::{number_portless, portless, Capability, Device, Usb, UsbId};
use plist::{Dictionary, Value as Property};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

type Ports = BTreeMap<u32, Usb>;

fn port_path(location: u32) -> String {
    // The high byte is the bus. Each following nibble is one downstream port.
    let bus = location >> 24;
    let mut path = bus.to_string();
    for shift in [20, 16, 12, 8, 4, 0] {
        let port = (location >> shift) & 0xf;
        if port == 0 && location & ((1 << (shift + 4)) - 1) == 0 {
            break;
        }
        let separator = if shift == 20 { '-' } else { '.' };
        write!(path, "{separator}{port}").unwrap();
    }
    path
}

#[derive(Clone, Copy)]
enum Record {
    Device,
    Interface,
}

fn dictionaries(bytes: &[u8]) -> Result<Vec<Dictionary>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    plist::from_bytes(bytes).map_err(|_| "could not parse properties from ioreg".into())
}

fn registry_records(bytes: &[u8], record: Record, ports: &mut Ports) -> Result<(), String> {
    for props in dictionaries(bytes)? {
        let num = |key: &str| props.get(key)?.as_unsigned_integer();
        let Some(location) = num("locationID").and_then(|n| u32::try_from(n).ok()) else {
            continue;
        };
        let usb = ports.entry(location).or_default();
        match record {
            Record::Device => {
                if let (Some(vendor), Some(product)) = (
                    num("idVendor").and_then(|n| n.try_into().ok()),
                    num("idProduct").and_then(|n| n.try_into().ok()),
                ) {
                    usb.id = Some(UsbId { vendor, product });
                    usb.product_string = props
                        .get("kUSBProductString")
                        .and_then(Property::as_string)
                        .unwrap_or_default()
                        .into();
                }
                usb.interface(num("bDeviceClass").unwrap_or(0) as u8, 0, 0);
            }
            Record::Interface => usb.interface(
                num("bInterfaceClass").unwrap_or(0) as u8,
                num("bInterfaceSubClass").unwrap_or(0) as u8,
                num("bInterfaceProtocol").unwrap_or(0) as u8,
            ),
        }
    }
    Ok(())
}

fn hid_devices(bytes: &[u8], ports: &mut Ports) -> Result<Vec<Device>, String> {
    let mut portless_hid = BTreeMap::<(bool, u64), BTreeSet<Capability>>::new();
    for props in dictionaries(bytes)? {
        let num = |key: &str| props.get(key)?.as_unsigned_integer();
        let Some(location) = num("LocationID") else {
            continue;
        };
        let capability = match (num("PrimaryUsagePage"), num("PrimaryUsage")) {
            (Some(1), Some(2)) => Some(Capability::Mouse),
            (Some(1), Some(6)) => Some(Capability::Keyboard),
            _ => None,
        };
        let built_in = props.get("Built-In").and_then(Property::as_boolean);
        match props.get("Transport").and_then(Property::as_string) {
            Some("USB") => {
                if let Some(usb) = u32::try_from(location)
                    .ok()
                    .and_then(|id| ports.get_mut(&id))
                {
                    usb.built_in = built_in.or(usb.built_in);
                    usb.capabilities.extend(capability);
                }
            }
            Some("Bluetooth") => {
                // Multiple HID collections can share one opaque local location.
                portless_hid
                    .entry((false, location))
                    .or_default()
                    .extend(capability);
            }
            Some("FIFO") if built_in == Some(true) => {
                if let Some(kind) = capability {
                    portless_hid
                        .entry((true, location))
                        .or_default()
                        .insert(kind);
                }
            }
            _ => {}
        }
    }
    Ok(portless_hid
        .into_iter()
        .map(|((built_in, _), mut kinds)| {
            if kinds.is_empty() {
                kinds.insert(Capability::Other);
            }
            let label = kinds
                .iter()
                .map(|kind| kind.name())
                .collect::<Vec<_>>()
                .join("/");
            let model = if built_in {
                label
            } else {
                format!("Bluetooth {label}")
            };
            portless(&model, built_in, kinds)
        })
        .collect())
}

fn isp_camera(bytes: &[u8]) -> Result<bool, String> {
    Ok(dictionaries(bytes)?.iter().any(|isp| {
        isp.get("IORegistryEntryName").and_then(Property::as_string) == Some("isp")
            && isp
                .get("IORegistryEntryChildren")
                .and_then(Property::as_array)
                .into_iter()
                .flatten()
                .filter_map(Property::as_dictionary)
                .filter_map(|child| child.get("IOObjectClass")?.as_string())
                .filter_map(|class| class.strip_prefix("AppleH")?.strip_suffix("CamIn"))
                .any(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    }))
}

fn parse(report: &[u8], properties: [&[u8]; 3], isp: &[u8]) -> Result<Vec<Device>, String> {
    let report: Value =
        serde_json::from_slice(report).map_err(|_| "invalid system_profiler JSON")?;
    let audio = report["SPAudioDataType"]
        .as_array()
        .ok_or("missing audio report from system_profiler")?;
    let mut ports = Ports::new();
    let [usb, interfaces, hid] = properties;
    registry_records(usb, Record::Device, &mut ports)?;
    registry_records(interfaces, Record::Interface, &mut ports)?;
    let hid = hid_devices(hid, &mut ports)?;
    let mut devices: Vec<_> = ports
        .into_iter()
        .filter_map(|(port, usb)| usb.device(port_path(port)))
        .collect();

    devices.extend(hid);
    if isp_camera(isp)? {
        devices.push(portless("Camera", true, [Capability::Camera]));
    }
    for input in audio
        .iter()
        .filter_map(|group| group["_items"].as_array())
        .flatten()
    {
        if input["coreaudio_device_transport"] == "coreaudio_device_type_builtin"
            && input["coreaudio_device_input"]
                .as_u64()
                .is_some_and(|n| n > 0)
        {
            devices.push(portless("Microphone", true, [Capability::Audio]));
        }
    }
    number_portless(&mut devices);
    Ok(devices)
}

#[cfg(target_os = "macos")]
pub(super) fn read() -> Result<Vec<Device>, String> {
    fn run(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| output.stdout)
            .ok_or_else(|| format!("could not read device information from {program}"))
    }
    let profiler = run("system_profiler", &["SPAudioDataType", "-json"])?;
    let records = |class: &str| run("ioreg", &["-a", "-l", "-r", "-c", class, "-d", "1"]);
    parse(
        &profiler,
        [
            &records("IOUSBHostDevice")?,
            &records("IOUSBHostInterface")?,
            &records("IOHIDDevice")?,
        ],
        &run("ioreg", &["-a", "-l", "-r", "-n", "isp", "-d", "2"])?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &[u8] =
        include_bytes!("../../tests/fixtures/real-shape/mac-system_profiler.json");
    const USB: &[u8] = include_bytes!("../../tests/fixtures/real-shape/mac-ioreg-usb.plist");
    const INTERFACES: &[u8] =
        include_bytes!("../../tests/fixtures/real-shape/mac-ioreg-usb-interfaces.plist");
    const HID: &[u8] = include_bytes!("../../tests/fixtures/real-shape/mac-ioreg-hid.plist");
    const ISP: &[u8] = include_bytes!("../../tests/fixtures/real-shape/mac-ioreg-isp.plist");

    fn xml(records: &[Dictionary]) -> Vec<u8> {
        let mut bytes = Vec::new();
        plist::to_writer_xml(&mut bytes, &records).unwrap();
        bytes
    }

    #[test]
    fn labels_do_not_change_the_inventory() {
        let list = |report: &[u8]| parse(report, [USB, INTERFACES, HID], ISP).unwrap();
        let original = list(REPORT);
        let rows: Vec<_> = original.iter().map(ToString::to_string).collect();
        assert!(rows
            .iter()
            .any(|r| r == "Camera (built-in) | camera (use with apps on this PC)"));
        assert!(rows
            .iter()
            .any(|r| r == "Microphone (built-in) | audio (carry audio)"));
        assert!(rows
            .iter()
            .any(|r| r == "Bluetooth mouse (external) | mouse (forward input)"));
        assert_eq!(
            original.iter().filter(|d| d.model == "Microphone").count(),
            1
        );
        let mut localized: Value = serde_json::from_slice(REPORT).unwrap();
        for item in localized["SPAudioDataType"][0]["_items"]
            .as_array_mut()
            .unwrap()
        {
            item["_name"] = "Périphérique".into();
            if let Some(source) = item.get_mut("coreaudio_input_source") {
                *source = "Eingang".into();
            }
            if let Some(source) = item.get_mut("coreaudio_output_source") {
                *source = "Haut-parleur".into();
            }
        }
        for camera in localized["SPCameraDataType"].as_array_mut().unwrap() {
            camera["_name"] = "Kamera".into();
        }
        let translated = list(&serde_json::to_vec(&localized).unwrap());
        assert_eq!(
            serde_json::to_value(original).unwrap(),
            serde_json::to_value(&translated).unwrap()
        );
        assert_eq!(
            rows,
            translated
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn camera_requires_an_isp_camera_child() {
        let mut records = dictionaries(ISP).unwrap();
        let node = &mut records[0];
        let isp_class = node["IOObjectClass"].clone();
        let children = node
            .get_mut("IORegistryEntryChildren")
            .unwrap()
            .as_array_mut()
            .unwrap();
        children[0]
            .as_dictionary_mut()
            .unwrap()
            .insert("IOObjectClass".into(), isp_class);
        let devices = parse(REPORT, [USB, INTERFACES, HID], &xml(&records)).unwrap();
        assert!(!devices.iter().any(|d| d.model == "Camera"));
    }

    #[test]
    fn duplicate_wireless_mice_have_only_local_numbers() {
        let mut records = dictionaries(HID).unwrap();
        let mut second = records
            .iter()
            .find(|r| {
                r["Transport"].as_string() == Some("Bluetooth")
                    && r["PrimaryUsagePage"].as_unsigned_integer() == Some(1)
            })
            .unwrap()
            .clone();
        let location = second["LocationID"].as_unsigned_integer().unwrap();
        second.insert("LocationID".into(), (location + 1).into());
        records.push(second);
        let list =
            |records: &[Dictionary]| parse(REPORT, [USB, INTERFACES, &xml(records)], ISP).unwrap();
        let devices = list(&records);
        let mice: Vec<_> = devices
            .iter()
            .filter(|d| d.model == "Bluetooth mouse")
            .collect();
        assert_eq!(mice.len(), 2);
        for (index, mouse) in mice.into_iter().enumerate() {
            let number = index + 1;
            assert_eq!(
                mouse.to_string(),
                format!("Bluetooth mouse #{number} (external) | mouse (forward input)")
            );
            assert_eq!(
                serde_json::to_value(mouse).unwrap(),
                serde_json::json!({
                    "model":"Bluetooth mouse", "port":null, "usb":null, "name_hidden":false, "built_in":false,
                    "number":number, "kinds":[{"kind":"mouse","usage":"forward_input"}]
                })
            );
        }
        records.reverse();
        assert_eq!(
            serde_json::to_value(devices).unwrap(),
            serde_json::to_value(list(&records)).unwrap()
        );
    }

    #[test]
    fn changes_to_a_single_descriptor_follow_the_name_policy() {
        let records = dictionaries(USB).unwrap();
        let index = records
            .iter()
            .position(|r| {
                r.get("kUSBProductString").and_then(Property::as_string)
                    == Some("Example Product 13")
            })
            .unwrap();
        let model = |key: &str, value: Property| {
            let mut records = records.clone();
            *records[index].get_mut(key).unwrap() = value;
            let devices = parse(REPORT, [&xml(&records), INTERFACES, HID], ISP).unwrap();
            devices
                .into_iter()
                .find(|d| d.port.as_deref() == Some("128-1.4.1.1")) // check-private: ok (a USB bus-port path in a test, not an address)
                .unwrap()
        };
        for label in [
            "HD Pro Webcam C920",
            "Scarlett 2i2 4th Gen",
            "UR22C",
            "MX Keys",
            "Brio 4K",
            "Model 12345",
            "Model 1A2B3C4",
            "Model AACCEEAA",
            "Moonbeam",
        ] {
            assert_eq!(
                model("kUSBProductString", label.into()).model,
                format!("{label} (USB fffc:fefd)")
            );
        }
        for label in [
            "Example Toy's Camera", // check-private: ok (invented possessive)
            "Example Toy’s Camera", // check-private: ok (invented possessive)
            "Example Toyʼs Camera", // check-private: ok (invented possessive)
            "Examples' Mouse",      // check-private: ok (invented possessive)
            "EXAMPLES' MOUSE",      // check-private: ok (invented possessive)
            "空想のカメラ",
            "Example 123456",      // check-private: ok (invented digit run)
            "Example AB12CD34",    // check-private: ok (invented hex run)
            "test@example.com",    // check-private: ok (documentation domain)
            "https://example.com", // check-private: ok (documentation domain)
            "ab:cd:ef:ab:cd:ef",   // check-private: ok (synthetic MAC shape)
            "abcd.efab.cdef",      // check-private: ok (synthetic dotted MAC shape)
            "ab-cd-ef-ab-cd-ef",   // check-private: ok (synthetic MAC shape)
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", // check-private: ok (synthetic UUID shape)
            &"x".repeat(60),
        ] {
            let device = model("kUSBProductString", label.into());
            assert!(device.name_hidden);
            assert_eq!(device.model, "USB fffc:fefd (name hidden)");
        }
        for marker in ["von", "de", "di", "du", "van"] {
            assert!(
                model(
                    "kUSBProductString",
                    format!("Camera {marker} Example").into()
                )
                .name_hidden
            );
        }
        for vendor in [0x1209_u64, 0xfeed, 0x1d6b] {
            assert!(model("idVendor", vendor.into()).name_hidden);
        }
    }
}

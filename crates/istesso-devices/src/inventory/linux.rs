//! Collect a small allowlist of sysfs attributes, then interpret the snapshot.

use super::{number_portless, portless, Capability, Device, Usb, UsbId};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn decimal(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}

fn usb_port(s: &str) -> bool {
    let Some((bus, ports)) = s.split_once('-') else {
        return false;
    };
    let port_number = |s: &str| decimal(s) && !s.starts_with('0') && s.parse::<u8>().is_ok();
    decimal(bus) && !bus.starts_with('0') && ports.split('.').all(port_number)
}

fn attr(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

struct Descriptor {
    port: String,
    id: Option<UsbId>,
    product_string: String,
    built_in: Option<bool>,
    class: String,
    subclass: String,
    protocol: String,
}

struct ClassNode {
    class: &'static str,
    name: String,
    parent: PathBuf,
    firmware_backed: bool,
    index: String,
    keys: String,
    relative: String,
}

impl ClassNode {
    fn media_input(&self) -> Option<Capability> {
        match self.class {
            "video4linux" if self.index == "0" => decimal(&self.name).then_some(Capability::Camera),
            "sound" => {
                let (card, pcm) = self.name.split_once('D')?;
                if !decimal(pcm.strip_suffix('c')?) {
                    return None;
                }
                decimal(card).then_some(Capability::Audio)
            }
            _ => None,
        }
    }
}

struct Snapshot {
    descriptors: Vec<Descriptor>,
    nodes: Vec<ClassNode>,
}

fn descriptors(root: &Path) -> Result<Vec<Descriptor>, String> {
    let mut rows = Vec::new();
    let usb = fs::read_dir(root.join("bus/usb/devices"))
        .map_err(|_| "could not read the USB device list from sysfs")?;
    for entry in usb {
        let entry = entry.map_err(|_| "could not read a USB device entry")?;
        let dir = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let (port, prefix) = match name.split_once(':') {
            Some((port, _)) => (port, "bInterface"),
            None => (&*name, "bDevice"),
        };
        if !usb_port(port) {
            continue;
        }
        let (id, product_string) = if prefix == "bDevice" {
            let id = |key| u16::from_str_radix(&attr(&dir, key), 16).ok();
            let id = id("idVendor")
                .zip(id("idProduct"))
                .map(|(vendor, product)| UsbId { vendor, product });
            // sysfs appends one newline. Preserve any controls in the descriptor.
            let bytes = fs::read(dir.join("product")).unwrap_or_default();
            let product = String::from_utf8_lossy(bytes.strip_suffix(b"\n").unwrap_or(&bytes));
            (id, product.into_owned())
        } else {
            (None, String::new())
        };
        rows.push(Descriptor {
            port: port.to_owned(),
            id,
            product_string,
            built_in: if prefix == "bDevice" {
                match (
                    attr(&dir, "removable").as_str(),
                    attr(&dir, "port/connect_type").as_str(),
                ) {
                    ("fixed", _) | (_, "hardwired") => Some(true),
                    ("removable", _) | (_, "hotplug") => Some(false),
                    _ => None,
                }
            } else {
                None
            },
            class: attr(&dir, &format!("{prefix}Class")),
            subclass: attr(&dir, &format!("{prefix}SubClass")),
            protocol: attr(&dir, &format!("{prefix}Protocol")),
        });
    }
    Ok(rows)
}

fn collect(root: &Path) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot {
        descriptors: descriptors(root)?,
        nodes: Vec::new(),
    };
    let devices = root.join("devices");
    let platform_bus = devices.join("platform");
    for (class, prefix) in [
        ("input", "event"),
        ("video4linux", "video"),
        ("sound", "pcmC"),
    ] {
        let dir = root.join("class").join(class);
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(format!("could not read the {class} list from sysfs")),
        };
        for entry in entries {
            let entry = entry.map_err(|_| format!("could not read a {class} entry"))?;
            let node = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(index) = name.strip_prefix(prefix) else {
                continue;
            };
            // Some PCM class nodes have no device link. Their parent is the card.
            let parent = node
                .join("device")
                .canonicalize()
                .or_else(|e| node.canonicalize()?.parent().map(Path::to_owned).ok_or(e));
            let Ok(parent) = parent else {
                continue;
            };
            let Ok(path) = parent.strip_prefix(&devices) else {
                continue;
            };
            // A platform device created by a virtual camera or sound driver
            // can live outside /devices/virtual. Only count non-USB media on
            // hardware described by firmware or attached to a PCI bus.
            let firmware_backed = class != "input"
                && parent
                    .ancestors()
                    .take_while(|p| *p != devices && *p != platform_bus)
                    .any(|p| {
                        p.join("firmware_node").exists()
                            || p.join("of_node").exists()
                            || fs::read_link(p.join("subsystem"))
                                .ok()
                                .is_some_and(|s| s.ends_with("pci"))
                    });
            snapshot.nodes.push(ClassNode {
                class,
                name: index.into(),
                parent: path.into(),
                firmware_backed,
                index: if class == "video4linux" {
                    attr(&node, "index")
                } else {
                    String::new()
                },
                keys: if class == "input" {
                    attr(&parent, "capabilities/key")
                } else {
                    String::new()
                },
                relative: if class == "input" {
                    attr(&parent, "capabilities/rel")
                } else {
                    String::new()
                },
            });
        }
    }
    Ok(snapshot)
}

fn bit(bitmap: &str, index: usize) -> bool {
    let width = usize::BITS as usize;
    bitmap
        .split_whitespace()
        .rev()
        .nth(index / width)
        .and_then(|s| usize::from_str_radix(s, 16).ok())
        .is_some_and(|word| word & (1 << (index % width)) != 0)
}

fn parse(snapshot: Snapshot) -> Vec<Device> {
    let mut ports = BTreeMap::<String, Usb>::new();
    for desc in snapshot.descriptors {
        let usb = ports.entry(desc.port).or_default();
        if let Some(id) = desc.id {
            usb.id = Some(id);
            usb.product_string = desc.product_string;
            usb.built_in = desc.built_in;
        }
        if let Ok(class) = u8::from_str_radix(&desc.class, 16) {
            usb.interface(
                class,
                u8::from_str_radix(&desc.subclass, 16).unwrap_or(0),
                u8::from_str_radix(&desc.protocol, 16).unwrap_or(0),
            );
        }
    }
    let mut internal = BTreeSet::new();
    for node in snapshot.nodes {
        if node.parent.starts_with("virtual") {
            continue;
        }
        let port = node
            .parent
            .ancestors()
            .filter_map(|p| p.file_name()?.to_str())
            .find(|p| usb_port(p));
        if let Some(port) = port {
            let Some(usb) = ports.get_mut(port) else {
                continue;
            };
            if let Some(capability) = node.media_input() {
                usb.capabilities.insert(capability);
            }
            // KEY_A and KEY_ENTER, or BTN_LEFT with relative X and Y.
            if bit(&node.keys, 30) && bit(&node.keys, 28) {
                usb.capabilities.insert(Capability::Keyboard);
            }
            if bit(&node.keys, 272) && bit(&node.relative, 0) && bit(&node.relative, 1) {
                usb.capabilities.insert(Capability::Mouse);
            }
        } else if node.firmware_backed {
            let Some(capability) = node.media_input() else {
                continue;
            };
            // A card can have several capture nodes. A camera can have a metadata node.
            internal.insert((node.parent, capability));
        }
    }
    let mut devices: Vec<_> = ports
        .into_iter()
        .filter_map(|(port, usb)| usb.device(port))
        .collect();
    for (_, capability) in internal {
        let model = if capability == Capability::Camera {
            "Camera"
        } else {
            "Audio input"
        };
        devices.push(portless(model, true, [capability]));
    }
    number_portless(&mut devices);
    devices
}

pub(super) fn read(root: &Path) -> Result<Vec<Device>, String> {
    collect(root).map(parse)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn inventory_from_captured_sysfs_tree() {
        #[derive(serde::Deserialize)]
        struct Tree {
            dirs: Vec<PathBuf>,
            files: BTreeMap<PathBuf, String>,
            links: BTreeMap<PathBuf, PathBuf>,
        }
        let tree: Tree = serde_json::from_str(include_str!(
            "../../tests/fixtures/real-shape/linux-sysfs.json"
        ))
        .unwrap();
        struct Root(PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let dir = std::env::temp_dir().join(format!("inventory-sysfs-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let root = Root(dir.canonicalize().unwrap());
        for dir in tree.dirs {
            fs::create_dir_all(root.0.join(dir)).unwrap();
        }
        for (file, value) in tree.files {
            let path = root.0.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }
        for (link, target) in tree.links {
            let target = root.0.join(target);
            fs::create_dir_all(&target).unwrap();
            let link = root.0.join(link);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(target, link).unwrap();
        }

        let list = || read(&root.0).unwrap();
        let original = list();
        let row = |port: &str| {
            original
                .iter()
                .find(|d| d.port.as_deref() == Some(port))
                .unwrap()
        };
        assert_eq!(original.len(), 11);
        assert_eq!(row("1-5").to_string(), "Example Product 11 (USB fff5:fef3) at 1-5 (external) | audio (carry audio) | camera (use with apps on this PC)");
        assert_eq!(row("1-12").built_in, Some(true));
        assert_eq!(row("1-2.1.1").built_in, None);
        assert!(row("1-2.1.1")
            .kinds
            .iter()
            .any(|k| k.kind == Capability::Keyboard));
        assert_eq!(row("1-14").model, "USB fffc:fefa");
        assert!(original
            .iter()
            .all(|device| device.port.as_deref() != Some("1-1")));

        let fixture_product = root.0.join("bus/usb/devices/1-5/product");
        let descriptor = fs::read(&fixture_product).unwrap();
        for value in ["Example Camera\x07\n", "Example Camera\n\n"] {
            fs::write(&fixture_product, value).unwrap();
            assert!(
                list()
                    .iter()
                    .find(|d| d.port.as_deref() == Some("1-5"))
                    .unwrap()
                    .name_hidden
            );
        }
        fs::write(fixture_product, descriptor).unwrap();

        let video_label = root.0.join("class/video4linux/video0/name");
        fs::write(video_label, "Caméra intégrée\n").unwrap();
        assert_eq!(
            serde_json::to_value(list()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }

    #[test]
    fn usb_paths_exclude_other_buses_and_interface_nodes() {
        assert!(usb_port("12-10.2"));
        for path in ["1-0050", "usb2", "2-0", "2-.1", "2-3:1.1", "3-256"] {
            assert!(!usb_port(path));
        }
    }

    #[test]
    fn capability_bits_count_from_the_lowest_word_on_the_right() {
        let width = usize::BITS as usize;
        assert!(bit("2 1", width + 1));
        assert!(bit("2 1", 0));
        assert!(!bit("2 1", width));
        assert!(!bit("2 1", 2 * width));
        assert!(!bit("invalid", 0));
    }
}

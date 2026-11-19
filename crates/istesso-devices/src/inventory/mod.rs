//! Read-only inventory of the devices on this computer.

use crate::{api::ConfirmedBy, api::UsbId, registry};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;
mod name;

/// A row contains only a model, a physical location and information about use.
/// Free-form OS device names and persistent device identifiers are not exported.
#[derive(Debug, Serialize)]
pub struct Device {
    model: String,
    usb: Option<String>,
    name_hidden: bool,
    port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    number: Option<usize>,
    built_in: Option<bool>,
    kinds: Vec<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter: Option<Adapter>,
}

#[derive(Debug, Serialize)]
struct Adapter {
    confirmation: Confirmation,
    systems: &'static [&'static str],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Confirmation {
    Author,
    Contributor,
    NotYet,
}

impl Confirmation {
    fn description(&self) -> &'static str {
        match self {
            Self::Author => "confirmed by the author",
            Self::Contributor => "confirmed by the contributor",
            Self::NotYet => "not yet confirmed",
        }
    }
}

#[derive(Debug, Serialize)]
struct Kind {
    kind: Capability,
    usage: Usage,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Usage {
    ForwardInput,
    CarryAudio,
    LocalApps,
    DirectControl,
}

impl Usage {
    fn description(&self) -> &'static str {
        match self {
            Self::ForwardInput => "forward input",
            Self::CarryAudio => "carry audio",
            Self::LocalApps => "use with apps on this PC",
            Self::DirectControl => "direct control",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum Capability {
    Keyboard,
    Mouse,
    Audio,
    Camera,
    Adapter,
    Other,
}

impl Capability {
    fn name(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Audio => "audio",
            Self::Camera => "camera",
            Self::Adapter => "adapter",
            Self::Other => "other",
        }
    }

    fn kind(self) -> Kind {
        let usage = match self {
            Self::Keyboard | Self::Mouse => Usage::ForwardInput,
            Self::Audio => Usage::CarryAudio,
            Self::Camera | Self::Other => Usage::LocalApps,
            Self::Adapter => Usage::DirectControl,
        };
        Kind { kind: self, usage }
    }
}

#[derive(Default)]
struct Usb {
    id: Option<UsbId>,
    product_string: String,
    built_in: Option<bool>,
    hub: bool,
    capabilities: BTreeSet<Capability>,
}

impl Usb {
    fn interface(&mut self, class: u8, subclass: u8, protocol: u8) {
        let capability = match (class, subclass, protocol) {
            (9, _, _) => {
                self.hub = true;
                return;
            }
            (1, _, _) => Capability::Audio,
            (14, _, _) => Capability::Camera,
            (3, 1, 1) => Capability::Keyboard,
            (3, 1, 2) => Capability::Mouse,
            _ => return,
        };
        self.capabilities.insert(capability);
    }

    fn device(mut self, port: String) -> Option<Device> {
        if self.hub {
            return None;
        }
        let id = self.id?;
        let usb = format!("{:04x}:{:04x}", id.vendor, id.product);
        let (model, name_hidden, adapter) = if let Some((_, entry)) = registry::find(id) {
            self.capabilities.insert(Capability::Adapter);
            let declaration = entry.declaration;
            let adapter = Adapter {
                confirmation: match declaration.confirmation.by {
                    ConfirmedBy::Author => Confirmation::Author,
                    ConfirmedBy::Contributor => Confirmation::Contributor,
                    ConfirmedBy::NotYet => Confirmation::NotYet,
                },
                systems: declaration.confirmation.systems,
            };
            (
                format!("{} (USB {usb})", declaration.name),
                false,
                Some(adapter),
            )
        } else if self.product_string.is_empty() {
            (format!("USB {usb}"), false, None)
        } else if name::hidden(id.vendor, &self.product_string) {
            (format!("USB {usb} (name hidden)"), true, None)
        } else {
            let name = crate::text::printable(&self.product_string);
            (format!("{name} (USB {usb})"), false, None)
        };
        if self.capabilities.is_empty() {
            self.capabilities.insert(Capability::Other);
        }
        Some(Device {
            model,
            usb: Some(usb),
            name_hidden,
            port: Some(port),
            number: None,
            built_in: self.built_in,
            kinds: self
                .capabilities
                .into_iter()
                .map(Capability::kind)
                .collect(),
            adapter,
        })
    }
}

fn portless(
    model: &str,
    built_in: bool,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Device {
    Device {
        model: model.into(),
        usb: None,
        name_hidden: false,
        port: None,
        number: None,
        built_in: Some(built_in),
        kinds: capabilities.into_iter().map(Capability::kind).collect(),
        adapter: None,
    }
}

// Both collectors supply portless devices in stable, local order.
fn number_portless(devices: &mut [Device]) {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for device in devices.iter_mut().filter(|d| d.port.is_none()) {
        let kinds: Vec<_> = device.kinds.iter().map(|entry| entry.kind).collect();
        groups.entry(kinds).or_default().push(device);
    }
    for group in groups.into_values().filter(|g| g.len() > 1) {
        for (index, device) in group.into_iter().enumerate() {
            device.number = Some(index + 1);
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let attachment = match self.built_in {
            Some(true) => "built-in",
            Some(false) => "external",
            None => "unknown",
        };
        write!(f, "{}", self.model)?;
        if let Some(port) = &self.port {
            write!(f, " at {port}")?;
        }
        if let Some(number) = self.number {
            write!(f, " #{number}")?;
        }
        write!(f, " ({attachment})")?;
        for kind in &self.kinds {
            write!(f, " | {} ({})", kind.kind.name(), kind.usage.description())?;
        }
        if let Some(adapter) = &self.adapter {
            write!(f, " | {}", adapter.confirmation.description())?;
            if !adapter.systems.is_empty() {
                write!(f, " on {}", adapter.systems.join(", "))?;
            }
        }
        Ok(())
    }
}

pub fn local() -> Result<Vec<Device>, String> {
    #[cfg(target_os = "linux")]
    let devices = linux::read(std::path::Path::new("/sys"));
    #[cfg(target_os = "macos")]
    let devices = macos::read();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let devices = Err("local device inventory requires Linux or macOS".into());
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_control_uses_the_declaration_for_each_supported_model() {
        for (_, entry) in registry::entries() {
            let declaration = entry.declaration;
            for id in declaration.devices {
                let usb = Usb {
                    id: Some(*id),
                    ..Default::default()
                };
                let device = usb.device("2-1".into()).unwrap();
                assert_eq!(
                    device.model,
                    format!(
                        "{} (USB {:04x}:{:04x})",
                        declaration.name, id.vendor, id.product
                    )
                );
                assert!(!device.name_hidden);
                assert_eq!(device.kinds[0].usage.description(), "direct control");
                let json = serde_json::to_value(&device).unwrap();
                assert_eq!(json["kinds"][0]["kind"], "adapter");
                assert_eq!(json["kinds"][0]["usage"], "direct_control");
                assert_eq!(
                    json["adapter"]["confirmation"],
                    match declaration.confirmation.by {
                        ConfirmedBy::Author => "author",
                        ConfirmedBy::Contributor => "contributor",
                        ConfirmedBy::NotYet => "not_yet",
                    }
                );
                let adapter = device.adapter.unwrap();
                assert_eq!(adapter.systems, declaration.confirmation.systems);
                assert_eq!(
                    adapter.confirmation.description(),
                    match declaration.confirmation.by {
                        ConfirmedBy::Author => "confirmed by the author",
                        ConfirmedBy::Contributor => "confirmed by the contributor",
                        ConfirmedBy::NotYet => "not yet confirmed",
                    }
                );
            }
        }
    }
}

//! Finding paired devices on the local network with mDNS (DNS-SD).
//!
//! Every agent announces a `_istesso._tcp` service carrying the fingerprint of its
//! device, and listens for the announcements of others. When an announcement
//! names a paired device, the announced address is remembered as a place to
//! try.
//!
//! An announcement proves nothing; anyone on the network can send one. That
//! is fine because discovery only ever produces address hints: whoever
//! answers at a discovered address still has to pass the handshake of the
//! control channel.
//!
//! The fingerprint in the announcement is a stable identifier, visible to
//! everyone on the same network. On networks where that is unwanted the agent
//! can be started without discovery.

use std::net::{IpAddr, SocketAddr};

use istesso_pair::PeerStore;
use mdns_sd::{IfKind, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::cache;
use crate::paths::Paths;

/// DNS-SD service type of the control channel.
pub const SERVICE_TYPE: &str = "_istesso._tcp.local.";
/// TXT record key holding the device fingerprint.
const ID_KEY: &str = "id";

/// Keeps the announcement alive; dropping it ends discovery.
pub struct Discovery {
    _daemon: ServiceDaemon,
}

impl Discovery {
    /// Announces this device (`fingerprint`, listening on `port`) and starts
    /// collecting the addresses of paired devices.
    pub fn start(paths: Paths, fingerprint: &str, port: u16) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        // The control channel listens on IPv4 only for now.
        daemon
            .disable_interface(IfKind::IPv6)
            .map_err(|e| e.to_string())?;

        let host = format!("istesso-{}.local.", fingerprint.replace('-', ""));
        let properties = [(ID_KEY, fingerprint), ("v", "1")];
        let service = ServiceInfo::new(SERVICE_TYPE, fingerprint, &host, (), port, &properties[..])
            .map_err(|e| e.to_string())?
            .enable_addr_auto();
        daemon.register(service).map_err(|e| e.to_string())?;

        let events = daemon.browse(SERVICE_TYPE).map_err(|e| e.to_string())?;
        let own = fingerprint.to_string();
        std::thread::spawn(move || {
            while let Ok(event) = events.recv() {
                if let ServiceEvent::ServiceResolved(service) = event {
                    note(&paths, &own, &service);
                }
            }
        });
        Ok(Discovery { _daemon: daemon })
    }
}

/// Remembers where a paired device announced itself.
fn note(paths: &Paths, own: &str, service: &ResolvedService) {
    let Some(id) = service.get_property_val_str(ID_KEY) else {
        return;
    };
    if id == own {
        return;
    }
    // Only paired devices matter; everything else on the network is ignored.
    let Ok(peers) = PeerStore::open(paths.peers()) else {
        return;
    };
    let Some(peer) = peers.peers().iter().find(|p| p.id == id) else {
        return;
    };
    for address in announced_addresses(service) {
        let _ = cache::suggest_address(paths, &peer.id, &peer.name, &address);
    }
}

/// The `ip:port` strings worth trying from an announcement.
///
/// Announcements travel only within a network segment, so a real one names
/// an address on a network this machine is attached to. Anything else is
/// dropped; it could only make this machine open connections elsewhere.
fn announced_addresses(service: &ResolvedService) -> Vec<String> {
    let mut addresses: Vec<String> = service
        .get_addresses_v4()
        .into_iter()
        .filter(|ip| istesso_doctor::on_attached_network(*ip))
        .map(|ip| SocketAddr::new(IpAddr::V4(ip), service.port).to_string())
        .collect();
    addresses.sort();
    addresses
}

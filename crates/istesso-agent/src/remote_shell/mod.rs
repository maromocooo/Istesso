//! A shell on another machine, carried over the control channel instead of
//! `ssh` (proposed in ADR 0007, "文字のシェル").
//!
//! * [`client`] is the program a terminal application runs instead of a
//!   local shell: `istesso shell <device>`.
//! * [`host`] is what the agent does when a device asks for a shell: the
//!   user's login shell on a new pseudo-terminal, as the user the agent runs
//!   as.
//!
//! Off by default: the host has to name the devices that may ask
//! ([`ShellPolicy`]), and they must still be paired and allowed to control
//! the host, looked up again every second. Only counts are written down,
//! never what is typed or printed.

use std::time::Duration;

use istesso_pair::{Peer, PeerStore};

pub mod client;
pub mod host;
mod sys;

/// How often the device says "still here".
pub const ALIVE_EVERY: Duration = Duration::from_secs(10);
/// Shells open at the same time for one device, and on this machine.
pub const MAX_PER_DEVICE: usize = 8;
pub const MAX_ON_HOST: usize = 16;

/// The devices that may open a shell on this machine. Empty, the default:
/// none may.
#[derive(Debug, Clone, Default)]
pub struct ShellPolicy {
    /// Each device by its id and key, as they were when the agent started.
    allowed: Vec<(String, String)>,
}

impl ShellPolicy {
    /// Looks the devices up in the list of paired devices, by id or name
    /// (as for `istesso peers`). A device that is not there is an error:
    /// the person meant one and should hear that it is not paired.
    pub fn resolve(peers: &PeerStore, selectors: &[String]) -> Result<Self, String> {
        let mut allowed = Vec::new();
        for selector in selectors {
            let peer = peers
                .find(selector)
                .map_err(|e| format!("--allow-shell {selector}: {e}"))?
                .ok_or_else(|| format!("--allow-shell: no paired device matches '{selector}'"))?;
            allowed.push((peer.id.clone(), peer.public_key.clone()));
        }
        Ok(ShellPolicy { allowed })
    }

    pub fn is_on(&self) -> bool {
        !self.allowed.is_empty()
    }

    /// Whether this device was named, with the key it had then.
    pub fn names(&self, peer: &Peer) -> bool {
        self.allowed
            .iter()
            .any(|(id, key)| *id == peer.id && *key == peer.public_key)
    }
}

#[cfg(test)]
mod tests;

//! Turns a plan into a Landlock ruleset.

use landlock::{
    make_bitflags, Access, AccessFs, AccessNet, BitFlags, NetPort, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope, ABI,
};

use crate::policy::{Access as PathAccess, Plan};

/// The lowest ABI this launcher accepts: files (1), TCP ports (4), ioctl on devices (5),
/// signals and abstract UNIX sockets (6). Below it, the launcher refuses instead of running with
/// less than the level promises.
pub const MIN_ABI: i32 = 6;

/// Rights are asked for up to this ABI. The crate drops what the running kernel does not know
/// (best effort), so a newer kernel also gets named UNIX sockets (ABI 9) without a change here.
const WANTED: ABI = ABI::V9;

pub struct Built {
    pub ruleset: RulesetCreated,
    /// Paths of the plan that do not exist on this machine and got no rule.
    pub skipped: Vec<std::path::PathBuf>,
}

pub fn build(plan: &Plan) -> Result<Built, String> {
    let err = |e: landlock::RulesetError| format!("landlock: {e}");
    let mut rs = Ruleset::default()
        .handle_access(AccessFs::from_all(WANTED))
        .map_err(err)?
        // Only connecting is handled. Binding stays free so development servers can listen;
        // who may reach them is the network rule's job (nftables, see the proto document).
        .handle_access(AccessNet::ConnectTcp)
        .map_err(err)?
        .scope(Scope::from_all(WANTED))
        .map_err(err)?
        .create()
        .map_err(err)?;

    let mut skipped = Vec::new();
    for rule in &plan.paths {
        let fd = match PathFd::new(&rule.path) {
            Ok(fd) => fd,
            Err(_) => {
                skipped.push(rule.path.clone());
                continue;
            }
        };
        let is_dir = std::fs::metadata(&rule.path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let mut access: BitFlags<AccessFs> = match rule.access {
            PathAccess::Read => AccessFs::from_read(WANTED),
            PathAccess::Write => AccessFs::from_all(WANTED) & !BitFlags::from(AccessFs::IoctlDev),
            PathAccess::Device => {
                make_bitflags!(AccessFs::{ReadFile | WriteFile | ReadDir | IoctlDev})
            }
        };
        if !is_dir {
            access &= AccessFs::from_file(WANTED);
        }
        rs = rs.add_rule(PathBeneath::new(fd, access)).map_err(err)?;
    }
    for port in &plan.connect_ports {
        rs = rs
            .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
            .map_err(err)?;
    }
    Ok(Built {
        ruleset: rs,
        skipped,
    })
}

/// Applies the ruleset to the calling thread (and so to everything it executes or starts).
pub fn restrict(ruleset: RulesetCreated) -> Result<(), String> {
    let status = ruleset
        .restrict_self()
        .map_err(|e| format!("landlock: {e}"))?;
    if status.ruleset == RulesetStatus::NotEnforced || !status.no_new_privs {
        return Err("landlock: the rules were not enforced".to_string());
    }
    Ok(())
}

/// One line per part, saying what this kernel enforces for the plan.
pub fn report(plan: &Plan, abi: i32) -> Vec<String> {
    let ports: Vec<String> = plan.connect_ports.iter().map(|p| p.to_string()).collect();
    let ports = if ports.is_empty() {
        "none".to_string()
    } else {
        ports.join(",")
    };
    let mut lines = vec![
        format!(
            "level {} ({}), Landlock ABI {abi}",
            plan.level.number(),
            plan.level.describe()
        ),
        "enforced: files (read, write, execute) outside the rules".to_string(),
        format!(
            "enforced: TCP connect only to ports {ports} (any address; binding is not limited)"
        ),
        "enforced: signals and abstract UNIX sockets outside the scope".to_string(),
    ];
    if abi < 9 {
        lines.push(
            "NOT enforced on this kernel: connecting to named UNIX sockets (needs ABI 9): \
             the Docker socket, the user's service manager and session bus stay reachable"
                .to_string(),
        );
    }
    if abi < 10 {
        lines.push("NOT enforced on this kernel: UDP (needs ABI 10)".to_string());
    }
    lines
}

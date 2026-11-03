//! Who is calling, from the bus connection: the caller's uid and process come from the
//! bus daemon (`GetConnectionCredentials`), never from the request. The process's
//! executable must be one of the suite's, installed next to the daemon, and it decides
//! what the caller may do.
//!
//! The process is pinned while its executable is read: through the pidfd the bus hands
//! out when it can, otherwise by the process's start time read before and after. A
//! process that exited (and whose pid was reused) in between is refused. This keeps
//! unrelated programs out; a program of the same user that runs one of the suite's
//! binaries itself is beyond what any same-user check can stop (docs/SECURITY.md).

use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};

/// What a caller may do; see docs/API.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The popup: the history, snippets, actions and the current codes.
    Popup,
    /// The Settings page: status, private mode, clearing history.
    Settings,
    /// Glimpse's `cb` search: the history and actions; never codes.
    Glimpse,
    /// The notification service: offering codes from notifications.
    Notifications,
    /// The Notification Center panel: copying a code it shows, safely.
    NotificationCenter,
    /// The Passwords (Vault) daemon: codes for the browser extension, by website.
    Vault,
    /// Any other Nyx app beside the daemon: registering its own actions only.
    App,
}

const PEERS: &[(&str, Scope)] = &[
    ("nyx-clipboard", Scope::Popup),
    ("nyx-clipboard-settings", Scope::Settings),
    ("nyx-glimpse", Scope::Glimpse),
    ("nyx-notifyd", Scope::Notifications),
    ("nyx-notification-center", Scope::NotificationCenter),
    ("nyx-passwordsd", Scope::Vault),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub pid: u32,
    pub scope: Scope,
    /// The executable's file name (`nyx-notes`).
    pub app: String,
}

/// The scope for an executable path: one of `PEERS`, or another `nyx-*` program, in
/// `allowed_dir`. A binary replaced while running reads as "… (deleted)" and is refused.
pub fn scope_of(exe: &Path, allowed_dir: &Path) -> Option<Scope> {
    if exe.parent()? != allowed_dir {
        return None;
    }
    let name = exe.file_name()?.to_str()?;
    if let Some((_, scope)) = PEERS.iter().find(|(n, _)| *n == name) {
        return Some(*scope);
    }
    let app = name.starts_with("nyx-")
        && !name.starts_with("nyx-clipboard")
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    app.then_some(Scope::App)
}

/// Where the suite's binaries are: the daemon's own folder (`~/.local/bin` installed,
/// `target/<profile>` in a checkout).
pub fn allowed_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Identify a caller from the credentials the bus gave for its connection.
pub fn identify(uid: Option<u32>, pid: Option<u32>, pidfd: Option<BorrowedFd<'_>>, allowed_dir: &Path) -> Option<Peer> {
    // SAFETY: getuid has no preconditions.
    if uid? != unsafe { libc::getuid() } {
        return None;
    }
    let pid = match pidfd {
        Some(fd) => pidfd_pid(fd.as_raw_fd())?,
        None => pid?,
    };
    let started = start_time(pid)?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let still = match pidfd {
        Some(fd) => pidfd_pid(fd.as_raw_fd()) == Some(pid),
        None => true,
    };
    if !still || start_time(pid)? != started {
        return None;
    }
    let scope = scope_of(&exe, allowed_dir)?;
    let app = exe.file_name()?.to_str()?.to_string();
    Some(Peer { pid, scope, app })
}

/// Field 22 of `/proc/<pid>/stat`: when the process started, in clock ticks.
fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces; fields resume after it.
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The pid a pidfd refers to; `None` once the process has gone.
fn pidfd_pid(fd: i32) -> Option<u32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}")).ok()?;
    let pid: i64 = info.lines().find_map(|l| l.strip_prefix("Pid:"))?.trim().parse().ok()?;
    (pid > 0).then_some(pid as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_by_name_and_place() {
        let dir = Path::new("/home/u/.local/bin");
        assert_eq!(scope_of(&dir.join("nyx-clipboard"), dir), Some(Scope::Popup));
        assert_eq!(scope_of(&dir.join("nyx-glimpse"), dir), Some(Scope::Glimpse));
        assert_eq!(scope_of(&dir.join("nyx-notifyd"), dir), Some(Scope::Notifications));
        assert_eq!(scope_of(&dir.join("nyx-passwordsd"), dir), Some(Scope::Vault));
        assert_eq!(scope_of(&dir.join("nyx-notes"), dir), Some(Scope::App));
        assert_eq!(scope_of(&dir.join("nyx-clipboard-other"), dir), None);
        assert_eq!(scope_of(&dir.join("nyx-Notes"), dir), None);
        assert_eq!(scope_of(Path::new("/tmp/nyx-clipboard"), dir), None);
        assert_eq!(scope_of(Path::new("/usr/bin/busctl"), dir), None);
        assert_eq!(scope_of(&dir.join("python3"), dir), None);
        assert_eq!(scope_of(Path::new("/home/u/.local/bin/nyx-glimpse (deleted)"), dir), None);
    }

    #[test]
    fn identifies_this_process() {
        let me = std::env::current_exe().unwrap();
        let uid = unsafe { libc::getuid() };
        let pid = std::process::id();
        // This test binary isn't one of the suite's programs.
        assert_eq!(identify(Some(uid), Some(pid), None, me.parent().unwrap()), None);
        assert_eq!(identify(Some(uid + 1), Some(pid), None, me.parent().unwrap()), None);
        assert!(start_time(pid).is_some());
    }
}

use super::text_log::TextLog;
use super::{Transaction, apt, dnf, pacman, zypper};

/// One package manager's history. The follower thread only asks it for what is new; how a text
/// log or a database notices a change, and where it last stopped, stays inside.
pub trait History: Send {
    fn name(&self) -> &'static str;
    fn path(&self) -> &str;
    /// Transactions finished since the last call; the first call returns those from the newest
    /// stored second on.
    fn poll(&mut self) -> Result<Vec<Transaction>, String>;
    /// Every transaction in the whole history, read from scratch (for `--check`).
    fn history(&self) -> Result<Vec<Transaction>, String>;
}

pub(super) const NONE_FOUND: &str = "no pacman, apt, zypper or dnf history found";

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

type Open = fn(&'static str, String, i64) -> Box<dyn History>;

/// The histories that exist on this machine, usually exactly one. `since` gives each manager's
/// newest stored transaction, where its source starts.
pub fn detect(since: fn(&str) -> i64) -> Vec<Box<dyn History>> {
    let text: [(&'static str, &str, &str, Open); 3] = [
        (
            "pacman",
            "BLACKBOX_PACMAN_LOG",
            "/var/log/pacman.log",
            TextLog::<pacman::Parser>::open,
        ),
        (
            "apt",
            "BLACKBOX_APT_LOG",
            "/var/log/apt/history.log",
            TextLog::<apt::Parser>::open,
        ),
        (
            "zypper",
            "BLACKBOX_ZYPPER_LOG",
            "/var/log/zypp/history",
            TextLog::<zypper::Parser>::open,
        ),
    ];
    let mut found: Vec<Box<dyn History>> = text
        .into_iter()
        .map(|(name, var, default, open)| (name, env_or(var, default), open))
        .filter(|(_, path, _)| std::path::Path::new(path).exists())
        .map(|(name, path, open)| open(name, path, since(name)))
        .collect();
    if let Some(path) = dnf::db_path() {
        found.push(Box::new(dnf::Dnf::new(path, since("dnf"))));
    }
    found
}

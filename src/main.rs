mod check;
mod database;
mod sources;

const USAGE: &str = "\
blackbox: records crashes, logs and system load into one SQLite file

usage: blackbox [--help | --version | --check]

Runs until stopped, normally as the systemd user service. --check reports what
the recorder can see on this machine and how to open up what it cannot.

Settings are environment variables: BLACKBOX_DB, BLACKBOX_MAX_MB,
BLACKBOX_BATCH_MS, BLACKBOX_AUDIT_LOG, BLACKBOX_PACMAN_LOG, BLACKBOX_APT_LOG,
BLACKBOX_ZYPPER_LOG, BLACKBOX_DNF_DB. See the README for what each does.";

/// What to print, and with which exit code, when the arguments mean the recorder must not start.
/// `--check` is not here: it has to look at the machine first.
fn early_exit(args: &[String]) -> Option<(String, i32)> {
    match args.first().map(String::as_str) {
        None | Some("--check") => None,
        Some("-h" | "--help") => Some((USAGE.to_string(), 0)),
        Some("-V" | "--version") => Some((format!("blackbox {}", env!("CARGO_PKG_VERSION")), 0)),
        Some(other) => Some((
            format!("blackbox: unknown argument '{other}'\n\n{USAGE}"),
            2,
        )),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some((text, code)) = early_exit(&args) {
        if code == 0 {
            println!("{text}");
        } else {
            eprintln!("{text}");
        }
        std::process::exit(code);
    }
    if args.first().map(String::as_str) == Some("--check") {
        check::run();
        return;
    }
    database::init_database();
    database::start_writer();
    let max_mb: u64 = std::env::var("BLACKBOX_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    database::start_trim_thread(max_mb * 1_000_000);
    sources::run();
}

#[cfg(test)]
mod tests {
    use super::early_exit;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_starts_the_recorder() {
        assert!(early_exit(&[]).is_none());
    }

    #[test]
    fn check_is_left_to_main() {
        assert!(early_exit(&args(&["--check"])).is_none());
    }

    #[test]
    fn help_and_version_exit_cleanly() {
        for flag in ["-h", "--help", "-V", "--version"] {
            assert_eq!(early_exit(&args(&[flag])).unwrap().1, 0, "{flag}");
        }
        assert!(
            early_exit(&args(&["--version"]))
                .unwrap()
                .0
                .starts_with("blackbox 0.")
        );
    }

    #[test]
    fn any_other_argument_is_refused_instead_of_starting() {
        let (text, code) = early_exit(&args(&["--bogus"])).unwrap();
        assert_eq!(code, 2);
        assert!(text.contains("--bogus"));
    }
}

mod auditd;
mod boot;
mod collector;
mod database;
mod exit;
mod journald;
mod pacman;
mod power;
mod sampler;
mod sensors;

const USAGE: &str = "\
blackbox: records crashes, logs and system load into one SQLite file

usage: blackbox [--help | --version]

Runs until stopped, normally as the systemd user service. Settings are
environment variables: BLACKBOX_DB, BLACKBOX_MAX_MB, BLACKBOX_BATCH_MS,
BLACKBOX_AUDIT_LOG, BLACKBOX_PACMAN_LOG. See the README for what each does.";

/// What to print, and with which exit code, when the arguments mean the recorder must not start.
fn early_exit(args: &[String]) -> Option<(String, i32)> {
    match args.first().map(String::as_str) {
        None => None,
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
    database::init_database();
    let max_mb: u64 = std::env::var("BLACKBOX_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    database::start_trim_thread(max_mb * 1_000_000);
    sampler::start();
    journald::start();
    auditd::start();
    pacman::start();
    boot::start();
    collector::build_collector();
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

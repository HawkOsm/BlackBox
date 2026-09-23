mod auditd;
mod boot;
mod collector;
mod database;
mod exit;
mod journald;
mod pacman;
mod sampler;

fn main() {
    database::init_database();
    let max_mb: u64 = std::env::var("BLACKBOX_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7500);
    database::start_trim_thread(max_mb * 1_000_000);
    sampler::start();
    journald::start();
    auditd::start();
    pacman::start();
    boot::start();
    collector::build_collector();
}

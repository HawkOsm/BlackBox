mod auditd;
mod boot;
mod collector;
mod database;
mod exit;
mod journald;
mod pacman;
mod sampler;
mod store;

use std::sync::Arc;
use store::Store;

fn main() {
    database::init_database();
    let max_mb: u64 = std::env::var("BLACKBOX_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7500);
    database::start_trim_thread(max_mb * 1_000_000);
    let store: Arc<dyn Store> = Arc::new(database::LocalStore);
    sampler::start(store.clone());
    journald::start(store.clone());
    auditd::start(store.clone());
    pacman::start(store.clone());
    boot::start(store.clone());
    collector::build_collector(&*store);
}

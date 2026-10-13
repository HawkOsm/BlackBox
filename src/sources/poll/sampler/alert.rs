//! When a sample is worth an alert: the thresholds in `docs/architecture.md`.

use super::store::Sample;
use crate::database::Severity;

/// The alert for a sample, if it crosses a threshold: memory 85% (95% is an error), CPU 90%, load
/// above twice the cores, GPU 85 °C or 90% of its memory.
pub(super) fn assess(s: &Sample, cores: f64) -> Option<(Severity, String)> {
    let mut problems: Vec<String> = Vec::new();
    let mut error = false;
    if s.mem_pct >= 95.0 {
        error = true;
        problems.push(format!("memory {:.0}% used", s.mem_pct));
    } else if s.mem_pct >= 85.0 {
        problems.push(format!("memory {:.0}% used", s.mem_pct));
    }
    if s.cpu_pct >= 90.0 {
        problems.push(format!("cpu {:.0}% busy", s.cpu_pct));
    }
    if s.load_avg > cores * 2.0 {
        problems.push(format!("load {:.1} on {:.0} cores", s.load_avg, cores));
    }
    if let Some(t) = s.gpu_temp.filter(|t| *t >= 85.0) {
        problems.push(format!("gpu {t:.0}C"));
    }
    if let Some(m) = s.gpu_mem_pct.filter(|m| *m >= 90.0) {
        problems.push(format!("gpu memory {m:.0}% used"));
    }
    if problems.is_empty() {
        return None;
    }
    let severity = if error {
        Severity::Error
    } else {
        Severity::Warning
    };
    Some((severity, problems.join(", ")))
}

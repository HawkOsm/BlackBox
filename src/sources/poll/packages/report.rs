use super::history::{NONE_FOUND, detect};
use super::summary;
use crate::database;

/// For `blackbox --check`: what each detected manager's history holds, read from scratch.
pub fn report() -> Vec<String> {
    let sources = detect(|_| i64::MIN);
    if sources.is_empty() {
        return vec![format!("{NONE_FOUND}: package changes are not recorded")];
    }
    sources
        .iter()
        .map(|s| {
            let (name, path) = (s.name(), s.path());
            match s.history() {
                Err(e) => format!("{name} ({path}): not readable: {e}"),
                Ok(all) => match all.iter().max_by_key(|t| t.ts) {
                    None => format!("{name} ({path}): readable, no transactions yet"),
                    Some(newest) => format!(
                        "{name} ({path}): {} transactions, newest {}: {}",
                        all.len(),
                        database::ts_text(newest.ts),
                        summary(name, newest)
                    ),
                },
            }
        })
        .collect()
}

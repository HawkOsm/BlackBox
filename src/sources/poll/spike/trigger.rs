//! When a reading is a spike: a rise of more than `RISE_C` over the last `WINDOW` readings, or
//! `HOT_C` and above. After one, `COOLDOWN` readings pass before the next, or a long hot stretch
//! would capture every second.

use std::collections::VecDeque;

const RISE_C: f64 = 15.0;
const HOT_C: f64 = 80.0;
/// Readings looked back on: one a second, so 3 s.
const WINDOW: usize = 3;
/// Readings to skip after a spike: about a minute.
const COOLDOWN: u32 = 60;

pub(super) struct Trigger {
    past: VecDeque<f64>,
    quiet: u32,
}

impl Trigger {
    pub(super) fn new() -> Self {
        Trigger {
            past: VecDeque::with_capacity(WINDOW + 1),
            quiet: 0,
        }
    }

    /// One reading, in C. Some(rise over the window) when it is a spike to capture.
    pub(super) fn see(&mut self, temp: f64) -> Option<f64> {
        let coolest = self.past.iter().copied().reduce(f64::min).unwrap_or(temp);
        self.past.push_back(temp);
        if self.past.len() > WINDOW {
            self.past.pop_front();
        }
        if self.quiet > 0 {
            self.quiet -= 1;
            return None;
        }
        let rise = temp - coolest;
        if rise > RISE_C || temp >= HOT_C {
            self.quiet = COOLDOWN;
            return Some(rise);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(t: &mut Trigger, temps: &[f64]) -> Vec<Option<f64>> {
        temps.iter().map(|x| t.see(*x)).collect()
    }

    #[test]
    fn a_fast_rise_fires_and_a_slow_one_does_not() {
        let mut t = Trigger::new();
        // +16 over 3 s
        assert_eq!(
            feed(&mut t, &[50.0, 52.0, 60.0, 66.0]),
            [None, None, None, Some(16.0)]
        );
        let mut t = Trigger::new();
        // +20 over 5 s, never more than 15 in any 3
        let slow = feed(&mut t, &[50.0, 54.0, 58.0, 62.0, 64.0, 66.0, 70.0]);
        assert!(slow.iter().all(Option::is_none), "{slow:?}");
    }

    #[test]
    fn heat_fires_once_a_minute_not_every_second() {
        let mut t = Trigger::new();
        let fired = feed(&mut t, &[85.0; 130]);
        let at: Vec<usize> = fired
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_some())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at, [0, 61, 122]);
    }
}

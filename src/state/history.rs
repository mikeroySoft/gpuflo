//! Preallocated history rings, session peaks, and daily summary accumulation.
//!
//! One ring slot corresponds to one production tick; `None` slots are honest
//! gaps where no fresh observation arrived. Retained, stale, and failed
//! observations never enter a ring or move a peak.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::Date;

/// Approved history capacity: 240 points over 60 seconds.
pub(crate) const HISTORY_CAPACITY: usize = 240;

/// Fixed-capacity per-tick history ring.
#[derive(Debug, Clone)]
pub(crate) struct Ring {
    slots: Vec<Option<f64>>,
    /// Index of the next slot to write.
    head: usize,
    /// Number of valid slots, up to capacity.
    len: usize,
}

impl Ring {
    /// Preallocates the full 240-slot ring.
    pub fn new() -> Self {
        Self {
            slots: vec![None; HISTORY_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    /// Appends one production-tick slot: a fresh value or an honest gap.
    pub fn push(&mut self, value: Option<f64>) {
        self.slots[self.head] = value;
        self.head = (self.head + 1) % HISTORY_CAPACITY;
        self.len = (self.len + 1).min(HISTORY_CAPACITY);
    }

    /// Copies the window oldest→newest. Bounded to capacity; used only when
    /// render data must cross a thread, never per frame.
    pub fn to_vec(&self) -> Vec<Option<f64>> {
        let mut out = Vec::with_capacity(self.len);
        let start = (self.head + HISTORY_CAPACITY - self.len) % HISTORY_CAPACITY;
        for offset in 0..self.len {
            out.push(self.slots[(start + offset) % HISTORY_CAPACITY]);
        }
        out
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }
}

/// Persisted per-day, per-physical-GPU summary. The only durable record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DailySummaryRecord {
    /// Local calendar date `YYYY-MM-DD`.
    pub date: String,
    /// Keyed by stable physical GPU id.
    pub gpus: BTreeMap<String, GpuDailyRecord>,
}

/// One physical GPU's daily peaks and energy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GpuDailyRecord {
    /// Peak primary-partition GFX activity, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_peak_percent: Option<f64>,
    /// Peak primary-partition memory occupancy, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_peak_percent: Option<f64>,
    /// Accumulated socket energy, joules, when the source exposes energy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_joules: Option<f64>,
}

/// In-memory accumulation of the current local day's summary.
#[derive(Debug, Clone)]
pub(crate) struct DailyAccumulator {
    date: Date,
    gpus: BTreeMap<String, GpuDailyRecord>,
    /// Last raw energy accumulator per GPU, to derive deltas.
    energy_raw: BTreeMap<String, u64>,
    dirty: bool,
}

impl DailyAccumulator {
    /// Starts a fresh accumulator for `date`.
    pub fn new(date: Date) -> Self {
        Self {
            date,
            gpus: BTreeMap::new(),
            energy_raw: BTreeMap::new(),
            dirty: false,
        }
    }

    /// Seeds the accumulator from a persisted record when the date matches;
    /// a record from another day is discarded.
    pub fn seed(&mut self, record: &DailySummaryRecord) {
        if record.date == format_date(self.date) {
            self.gpus = record.gpus.clone();
        }
    }

    /// Rolls to `date` when the local day changed, discarding the old day.
    /// Returns true on rollover.
    pub fn roll(&mut self, date: Date) -> bool {
        if date == self.date {
            return false;
        }
        self.date = date;
        self.gpus.clear();
        self.energy_raw.clear();
        self.dirty = true;
        true
    }

    /// Records a fresh primary-partition activity observation.
    pub fn observe_activity(&mut self, gpu: &str, percent: f64) {
        let entry = self.gpus.entry(gpu.to_owned()).or_default();
        if entry
            .activity_peak_percent
            .is_none_or(|peak| percent > peak)
        {
            entry.activity_peak_percent = Some(percent);
            self.dirty = true;
        }
    }

    /// Records a fresh primary-partition memory occupancy observation.
    pub fn observe_memory(&mut self, gpu: &str, percent: f64) {
        let entry = self.gpus.entry(gpu.to_owned()).or_default();
        if entry.memory_peak_percent.is_none_or(|peak| percent > peak) {
            entry.memory_peak_percent = Some(percent);
            self.dirty = true;
        }
    }

    /// Records a raw socket energy accumulator reading. The first reading
    /// after start or counter reset only anchors the baseline.
    pub fn observe_energy(&mut self, gpu: &str, raw: u64, joules_per_count: f64) {
        let previous = self.energy_raw.insert(gpu.to_owned(), raw);
        let Some(previous) = previous else { return };
        let Some(delta) = raw.checked_sub(previous) else {
            return;
        };
        if delta == 0 {
            return;
        }
        let joules = delta as f64 * joules_per_count;
        let entry = self.gpus.entry(gpu.to_owned()).or_default();
        *entry.energy_joules.get_or_insert(0.0) += joules;
        self.dirty = true;
    }

    /// Whether the summary changed since the last [`Self::record`] call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The current persistable record.
    pub fn record(&self) -> DailySummaryRecord {
        DailySummaryRecord {
            date: format_date(self.date),
            gpus: self.gpus.clone(),
        }
    }
}

/// `YYYY-MM-DD`.
pub(crate) fn format_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn ring_is_bounded_and_ordered() {
        let mut ring = Ring::new();
        assert_eq!(ring.to_vec(), Vec::<Option<f64>>::new());
        for i in 0..250u32 {
            ring.push(if i % 3 == 0 { None } else { Some(f64::from(i)) });
        }
        let window = ring.to_vec();
        assert_eq!(ring.len(), HISTORY_CAPACITY);
        assert_eq!(window.len(), HISTORY_CAPACITY);
        // Oldest surviving slot is tick 10, newest is tick 249.
        assert_eq!(window[0], Some(10.0));
        assert_eq!(window[HISTORY_CAPACITY - 1], None); // 249 % 3 == 0
        assert_eq!(window[HISTORY_CAPACITY - 2], Some(248.0));
    }

    #[test]
    fn daily_accumulator_tracks_peaks_and_energy_deltas() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        daily.observe_activity("gpu-a", 40.0);
        daily.observe_activity("gpu-a", 90.0);
        daily.observe_activity("gpu-a", 60.0);
        daily.observe_memory("gpu-a", 55.5);
        daily.observe_energy("gpu-a", 1_000, 0.5); // baseline only
        daily.observe_energy("gpu-a", 1_100, 0.5); // +50 J
        daily.observe_energy("gpu-a", 900, 0.5); // reset: re-anchor
        daily.observe_energy("gpu-a", 1_000, 0.5); // +50 J
        let record = daily.record();
        assert_eq!(record.date, "2026-08-21");
        let gpu = &record.gpus["gpu-a"];
        assert_eq!(gpu.activity_peak_percent, Some(90.0));
        assert_eq!(gpu.memory_peak_percent, Some(55.5));
        assert_eq!(gpu.energy_joules, Some(100.0));
    }

    #[test]
    fn rollover_resets_and_seed_ignores_other_days() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        daily.observe_activity("gpu-a", 90.0);
        assert!(!daily.roll(date!(2026 - 08 - 21)));
        assert!(daily.roll(date!(2026 - 08 - 22)));
        assert!(daily.record().gpus.is_empty());

        let mut seeded = DailyAccumulator::new(date!(2026 - 08 - 22));
        let mut old = DailySummaryRecord {
            date: "2026-08-21".into(),
            gpus: BTreeMap::new(),
        };
        old.gpus.insert(
            "gpu-a".into(),
            GpuDailyRecord {
                activity_peak_percent: Some(99.0),
                ..Default::default()
            },
        );
        seeded.seed(&old);
        assert!(seeded.record().gpus.is_empty());
        let same_day = DailySummaryRecord {
            date: "2026-08-22".into(),
            gpus: old.gpus.clone(),
        };
        seeded.seed(&same_day);
        assert_eq!(
            seeded.record().gpus["gpu-a"].activity_peak_percent,
            Some(99.0)
        );
    }
}

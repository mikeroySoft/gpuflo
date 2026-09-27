//! Preallocated history rings, session peaks, and daily summary accumulation.
//!
//! One ring slot corresponds to one production tick; `None` slots are honest
//! gaps where no fresh observation arrived. Retained, stale, and failed
//! observations never enter a ring or move a peak.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

/// Approved history capacity: 240 points over 60 seconds.
pub(crate) const HISTORY_CAPACITY: usize = 240;
/// Throttle episodes retained per GPU for the current day.
const MAX_THROTTLE_EPISODES_PER_GPU: usize = 64;

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
    /// Throttle episodes observed today, in observation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub throttle_episodes: Vec<ThrottleEpisode>,
}

/// One continuously active throttle, from first to last observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ThrottleEpisode {
    /// Local time the episode was first observed, RFC 3339.
    pub started_at: String,
    /// Local time the episode was first observed to have ended, RFC 3339.
    /// Absent while the episode is still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Seconds between started_at and ended_at. Absent while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// Source-named reason groups, as first observed for this episode.
    pub reasons: String,
}

/// In-memory accumulation of the current local day's summary. It owns what the
/// day retains, including throttle-episode retention, so [`Self::record`] is
/// the canonical record persistence writes unchanged.
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
    /// a record from another day is discarded. A record written by another
    /// build may carry more episodes than this one retains, so the seeded
    /// state is trimmed back to the retention rules.
    pub fn seed(&mut self, record: &DailySummaryRecord) {
        if record.date == format_date(self.date) {
            self.gpus = record.gpus.clone();
            if self.enforce_retention() {
                self.dirty = true;
            }
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

    /// Updates an episode on each accepted slow health observation.
    pub fn observe_health(
        &mut self,
        gpu: &str,
        observed_at: OffsetDateTime,
        offset: UtcOffset,
        reasons: Option<&str>,
    ) {
        let entry = self.gpus.entry(gpu.to_owned()).or_default();
        let episodes = &mut entry.throttle_episodes;
        let open = episodes
            .last()
            .is_some_and(|episode| episode.ended_at.is_none());
        let changed = match (open, reasons) {
            (false, Some(reasons)) => {
                episodes.push(ThrottleEpisode {
                    started_at: stamp(observed_at, offset),
                    ended_at: None,
                    duration_seconds: None,
                    reasons: reasons.to_owned(),
                });
                true
            }
            (true, None) => {
                let episode = episodes.last_mut().expect("open episode");
                let start = OffsetDateTime::parse(&episode.started_at, &Rfc3339).ok();
                episode.ended_at = Some(stamp(observed_at, offset));
                episode.duration_seconds =
                    start.map(|start| (observed_at - start).as_seconds_f64());
                true
            }
            _ => false,
        };
        if changed {
            self.enforce_retention();
            self.dirty = true;
        }
    }

    /// Trims retained throttle episodes to the day's retention rules: at most
    /// [`MAX_THROTTLE_EPISODES_PER_GPU`] per GPU, and few enough in total that
    /// the record fits the persistence budget. Only closed episodes are
    /// dropped, oldest first across all GPUs, so an in-progress throttle stays
    /// visible. Returns whether anything was dropped.
    fn enforce_retention(&mut self) -> bool {
        let mut dropped = false;
        for entry in self.gpus.values_mut() {
            while entry.throttle_episodes.len() > MAX_THROTTLE_EPISODES_PER_GPU {
                let Some(index) = entry
                    .throttle_episodes
                    .iter()
                    .position(|episode| episode.ended_at.is_some())
                else {
                    break;
                };
                entry.throttle_episodes.remove(index);
                dropped = true;
            }
        }
        while !self.fits_record_budget() {
            let Some((gpu, index)) = self.oldest_closed_episode() else {
                break;
            };
            self.gpus
                .get_mut(&gpu)
                .expect("gpu from this map")
                .throttle_episodes
                .remove(index);
            dropped = true;
        }
        dropped
    }

    /// Whether the record serializes within the budget `persist` accepts,
    /// measured on the exact encoding written to disk.
    fn fits_record_budget(&self) -> bool {
        match crate::persist::encode(&self.record()) {
            Ok(body) => body.len() as u64 <= crate::persist::MAX_RECORD_BYTES,
            // An unencodable record cannot be shrunk by dropping episodes.
            Err(_) => true,
        }
    }

    /// The globally oldest closed episode, by start time then GPU id then
    /// position, so eviction order does not depend on map iteration luck.
    /// An unparsable start sorts oldest and is evicted first.
    fn oldest_closed_episode(&self) -> Option<(String, usize)> {
        self.gpus
            .iter()
            .flat_map(|(gpu, entry)| {
                entry
                    .throttle_episodes
                    .iter()
                    .enumerate()
                    .filter(|(_, episode)| episode.ended_at.is_some())
                    .map(move |(index, episode)| {
                        (
                            OffsetDateTime::parse(&episode.started_at, &Rfc3339).ok(),
                            gpu.clone(),
                            index,
                        )
                    })
            })
            .min()
            .map(|(_, gpu, index)| (gpu, index))
    }

    /// Whether the summary changed since the last [`Self::record`] call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The current persistable record. Already within the retention rules, so
    /// persistence stores exactly this.
    pub fn record(&self) -> DailySummaryRecord {
        DailySummaryRecord {
            date: format_date(self.date),
            gpus: self.gpus.clone(),
        }
    }
}

/// Local-offset RFC 3339 instant.
fn stamp(at: OffsetDateTime, offset: UtcOffset) -> String {
    at.to_offset(offset)
        .format(&Rfc3339)
        .expect("valid timestamp")
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
    #[test]
    fn throttle_episodes_close_and_keep_first_reasons() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        let start = date!(2026 - 08 - 21).midnight().assume_utc();
        daily.observe_health("gpu-a", start, UtcOffset::UTC, Some("thermal"));
        daily.observe_health(
            "gpu-a",
            start + time::Duration::seconds(2),
            UtcOffset::UTC,
            Some("power"),
        );
        let open = serde_json::to_value(daily.record()).unwrap();
        let episode = &open["gpus"]["gpu-a"]["throttle_episodes"][0];
        assert!(episode.get("ended_at").is_none());
        assert!(episode.get("duration_seconds").is_none());
        daily.observe_health(
            "gpu-a",
            start + time::Duration::seconds(5),
            UtcOffset::UTC,
            None,
        );
        let episodes = &daily.record().gpus["gpu-a"].throttle_episodes;
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].reasons, "thermal");
        assert_eq!(episodes[0].duration_seconds, Some(5.0));
        assert!(episodes[0].ended_at.is_some());
        daily.observe_health(
            "gpu-a",
            start + time::Duration::seconds(6),
            UtcOffset::UTC,
            Some("power"),
        );
        let episodes = &daily.record().gpus["gpu-a"].throttle_episodes;
        assert_eq!(episodes.len(), 2);
        assert!(episodes[0].started_at < episodes[1].started_at);
        assert_eq!(episodes[1].reasons, "power");
        assert!(episodes[1].ended_at.is_none());
    }

    #[test]
    fn throttle_roll_seed_and_legacy_record() {
        let start = date!(2026 - 08 - 21).midnight().assume_utc();
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        daily.observe_health("gpu-a", start, UtcOffset::UTC, Some("thermal"));
        let record = daily.record();
        let mut restored = DailyAccumulator::new(date!(2026 - 08 - 21));
        restored.seed(&record);
        restored.observe_health(
            "gpu-a",
            start + time::Duration::seconds(3),
            UtcOffset::UTC,
            None,
        );
        assert_eq!(
            restored.record().gpus["gpu-a"].throttle_episodes[0].duration_seconds,
            Some(3.0)
        );
        assert!(daily.roll(date!(2026 - 08 - 22)));
        assert!(daily.record().gpus.is_empty());
        let legacy: DailySummaryRecord = serde_json::from_str(
            r#"{"date":"2026-08-21","gpus":{"gpu-a":{"activity_peak_percent":10}}}"#,
        )
        .unwrap();
        assert!(legacy.gpus["gpu-a"].throttle_episodes.is_empty());
    }

    #[test]
    fn malformed_seeded_start_closes_without_panicking() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        let mut record = daily.record();
        record.gpus.insert(
            "gpu-a".into(),
            GpuDailyRecord {
                throttle_episodes: vec![ThrottleEpisode {
                    started_at: "invalid".into(),
                    ended_at: None,
                    duration_seconds: None,
                    reasons: "thermal".into(),
                }],
                ..Default::default()
            },
        );
        daily.seed(&record);
        daily.observe_health(
            "gpu-a",
            date!(2026 - 08 - 21).midnight().assume_utc(),
            UtcOffset::UTC,
            None,
        );
        let episode = &daily.record().gpus["gpu-a"].throttle_episodes[0];
        assert!(episode.ended_at.is_some());
        assert_eq!(episode.duration_seconds, None);
    }

    #[test]
    fn throttle_cap_drops_oldest_closed_and_keeps_open() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        let start = date!(2026 - 08 - 21).midnight().assume_utc();
        for n in 0..=MAX_THROTTLE_EPISODES_PER_GPU {
            let at = start + time::Duration::seconds((n * 2) as i64);
            daily.observe_health("gpu-a", at, UtcOffset::UTC, Some("thermal"));
            if n < MAX_THROTTLE_EPISODES_PER_GPU {
                daily.observe_health(
                    "gpu-a",
                    at + time::Duration::seconds(1),
                    UtcOffset::UTC,
                    None,
                );
            }
        }
        let episodes = &daily.record().gpus["gpu-a"].throttle_episodes;
        assert_eq!(episodes.len(), MAX_THROTTLE_EPISODES_PER_GPU);
        assert!(!episodes[0].started_at.ends_with("00:00:00Z"));
        assert!(episodes.last().unwrap().ended_at.is_none());
    }

    /// One long-named GPU's worth of capped episodes, all closed but the last.
    fn capped_episodes(reasons: &str) -> GpuDailyRecord {
        let mut entry = GpuDailyRecord::default();
        for n in 0..MAX_THROTTLE_EPISODES_PER_GPU {
            let open = n == MAX_THROTTLE_EPISODES_PER_GPU - 1;
            let started_at = format!("2026-08-21T{:02}:{:02}:00Z", n / 60, n % 60);
            entry.throttle_episodes.push(ThrottleEpisode {
                started_at,
                ended_at: (!open).then(|| format!("2026-08-21T{:02}:{:02}:30Z", n / 60, n % 60)),
                duration_seconds: (!open).then_some(30.0),
                reasons: reasons.to_owned(),
            });
        }
        entry
    }

    fn encoded_len(record: &DailySummaryRecord) -> u64 {
        crate::persist::encode(record)
            .expect("record encodes")
            .len() as u64
    }

    #[test]
    fn observed_episodes_stay_within_the_persistence_budget() {
        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        let start = date!(2026 - 08 - 21).midnight().assume_utc();
        let gpus: Vec<String> = (0..8)
            .map(|n| format!("pci-0000:{n:02}:00.0-amdgpu-primary-{n}"))
            .collect();
        for gpu in &gpus {
            for n in 0..MAX_THROTTLE_EPISODES_PER_GPU {
                let at = start + time::Duration::seconds((n * 60) as i64);
                daily.observe_health(
                    gpu,
                    at,
                    UtcOffset::UTC,
                    Some("thermal, power, current, other accumulated"),
                );
                if n < MAX_THROTTLE_EPISODES_PER_GPU - 1 {
                    daily.observe_health(
                        gpu,
                        at + time::Duration::seconds(30),
                        UtcOffset::UTC,
                        None,
                    );
                }
            }
        }
        let record = daily.record();
        assert!(encoded_len(&record) <= crate::persist::MAX_RECORD_BYTES);
        // Budget eviction was needed, and no GPU lost its in-progress throttle.
        let retained: usize = record
            .gpus
            .values()
            .map(|gpu| gpu.throttle_episodes.len())
            .sum();
        assert!(retained < gpus.len() * MAX_THROTTLE_EPISODES_PER_GPU);
        for gpu in &gpus {
            assert!(
                record.gpus[gpu]
                    .throttle_episodes
                    .last()
                    .expect("episodes retained")
                    .ended_at
                    .is_none()
            );
        }
    }

    #[test]
    fn seeded_episodes_are_trimmed_to_the_persistence_budget() {
        let mut oversized = DailySummaryRecord {
            date: "2026-08-21".into(),
            gpus: BTreeMap::new(),
        };
        for n in 0..8 {
            oversized.gpus.insert(
                format!("pci-0000:{n:02}:00.0-amdgpu-primary-{n}"),
                capped_episodes("thermal, power, current, other accumulated"),
            );
        }
        assert!(encoded_len(&oversized) > crate::persist::MAX_RECORD_BYTES);

        let mut daily = DailyAccumulator::new(date!(2026 - 08 - 21));
        daily.seed(&oversized);
        let record = daily.record();
        assert!(encoded_len(&record) <= crate::persist::MAX_RECORD_BYTES);
        assert!(daily.take_dirty(), "trimming a seeded record is a change");
        for (gpu, entry) in &record.gpus {
            assert!(
                entry
                    .throttle_episodes
                    .last()
                    .expect("episodes retained")
                    .ended_at
                    .is_none(),
                "{gpu} lost its open episode"
            );
        }
        // The globally oldest closed episode is the first one evicted.
        let first = record.gpus.values().next().expect("a GPU is retained");
        assert!(!first.throttle_episodes[0].started_at.contains("T00:00:00"));
    }
}

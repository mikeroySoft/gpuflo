//! Source-backed health candidates, priority, and wording inputs.
//!
//! Health is one highest-priority factual sentence per physical GPU, never a
//! score. Busy activity and high memory occupancy never produce a candidate.

use crate::model::{Health, HealthCategory, Timestamp};

/// One active source-backed condition competing for the health sentence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HealthCandidate {
    pub category: HealthCategory,
    pub message: String,
    pub observed_at: Timestamp,
}

/// Canonical priority rank; lower wins. Unknown categories rank behind every
/// known active condition but ahead of `none`, so they are never mistaken
/// for normal health.
fn rank(category: &HealthCategory) -> u8 {
    match category.as_str() {
        "fault" => 0,
        "throttle" => 1,
        "limit" => 2,
        "telemetry" => 3,
        "memory_pressure" => 4,
        "none" => 6,
        _ => 5,
    }
}

/// Selects the highest-priority candidate; ties resolve to the newest source
/// time. With no candidates the factual normal sentence is produced.
pub(crate) fn select(candidates: &[HealthCandidate], assembled_at: Timestamp) -> Health {
    let best = candidates.iter().min_by(|a, b| {
        rank(&a.category)
            .cmp(&rank(&b.category))
            .then_with(|| b.observed_at.cmp(&a.observed_at))
    });
    match best {
        Some(candidate) => Health {
            category: candidate.category.clone(),
            message: candidate.message.clone(),
            observed_at: candidate.observed_at,
        },
        None => Health {
            category: HealthCategory::NONE,
            message: "no active limits or faults".to_owned(),
            observed_at: assembled_at,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn at(second: u8) -> Timestamp {
        Timestamp::from_odt(
            datetime!(2026-08-21 10:00 UTC) + time::Duration::seconds(second.into()),
        )
    }

    fn candidate(category: HealthCategory, message: &str, t: Timestamp) -> HealthCandidate {
        HealthCandidate {
            category,
            message: message.to_owned(),
            observed_at: t,
        }
    }

    #[test]
    fn priority_is_fault_throttle_limit_telemetry_pressure_none() {
        let candidates = vec![
            candidate(HealthCategory::MEMORY_PRESSURE, "pressure", at(5)),
            candidate(HealthCategory::TELEMETRY, "stale", at(5)),
            candidate(HealthCategory::LIMIT, "power limit active", at(5)),
            candidate(HealthCategory::THROTTLE, "thermal throttle", at(5)),
            candidate(HealthCategory::FAULT, "2 uncorrectable ECC errors", at(1)),
        ];
        let health = select(&candidates, at(9));
        assert_eq!(health.category, HealthCategory::FAULT);
        assert_eq!(health.message, "2 uncorrectable ECC errors");
        assert_eq!(health.observed_at, at(1));
    }

    #[test]
    fn newest_wins_within_a_category() {
        let candidates = vec![
            candidate(HealthCategory::THROTTLE, "older", at(1)),
            candidate(HealthCategory::THROTTLE, "newer", at(3)),
        ];
        assert_eq!(select(&candidates, at(9)).message, "newer");
    }

    #[test]
    fn no_candidates_is_factual_normal_text() {
        let health = select(&[], at(9));
        assert_eq!(health.category, HealthCategory::NONE);
        assert_eq!(health.message, "no active limits or faults");
        assert_eq!(health.observed_at, at(9));
    }

    #[test]
    fn unknown_category_outranks_none_but_not_known_conditions() {
        let candidates = vec![
            candidate(HealthCategory::new("future_condition"), "future", at(5)),
            candidate(HealthCategory::MEMORY_PRESSURE, "pressure", at(5)),
        ];
        assert_eq!(
            select(&candidates, at(9)).category,
            HealthCategory::MEMORY_PRESSURE
        );
        let only_unknown = vec![candidate(
            HealthCategory::new("future_condition"),
            "f",
            at(5),
        )];
        assert_eq!(
            select(&only_unknown, at(9)).category.as_str(),
            "future_condition"
        );
    }
}

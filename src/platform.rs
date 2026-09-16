//! Platform classification: what kind of AMD GPU is this?
//!
//! Every physical GPU is classified exactly once, at discovery, from its PCI
//! device ID plus whatever KFD heap-type evidence the source could gather.
//! This is the one place device-specific quirks get resolved, so adding a
//! new recognized device is a table entry here rather than a new `if`
//! branch scattered through a source adapter.

use crate::model::{MemoryPool, Platform, PlatformId};

/// Physical-memory evidence a source can offer about a GPU's memory model,
/// used as the fallback signal for devices absent from [`KNOWN_DEVICES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryEvidence {
    /// KFD-reported heap types indicate unified, GTT-backed memory.
    Unified,
    /// KFD-reported heap types indicate dedicated VRAM.
    Dedicated,
    /// No conclusive heap-type evidence.
    Unknown,
}

/// Devices whose heap-type evidence alone would misclassify them, keyed by
/// lowercase hex PCI device ID (no `0x` prefix). Growing this table to cover
/// another such device is a one-line addition, not new branching logic.
const KNOWN_DEVICES: &[(&str, Platform)] = &[(
    "1586",
    Platform {
        id: PlatformId::STRIX_HALO,
        memory_pool: MemoryPool::GTT,
    },
)];

/// Classifies one physical GPU from its PCI device ID and heap-type
/// evidence. Always resolves to a [`Platform`] — an unrecognized device
/// falls back to a generic classification from `evidence` rather than going
/// unclassified.
pub(crate) fn classify(device_key: &str, evidence: MemoryEvidence) -> Platform {
    if let Some((_, platform)) = KNOWN_DEVICES.iter().find(|(id, _)| *id == device_key) {
        return platform.clone();
    }
    match evidence {
        MemoryEvidence::Unified => Platform {
            id: PlatformId::GENERIC_APU,
            memory_pool: MemoryPool::GTT,
        },
        MemoryEvidence::Dedicated => Platform {
            id: PlatformId::GENERIC_DISCRETE,
            memory_pool: MemoryPool::VRAM,
        },
        MemoryEvidence::Unknown => Platform::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_device_wins_over_conflicting_heap_evidence() {
        // Strix Halo reports discrete-looking heap evidence but must still
        // classify as unified/GTT via the recognition table.
        let platform = classify("1586", MemoryEvidence::Dedicated);
        assert_eq!(platform.id, PlatformId::STRIX_HALO);
        assert_eq!(platform.memory_pool, MemoryPool::GTT);
    }

    #[test]
    fn unrecognized_device_falls_back_to_heap_evidence() {
        assert_eq!(
            classify("15bf", MemoryEvidence::Unified),
            Platform {
                id: PlatformId::GENERIC_APU,
                memory_pool: MemoryPool::GTT,
            }
        );
        assert_eq!(
            classify("740f", MemoryEvidence::Dedicated),
            Platform {
                id: PlatformId::GENERIC_DISCRETE,
                memory_pool: MemoryPool::VRAM,
            }
        );
    }

    #[test]
    fn inconclusive_evidence_classifies_unknown() {
        assert_eq!(
            classify("0000", MemoryEvidence::Unknown),
            Platform::default()
        );
    }

    #[test]
    fn no_platform_reports_npu_yet() {
        assert!(!classify("1586", MemoryEvidence::Unified).has_npu());
        assert!(!Platform::default().has_npu());
    }
}

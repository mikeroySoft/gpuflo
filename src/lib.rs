//! gruflo — read-only AMD GPU instrument library.
//!
//! Only the canonical model types re-exported here and the narrow
//! [`Monitor`] interface are semver-supported. Everything else is private
//! and may change without notice.

#![warn(missing_docs)]

mod cli;
mod config;
mod model;
mod normalize;
mod output;
mod persist;
mod source;
mod state;

pub use model::{
    Health, HealthCategory, InvalidPciBdf, Memory, MemoryPool, Observation, ObservationState,
    Partition, PartitionId, PciBdf, PhysicalGpu, PhysicalGpuId, Power, SCHEMA_VERSION, Snapshot,
    Temperature, Timestamp,
};

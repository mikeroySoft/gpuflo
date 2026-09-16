//! gpuflo — read-only AMD GPU instrument library.
//!
//! Only the canonical model types re-exported here and the narrow
//! [`Monitor`] interface are semver-supported. Everything else is private
//! and may change without notice.

#![warn(missing_docs)]

mod cli;
mod config;
mod model;
mod monitor;
mod normalize;
mod output;
mod persist;
mod platform;
mod run;
mod source;
mod state;
mod terminal;
mod ui;

pub use model::{
    Health, HealthCategory, InvalidPciBdf, Memory, MemoryPool, Observation, ObservationState,
    Partition, PartitionId, PciBdf, PhysicalGpu, PhysicalGpuId, Platform, PlatformId, Power,
    SCHEMA_VERSION, Snapshot, Temperature, Timestamp,
};
pub use monitor::{
    Monitor, MonitorClosed, MonitorCommand, MonitorError, MonitorEvent, MonitorOptions, Notice,
    ReceiveTimeoutError, ShutdownError, StartError,
};

/// Binary glue entrypoint: runs gpuflo from process arguments and
/// environment, returning the exit code. Public only for `src/main.rs`;
/// excluded from the supported reuse interface.
pub fn run_from_env() -> u8 {
    run::run_from_env()
}

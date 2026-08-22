# Gpuflo Minimal Rust Architecture Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gpuflo is one small, standalone Linux binary. Its architecture must isolate blocking and optional telemetry, preserve one canonical metric model across the TUI and machine outputs, keep visual animation separate from observations, and provide a narrow Rust reuse seam without importing rocm-cli's daemon/application structure.

The selected design is a **deep single-package monitor**: one Cargo package with a library and binary target, private source-specific adapters, bounded worker lanes, a coordinator-owned pure reducer, and a semver-supported canonical model plus monitor interface.

## Architectural invariants

- One Cargo package; no workspace and no daemon/API/IPC crate.
- One shipped `gpuflo` binary.
- Kernel telemetry is authoritative; AMD SMI is optional enrichment.
- No source or output surface mutates shared product state.
- One coordinator owns runtime state and sequences scheduling, normalization, reduction, publication, and lifecycle; the named modules own their rules.
- Every queue is bounded; slow consumers and superseded work create observable gaps rather than memory growth.
- TUI animation never changes canonical observations or machine output.
- Expected telemetry failures are observation states, not control-flow errors.
- Terminal restoration precedes worker joining and fatal diagnostics.
- Only the canonical model and monitor interface are external compatibility promises.

## Package shape

The repository contains one Cargo package named `gpuflo`:

```text
Cargo.toml
src/
├── main.rs
├── lib.rs
├── cli.rs
├── config.rs
├── model.rs
├── monitor.rs
├── normalize.rs
├── output.rs
├── persist.rs
├── terminal.rs
├── source/
│   ├── mod.rs
│   ├── kernel.rs
│   ├── amdsmi.rs
│   └── process.rs
├── state/
│   ├── mod.rs
│   ├── reducer.rs
│   ├── history.rs
│   └── health.rs
└── ui/
    ├── mod.rs
    ├── layout.rs
    ├── widgets.rs
    ├── theme.rs
    └── format.rs
```

`src/main.rs` calls one library entrypoint and maps its result to the process exit code. The entrypoint is public only as binary glue and is excluded from the supported reuse interface.

There are no `core`, `common`, `utils`, or `services` buckets. A module exists only when it owns a settled rule or hides source/runtime complexity behind a smaller interface.

## Module responsibilities

### `model`

Owns the semver-supported canonical vocabulary:

- `PhysicalGpuId`, `PartitionId`, and `PciBdf` newtypes;
- physical-GPU and XCP-scoped records;
- generic tagged `Observation<T>` values/states;
- health conditions and categories;
- memory pools;
- snapshot schema types and version;
- source-independent timestamps and units.

Observation-state, health-category, memory-pool, and topology values are string-backed unknown-safe types at serialization edges. Internal code must not collapse an unknown future string to a known healthy/default variant.

Display indexes are presentation fields, never identity keys. Backend handles and raw source identifiers do not appear in the canonical model.

### `monitor`

Presents the supported external monitor interface and privately owns:

- coordinator lifecycle;
- monotonic schedules;
- worker creation/removal;
- device rediscovery;
- bounded channel supervision;
- normalization and reducer invocation;
- sequence-numbered snapshot publication;
- command handling;
- fatal termination and shutdown.

### `source`

Contains three private, source-specific deep adapters:

- `kernel` — DRM/sysfs/hwmon/`gpu_metrics`, topology, and RAS;
- `amdsmi` — optional runtime-loaded AMD SMI enrichment and events;
- `process` — on-demand KFD/fdinfo process identity, association, and memory attribution.

The adapters do not implement one generic backend trait. Their authority, cadence, capabilities, and failure modes differ. Each emits owned source-specific samples and structured source failures.

Internal source-specific traits exist only where fixtures/fakes need a seam. They are not exported.

### `normalize`

Is the only source-to-domain seam. It:

- maps source sentinels/statuses/errors to observation states;
- validates numeric domains without clamping;
- converts units;
- applies kernel-first source precedence;
- assigns physical/XCP scope;
- maps source identities to canonical IDs;
- emits typed canonical observation batches for the reducer.

It does not own history, freshness, health priority, rendering, or output formatting.

### `state`

Owns the deterministic product model:

- current canonical observations;
- structural capabilities and runtime states;
- freshness transitions;
- topology and stable selection identity;
- fixed-capacity histories;
- session peaks;
- daily energy/peak summaries;
- highest-priority health condition;
- notices and fatal partition-change detection.

`reducer` applies typed events to owned state and mutates histories through `history`. `history` owns preallocated rings and daily accumulation rules. `health` owns source-backed priority/wording inputs. None performs I/O.

### `persist`

Loads and atomically writes the small daily summary set. It never persists raw observations or history.

`persist` receives an optional state path already resolved by `config`; it does not inspect XDG environment variables. `None` disables persistence for library embedders.

Writes use a temporary file plus atomic rename. A coalescing writer retains only the latest pending summary.

### `config`

Owns built-in defaults and one pure precedence merge:

```text
built-in defaults → optional TOML overrides → CLI flags
```

The result is one typed `AppConfig`, split once into monitor and presentation options. Sources, output, and UI never read environment variables or files directly. The normal config file remains blank; defaults stay in code.

### `output`

Formats canonical snapshots into one-shot text, pretty JSON, NDJSON, and tiny status output. It cannot access render animation/history and cannot reinterpret metric meaning.

### `ui`

Renders private immutable render models with ratatui. It owns only presentation state: selection focus, responsive mode choice, spring velocity, graph interpolation fraction, and overlay visibility. It does not derive health, freshness, peaks, or output-schema values.

### `terminal`

Owns staged RAII acquisition/restoration of raw mode, cursor visibility, and alternate-screen state. It is used only by the interactive surface.

### `cli`

Uses `lexopt` to parse the small fixed flag surface, renders explicit help/version text, loads configuration, and selects the output surface. It contains no telemetry or product-state rules.

## Supported Rust interface

The initial package semver supports only reexported canonical model types and this narrow monitor shape:

```rust
pub struct Monitor { /* private */ }

impl Monitor {
    pub fn start(options: MonitorOptions) -> Result<Self, StartError>;
    pub fn receive(&self) -> Result<MonitorEvent, MonitorClosed>;
    pub fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<MonitorEvent, ReceiveTimeoutError>;
    pub fn command(&self, command: MonitorCommand) -> Result<(), MonitorClosed>;
    pub fn shutdown(self) -> Result<(), ShutdownError>;
}

pub enum MonitorEvent {
    Snapshot(Snapshot),
    Notice(Notice),
    Fatal(MonitorError),
}

pub enum MonitorCommand {
    SetProcessScope(Option<PhysicalGpuId>),
    ResetSessionPeaks,
}

pub enum ReceiveTimeoutError {
    Timeout,
    Closed,
}
```

The signatures are normative in shape, not a commitment to field spelling before implementation compilation. The interface invariants are normative:

- received snapshots are owned and immutable;
- `Snapshot` uses the approved schema/domain meanings;
- notices are factual lifecycle transitions, not logs;
- fatal means no further snapshots will be produced;
- commands are bounded and may fail only when the monitor has terminated;
- shutdown is explicit and joins/flushes to its documented bound;
- `MonitorOptions` carries an optional daily-summary path; `None` disables persistence for embedders without changing telemetry semantics.

Raw samples, source adapters, worker requests/results, reducer events/state, histories, render models, persistence records, unsafe handles, CLI types, and terminal/UI functions are private and may evolve without semver guarantees.

Whether the library target is published to crates.io or consumed from the repository is a packaging decision, not an architecture change.

## Runtime topology

`Monitor::start` performs the approved preflight, then starts one coordinator thread.

The coordinator owns the live inventory/reducer instance and sequences:

- fast, slow, process, rediscovery, and cooldown schedules;
- worker-lane lifecycle;
- invocation of `normalize` and `state` in deterministic event order;
- sequence numbers and outward monitor events; and
- shutdown coordination.

It never performs potentially blocking source or filesystem operations itself.

### Worker lanes

- **Discovery lane:** performs post-startup DRM/PCI topology rescans so the coordinator never reads the filesystem.

- **Kernel lane per physical GPU:** gives fast reads priority and performs that GPU's bounded fast and slow kernel jobs. One physical GPU cannot occupy another GPU's lane.
- **AMD SMI enrichment lane:** one isolated optional lane containing all library state and FFI calls. Failure cannot stall kernel lanes.
- **Process lane:** one on-demand lane enabled only while `SetProcessScope(Some(id))` is active.
- **Persistence lane:** one latest-value coalescing lane for atomic daily-summary writes.

Source lanes have capacity-one request and result channels. Sending is nonblocking. A full channel means an older in-flight/pending operation already supersedes the new request, or a result has not yet been consumed; the new item is dropped rather than queued.

Outward delivery uses two bounded capacity-one mailboxes behind the single `MonitorEvent` receive interface: a lossy latest-snapshot mailbox and a priority control mailbox for notices/fatal termination. Receivers check control first. A snapshot can never displace a notice/fatal event, and fatal termination can replace a pending notice. Slow consumers still observe documented snapshot-sequence gaps.

There is no generic thread pool, unbounded queue, async runtime, or thread per metric.

## Scheduling

The coordinator uses monotonic time and skip-on-miss scheduling:

- kernel fast request: every 250 ms;
- render tick: 125 ms in the UI, outside the monitor;
- slow health request: every 1 second;
- process request: every 2 seconds only while active;
- rediscovery/circuit-breaker work: bounded schedules defined from architecture/validation evidence.

At most one operation is in flight per source/device lane. A shared kernel-lane job has a deadline shorter than the 250 ms fast cadence; due slow work is split into bounded jobs and dispatched only when the lane is idle after a fast result. Other lane operations have deadlines shorter than their own cadence. Late results are discarded and no missed tick is replayed.

Exact timeout/cooldown constants remain validation-owned because current research has no measured representative hardware latency. The scheduling and isolation invariants above are fixed.

The monitor uses the first fast collection as a priming observation and publishes only after the second fast collection. The first public sequence number is therefore the first exportable coherent snapshot required by the output contract.

## Snapshot production

At every 250 ms production tick, the coordinator:

1. drains all ready source results;
2. normalizes source-specific samples and failures;
3. applies canonical batches/events to the reducer;
4. evaluates freshness and health against current monotonic time;
5. updates history, peaks, and daily summaries only from new real observations;
6. assembles one all-GPU immutable `Snapshot` from the latest observations; and
7. increments sequence and attempts nonblocking publication.

There is no all-device barrier. A late physical GPU retains its prior source timestamp while fresh, then becomes stale under the approved rule. One slow/failing GPU cannot delay healthy GPUs or snapshot production.

A snapshot is assembled once per production tick regardless of how many source results arrived. Worker results never directly trigger public snapshots.

## Typed data pipeline

```text
kernel bytes / AMD SMI values / process files
                │
                ▼
      source-specific owned samples
                │
                ▼
 normalize: validate, scope, convert, precedence
                │
                ▼
       canonical observation batches
                │
                ▼
 reducer: freshness, topology, history, health
                │
        ┌───────┴────────┐
        ▼                ▼
 canonical Snapshot   private RenderModel
        │                │
 text/JSON/NDJSON      ratatui only
```

Parsing and validation stay next to source knowledge. Product semantics stay source-independent. Output and UI consume derived canonical state; they never parse telemetry.

## Canonical and render views

`state::history` owns preallocated 240-point rings for approved histories. The reducer mutates them through that module and projects two private/public views:

- the public canonical current `Snapshot`, which contains no history or visual values; and
- a private immutable `RenderModel`, containing only canonical current observations plus bounded history—never ratatui layout, strings, styles, or animation state.

Render data is refreshed only when observation state changes, at most on production ticks. The 125 ms UI loop reuses the latest render data and changes only spring/interpolation state. It does not rebuild/copy history, parse sources, serialize JSON, or recompute product health each frame.

Where immutable cross-thread render ownership requires a bounded history copy, the copy occurs only on the corresponding observation update, never per frame, and is limited to the fixed 240-point ring.

## Source precedence and FFI containment

Kernel observations are authoritative. A fresh kernel value cannot be overwritten by AMD SMI because the optional source is newer or more convenient. AMD SMI may:

- fill a field structurally unavailable from the kernel source; or
- add richer detail/health observations with distinct canonical fields.

All AMD SMI unsafety lives in `source::amdsmi`:

- `libloading` and SONAME discovery;
- symbol signatures;
- ABI/major-version validation;
- init/shutdown ownership;
- socket/processor handles;
- C string/struct conversion;
- documented sentinels;
- raw error/status mapping.

No raw pointer, borrowed C memory, library handle, or processor handle crosses the lane. Only owned typed samples or structured failures leave it.

## Freshness, error, and fatal flow

Expected telemetry failure is data:

- source adapters return structured source results;
- normalization maps expected failures to observation states;
- the reducer retains fresh prior observations, transitions stale values, and isolates affected scope;
- output/UI render the canonical state.

Typed errors control whether the requested operation can continue:

- `StartError` — host/preflight/monitor startup cannot proceed;
- `MonitorError` — coordinator cannot produce further valid snapshots;
- `OutputError` — requested serialization/write cannot continue;
- `TerminalError` — terminal acquisition/restoration failed;
- `ShutdownError` — bounded worker/persistence shutdown did not complete cleanly.

The binary is the only layer that maps fatal types to stderr and process exit codes. Library callers receive typed errors/events and choose their own process policy.

A confirmed partition configuration change is fatal. Ordinary metric/source/GPU failure is not.

## Device lifecycle

Discovery creates one kernel lane per physical GPU. Confirmed disappearance removes its lane and topology entry after freshness/rescan rules, emits a factual notice, and leaves other devices running.

If no GPUs remain after startup, the coordinator remains alive, continues rediscovery, and produces the approved empty stream snapshots. A returned/new GPU receives new source adapters and a full capability probe.

The TUI owns selected-display behavior. It consumes stable IDs/notices to choose the nearest surviving GPU and may restore selection when the same stable physical identity returns.

Partition topology is not live-migrated. Confirmation stops production with a fatal event so the application can restore terminal state and exit for restart.

## Configuration

`config` resolves config and state paths from explicit environment inputs, passes the optional daily-summary path into `MonitorOptions`, and performs no process-global mutation in tests. `persist` consumes that resolved path without reading the environment.

Configuration path:

```text
$XDG_CONFIG_HOME/gpuflo/config.toml
```

fallback:

```text
~/.config/gpuflo/config.toml
```

Daily summary path:

```text
$XDG_STATE_HOME/gpuflo/daily.json
```

fallback:

```text
~/.local/state/gpuflo/daily.json
```

Missing/blank config means built-in defaults. The separate configuration decision owns malformed-file behavior and the later configurable-key set. Sampling cadences, queue capacities, schema meanings, and source precedence are not generic tuning knobs.

## Persistence

The reducer owns daily-summary meaning and local-date rollover. Persistence only stores/loads the canonical summary record at the path supplied through `MonitorOptions`.

On startup, the persistence lane loads once before normal summary accumulation. On rollover or relevant state change, the coordinator replaces one latest-summary slot and sends a capacity-one wakeup; the writer always takes the newest complete state. Obsolete pending summaries are coalesced rather than queued.

Shutdown restores the terminal first, then requests a final summary flush and bounded worker joins. A persistence failure is surfaced as a transition/final shutdown error without corrupting current telemetry. Atomic rename prevents a partial JSON file from becoming the next startup state.

## Terminal and shutdown lifecycle

Interactive startup order:

1. parse config/CLI;
2. call `Monitor::start`, which performs host/device preflight and starts the coordinator;
3. acquire terminal modes under a staged RAII guard;
4. run the input/render loop.

Interactive shutdown/fatal order:

1. stop accepting/publishing application work;
2. restore cursor, raw mode, and alternate screen;
3. print any fatal diagnostic;
4. signal monitor/worker shutdown;
5. flush the latest daily summary;
6. wait on lane completion channels to a bounded deadline;
7. report/detach a nonresponsive lane and return the approved exit result.

Restoration is never delayed by a worker join. A detached in-process lane is a final fallback, not normal operation; validation must exercise bounded source behavior. SIGKILL/process abort remain untrappable.

Noninteractive surfaces never acquire terminal modes.

## Dependencies

Initial focused dependency set:

- `ratatui`, `crossterm` — terminal UI;
- `serde`, `serde_json`, `toml` — canonical schema/config/state serialization;
- `lexopt` — small explicit CLI parser;
- `time` — RFC 3339 timestamps and local-date rollover;
- `thiserror` — typed library errors;
- `crossbeam-channel` — dynamic selection over bounded worker lanes;
- `libloading` — runtime AMD SMI loading;
- `signal-hook` — catchable shutdown signals.

No Tokio, futures runtime, async traits, DI framework, plugin registry, `sysinfo`, network client/server, database, daemon/IPC stack, or general application framework.

Dependency versions and feature flags are fixed during implementation against current MSRV/build evidence, not by this architecture ticket.

## Test seams

Private injectable seams:

- kernel, AMD SMI, and process source-specific adapters;
- monotonic and wall-clock providers;
- summary store;
- terminal operations;
- explicit environment/path inputs.

Tests use the same reducer and supported monitor interface as production. No test mutates global time, environment, or filesystem paths in a way that races parallel tests.

Expected verification layers:

1. source parser/ABI fixtures for sentinels, layouts, malformed fields, and units;
2. pure normalization tests for precedence, scoping, and state mapping;
3. reducer transition tests for freshness, health, history, peaks, hotplug, partition fatality, and daily rollover;
4. monitor integration tests using fake lanes and deterministic clocks;
5. output contract fixtures and JSON parsing;
6. ratatui `TestBackend` characterization, widget buffer assertions, and squeezed-size sweeps;
7. ignored live-hardware smoke/latency tests owned by the hardware and validation tickets.

The validation/release decision sets the mandatory matrix, budgets, and release evidence.

## Performance and allocation invariants

- Preallocate every 240-point history ring.
- Never allocate/rebuild history, parse source bytes, derive health, or serialize output on a 125 ms render-only frame.
- Reuse source-specific read buffers where practical.
- Move owned samples/events through channels; do not copy raw blobs across stages.
- Bound every source request/result channel and outward snapshot/control mailbox at one item; coalesce persistence through one latest-value slot.
- Never queue missed ticks.
- Never wait for an all-device barrier.
- Never run optional AMD SMI/process/persistence work on a kernel lane.
- Keep snapshot/render copies bounded to canonical current state and fixed history at observation cadence.

## Reuse contract

Another Rust project, including rocm-cli, can embed the monitor without taking gpuflo's TUI, CLI, or terminal lifecycle. Daily-summary persistence is opt-in through `MonitorOptions`; disabling it does not change collection or canonical snapshot semantics. The caller learns one canonical model and one monitor interface.

The public interface is semver-supported from the initial package release. Private modules may be refactored freely. Packaging later decides publication/distribution channel; it must not split the package or widen the supported interface merely to publish it.

Copied or closely translated upstream implementations retain the already-approved provenance headers and notices. Reuse of gpuflo by rocm-cli is dependency reuse, not a reason to import rocm-cli's daemon or application types back into gpuflo.

## Rejected architectures

### Binary-first threaded monolith

Rejected because it keeps the prototype's App/render/history coupling, makes output semantics surface-specific, and offers no credible external seam. It is initially shorter but shallow: removing it does not concentrate complexity.

### Async multi-crate platform

Rejected because Tokio/tasks/watch channels and core/collectors/TUI/CLI crates add runtime, packaging, and interface weight without improving cancellation of blocking sysfs/FFI calls. It recreates the daemon-shaped architecture explicitly excluded by the map.

### Generic backend/plugin registry

Rejected because kernel telemetry and AMD SMI enrichment are not interchangeable and no third live backend exists. A generic provider interface would expose source-routing complexity rather than hide it.

## Explicit non-goals

- daemon or API server;
- local/remote IPC protocol;
- remote/fleet collection;
- multi-crate workspace;
- async runtime;
- generic metric/provider/plugin registry;
- database or persisted raw history;
- user scripting or extension hooks;
- source hot-swapping as a user feature;
- live partition-topology migration;
- public source/reducer/render/widget internals;
- importing rocm-cli's application, daemon, or `rocm-core` stack.

## Acceptance criteria

The architecture is settled when:

- one Cargo package produces one standalone binary and a narrow reusable library;
- the public interface exposes canonical model + monitor behavior and nothing source/UI-specific;
- kernel, optional enrichment, and process failures cannot stall one another or the UI;
- one coordinator owns every cross-source product invariant;
- source parsing, normalization, reduction, output, and rendering have one owner each;
- all surfaces consume the same canonical snapshot meanings;
- render-only animation cannot enter machine data;
- every queue is bounded and snapshot gaps are observable;
- terminal restoration precedes joins and fatal diagnostics;
- daily summaries remain atomic, tiny, and separate from config;
- unsafe AMD SMI code is localized to one adapter/lane;
- deterministic tests can replace sources, clocks, terminal operations, and state storage privately;
- deleting `monitor` would force scheduling, precedence, freshness, isolation, and lifecycle complexity back into every caller, demonstrating module depth; and
- no production implementation is created by this decision ticket.

## Evidence

- [Identify the reusable code boundary](https://github.com/mikeroysoft/gpuflo/issues/2)
- [Reuse boundary research](https://github.com/mikeroysoft/gpuflo/blob/research/reuse-boundary/research/reuse-boundary.md)
- [Inventory AMD telemetry sources and support](https://github.com/mikeroysoft/gpuflo/issues/8)
- [AMD telemetry source research](https://github.com/mikeroysoft/gpuflo/blob/research/amd-telemetry-sources/research/amd-telemetry-sources.md)
- [Define the metric and health contract](https://github.com/mikeroysoft/gpuflo/issues/5)
- [Set sampling, smoothing, and history semantics](https://github.com/mikeroysoft/gpuflo/issues/3)
- [Define sparse configuration behavior](https://github.com/mikeroysoft/gpuflo/issues/13)
- [Prototype the responsive dashboard language](https://github.com/mikeroysoft/gpuflo/issues/7)
- [Set the process overlay contract](https://github.com/mikeroysoft/gpuflo/issues/4)
- [Define the machine-readable output contract](https://github.com/mikeroysoft/gpuflo/issues/11)
- [Define capability, failure, and permission behavior](https://github.com/mikeroysoft/gpuflo/issues/10)

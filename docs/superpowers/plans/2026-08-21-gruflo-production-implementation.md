# Gruflo Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the complete local, read-only gruflo Rust library and binary from the approved Wayfinder specifications, with truthful kernel-first AMD GPU telemetry, bounded monitoring, canonical outputs, safe terminal lifecycle, responsive Ratatui UI, and release-ready artifacts.

**Architecture:** One Cargo package exposes only canonical model types and the narrow `Monitor` API. Private kernel, optional runtime AMD SMI, process, reducer, persistence, output, terminal, and UI modules feed one coordinator-owned state machine through capacity-one lanes; render animation remains presentation-only. The binary performs configuration and host preflight before terminal takeover, then selects one-shot, streaming, or interactive presentation over the same snapshots.

**Tech Stack:** Rust 2024; `ratatui`, `crossterm`, `serde`, `serde_json`, `toml`, `lexopt`, `time`, `thiserror`, `crossbeam-channel`, `libloading`, and `signal-hook`; standard-library filesystem, threads, atomics, and Unix process APIs.

---

## Fixed implementation decisions

- Base commit: `c7612004902ffb5a4f7d66672f9385f08c8f419b` on `origin/main`.
- Worktree: the existing linked worktree, repurposed onto `feature/production-implementation`; do not create a nested worktree.
- Package: one publishable package named `gruflo`, version `0.1.0`, edition 2024, library plus one `gruflo` binary, no Cargo features and no workspace.
- Public API: re-export canonical model vocabulary and `Monitor`, `MonitorOptions`, `MonitorEvent`, `MonitorCommand`, and typed lifecycle errors only.
- Kernel source: discover `card*` DRM devices whose PCI vendor is `0x1002` and driver is `amdgpu`; use textual device sysfs/hwmon for broadly stable metrics and inspect the four-byte versioned `gpu_metrics` header before parsing supported layouts. Unknown layout means `unsupported_driver_version`, never a guessed struct.
- Implemented `gpu_metrics` families: fixed v1.3, fixed v1.4-v1.8 common hero fields, APU v2.1-v2.4, and APU v3.0. Dynamic v1.9 is detected and explicitly represented as `unsupported_driver_version` until a pointer-free kernel payload contract is verified; stable text nodes continue to provide independent kernel observations.
- AMD SMI: runtime-load only known SONAME candidates, validate required symbol presence and library version, keep every raw handle/pointer/function pointer inside `source::amdsmi`, and enrich only fields without a fresh kernel value. No `amd-smi` subprocess.
- Process overlay: scan only while visible, at two seconds, on its own lane. Parse DRM fdinfo by `drm-pdev`; parse KFD membership and `vram_<gpuid>` independently; report both evidence sources without summing or reconciling them; never expose utilization or engine-time claims.
- Test-only host injection: debug/test builds may accept a private explicit sysfs/proc root and fatal-after-acquisition trigger used by integration/PTY tests. Release builds ignore these hooks. They are not CLI options or supported API.
- Responsive surfaces: `mode` at `>=72×34`, compact at `>=62×17`, mini at `>=48×11`, tiny otherwise. Forced preferences fall back to the richest fitting surface. `p` toggles process overlay, `d` toggles detail, `?` toggles help, `t` cycles theme, `m` cycles preferred mode, arrows select physical GPU, and `q`/Escape quits.
- Persistence: atomically store only per-day, per-physical-GPU energy and activity/memory peaks under the resolved state path. Never persist raw samples or the user-owned TOML.
- Validation: only the handoff’s fixtures, load-bearing reducer/model checks, one monitor journey, output contracts, representative/swept Ratatui frames, and exactly three PTY restoration journeys.

## Planned file ownership

```text
Cargo.toml                         package metadata and pinned dependency surface
Cargo.lock                         locked dependency graph
LICENSE                            project MIT license
README.md                          install, prerequisites, use, output, optional sources
THIRD_PARTY_NOTICES.txt            generated locked-dependency and reused-code notices
.github/workflows/ci.yml           MSRV/current-stable deterministic gate
.github/workflows/release.yml      tagged x86_64 archive and SHA256SUMS production
src/main.rs                        process exit mapping only
src/lib.rs                         private modules, public re-exports, binary glue entrypoint
src/cli.rs                         lexopt parsing/help/version/output selection
src/config.rs                      sparse TOML, explicit environment inputs, precedence/path resolution
src/model.rs                       canonical semver-supported IDs, observations, topology, health, snapshots
src/monitor.rs                     public monitor API and private coordinator/lane lifecycle
src/normalize.rs                   source-to-canonical validation, unit conversion, scope, precedence
src/output.rs                      semantic human/JSON/NDJSON writers
src/persist.rs                     atomic latest-summary persistence lane
src/terminal.rs                    staged terminal acquisition/restoration and signal-aware lifecycle
src/source/mod.rs                  private owned source request/result vocabulary
src/source/kernel.rs               discovery, sysfs/hwmon/gpu_metrics/RAS parsing and collection
src/source/amdsmi.rs               runtime FFI loading, ABI validation, owned enrichment samples
src/source/process.rs              DRM fdinfo/KFD/process/container attribution
src/state/mod.rs                   private reducer state and render projection
src/state/reducer.rs               deterministic topology/freshness/precedence transitions
src/state/history.rs               preallocated 240-point rings, peaks, daily summary accumulation
src/state/health.rs                factual priority and wording
src/ui/mod.rs                      input/render loop and overlays
src/ui/layout.rs                   fit policy and responsive composition
src/ui/widgets.rs                  braille graph, mode panels, process/detail/help overlays
src/ui/theme.rs                    buffalo/nord/monochrome semantic palettes
src/ui/format.rs                   observation-aware display formatting and spring/trend helpers
tests/fixtures/kernel/**           provenance-labelled SPX, XCP, APU, hwmon, RAS, gpu_metrics fixtures
tests/fixtures/process/**          sanitized fdinfo and KFD fixtures from live evidence plus boundaries
tests/monitor_journey.rs           public API journey with bounded fake lanes
tests/output_contract.rs           semantic text/JSON/NDJSON checks
tests/ui_contract.rs               representative frames and breakpoint sweep
tests/pty_restoration.rs           normal, signal, and injected-fatal compiled-binary journeys
```

### Task 1: Establish package, canonical model, CLI, and sparse configuration

**Files:**
- Create: `.gitignore`
- Create: `Cargo.toml`
- Create: `Cargo.lock`
- Create: `LICENSE`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/model.rs`
- Create: `src/cli.rs`
- Create: `src/config.rs`

- [ ] **Step 1: Add failing canonical-model tests**

Define tests before production implementations for:

```rust
#[test]
fn stale_observation_serializes_without_a_value();
#[test]
fn unknown_observation_state_round_trips();
#[test]
fn snapshot_keeps_socket_and_partition_scope_separate();
```

The wished-for API is `Observation<T>::value`, `Observation::unavailable`, string-backed `ObservationState`, `PhysicalGpu`, `Partition`, and `Snapshot::new`. Decode serialized JSON and assert no stale `value`, no `null`, and no physical activity aggregate.

Run: `cargo test --lib model::tests -- --nocapture`

Expected: FAIL because package/model symbols do not exist.

- [ ] **Step 2: Create the package and canonical model**

Create the normative module tree in `src/lib.rs`; keep private modules private and re-export only model and monitor types. Implement:

```rust
#[serde(untagged)]
pub enum Observation<T> {
    Value { value: T, observed_at: Timestamp },
    Unavailable { state: ObservationState, #[serde(skip_serializing_if = "Option::is_none")] observed_at: Option<Timestamp> },
}

pub struct Snapshot {
    pub schema_version: u32,
    pub gruflo_version: String,
    pub sampled_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    pub gpus: Vec<PhysicalGpu>,
}
```

Use validated `PciBdf`, `PhysicalGpuId`, and `PartitionId` newtypes. `ObservationState`, `HealthCategory`, and `MemoryPool` are transparent string-backed types with named constants so unknown future values deserialize unchanged. Physical GPU owns health/temperature/power and nested partitions; partitions own activity/memory/GFX clock/memory-controller activity. Do not expose backend handles.

Add `.gitignore` entries for `/target/`, editor artifacts, generated archives, and local validation output only.

- [ ] **Step 3: Verify model tests pass**

Run: `cargo test --lib model::tests -- --nocapture`

Expected: PASS.

- [ ] **Step 4: Add failing CLI/config precedence tests**

Cover exact flags, mutual exclusion of `--once`/`--json`/`--json-stream`/`--tiny`, GPU selector forms, unknown flags, help/version, closed TOML keys, blank TOML, defaults→TOML→CLI precedence, explicit XDG/HOME path resolution, and unconditional non-empty `NO_COLOR`.

Run: `cargo test --lib cli::tests config::tests -- --nocapture`

Expected: FAIL because parsers are not implemented.

- [ ] **Step 5: Implement CLI and configuration**

Use `lexopt`; accepted options are exactly `--help`, `--version`, `--once`, `--json`, `--json-stream`, `--tiny`, `--gpu`, `--theme`, `--mode`, and `--no-color`. Implement a closed `#[serde(deny_unknown_fields)]` sparse TOML with only `theme`, `mode`, and `no_color`. Resolve paths from an explicit environment-value struct; missing/blank config is valid; malformed, wrong-type, and unknown-key files are startup errors with path context. Split resolved options once into monitor and presentation options.

`src/main.rs` calls one `gruflo::run_from_env()` entrypoint and maps its returned outcome to exit 0/1/2/130.

- [ ] **Step 6: Verify and commit the foundation**

Run:

```bash
cargo fmt --check
cargo test --lib model::tests -- --nocapture
cargo test --lib cli::tests -- --nocapture
cargo test --lib config::tests -- --nocapture
cargo check --all-targets
```

Commit: `feat: establish canonical gruflo package`

### Task 2: Implement deterministic reducer, history, health, and persistence

**Files:**
- Create: `src/state/mod.rs`
- Create: `src/state/reducer.rs`
- Create: `src/state/history.rs`
- Create: `src/state/health.rs`
- Create: `src/persist.rs`

- [ ] **Step 1: Add failing state-transition tests**

Table-test the approved invariants:

```rust
fresh_value -> failed_read_before_deadline => retained current value, no history append
fresh_value -> deadline_elapsed => stale with last observed_at and no numeric value
stale -> fresh_value => append exactly once and update peak
kernel_fresh + amdsmi_newer => kernel remains authoritative
socket metric batch => physical GPU only
secondary partition socket metric => reported_by_primary_partition
fault > throttle > limit > telemetry > memory_pressure > none
one metric/GPU failure => independent values survive
partition identity change => fatal restart event
```

Run: `cargo test --lib state:: -- --nocapture`

Expected: FAIL because reducer/history/health do not exist.

- [ ] **Step 2: Implement fixed-capacity history and reducer**

Use a preallocated 240-element ring with explicit `push_fresh` only. Track source cadence, last success, last attempted generation, and authority internally so retained values do not append or alter peaks. The stale threshold is `max(1s, 3 × cadence)`. Represent typed normalized batches by stable ID and scope. Preserve device selection identity across disappearance/return; emit notices for disconnect; emit fatal on confirmed partition topology change.

- [ ] **Step 3: Implement health selection**

Store source-backed health candidates with category, message, and source time. Select one by canonical category priority and then newest source time within a category. Busy activity/high occupancy produce no candidate. Contracted telemetry state contributes telemetry health; optional detail/process failures do not.

- [ ] **Step 4: Add failing persistence tests**

Cover missing file, valid load, malformed file, local-day rollover, coalescing to newest summary, temp-file plus atomic rename, and a write failure that leaves the previous complete file intact.

Run: `cargo test --lib persist:: state::history:: -- --nocapture`

Expected: FAIL on missing persistence implementation.

- [ ] **Step 5: Implement daily-summary persistence**

Persist only date plus per-physical-GPU activity peak, memory peak, and energy when derivable. `persist` receives `Option<PathBuf>` and never reads environment variables. Use same-directory temporary files, flush, and rename. Use one latest-value slot plus capacity-one wakeup. Startup load and final flush are bounded; no raw observation/history is serialized.

- [ ] **Step 6: Verify and commit deterministic state**

Run:

```bash
cargo fmt --check
cargo test --lib state:: -- --nocapture
cargo test --lib persist:: -- --nocapture
cargo check --all-targets
```

Commit: `feat: add canonical reducer and summaries`

### Task 3: Implement kernel discovery and telemetry parsing

**Files:**
- Create: `src/source/mod.rs`
- Create: `src/source/kernel.rs`
- Create: `src/normalize.rs`
- Create: `tests/fixtures/kernel/**`

- [ ] **Step 1: Add provenance-labelled kernel fixtures**

Create the smallest source trees and binary blobs that express behavior differences:

- discrete SPX with `gpu_busy_percent`, VRAM, hwmon hotspot/power/cap/GFX clock;
- APU with GTT/shared memory and centi-unit v2.4 metrics;
- multi-XCP topology with socket ownership and secondary partition states;
- valid v1.3, v1.6 common fields, v2.4, and v3.0 metric blobs;
- unsupported v1.9 header, unknown version, truncated recognized layout, malformed scalar, `EPERM` seam, throttle/RAS values.

Each fixture directory contains `PROVENANCE.txt` stating captured, reduced-upstream, or synthesized boundary origin. Keep all identifying data sanitized.

- [ ] **Step 2: Add failing parser/discovery/normalization tests**

Tests assert AMD vendor+amdgpu binding gates discovery, BDF and stable IDs parse correctly, absent/sentinel/version/permission/asleep/malformed cases map to exactly one canonical state, units convert without clamping, discrete vs APU pools remain explicit, socket/XCP scope is not duplicated, and kernel values defeat AMD SMI values.

Run: `cargo test --lib source::kernel:: normalize:: -- --nocapture`

Expected: FAIL because source and normalize implementations are absent.

- [ ] **Step 3: Implement owned source vocabulary and kernel discovery**

All adapters emit owned records. Discovery takes an explicit root, never debugfs, resolves DRM card entries, verifies vendor and driver, reads PCI BDF/model/optional IDs, locates hwmon, identifies memory pool, and constructs physical/XCP topology. Missing optional nodes become structural observations; an unreadable existing node is permission denied; an injected runtime-suspend `EPERM` is asleep.

- [ ] **Step 4: Implement versioned `gpu_metrics` parsing and text collection**

Read the header as little-endian `{structure_size, format_revision, content_revision}` and reject lengths before field access. Parse only the listed supported layouts using bounds-checked cursor/offset functions; no transmute and no borrowed struct overlay. Recognize documented sentinels before numeric validation. Prefer one coherent supported metrics blob, then use stable text nodes for independent or unsupported fields. Fast collection reads activity, memory, hotspot, socket power, and optional memory-controller activity. Slow collection reads cap/limits, clocks, throttle/violations available from implemented layout, RAS counts, and bad-page state. Reuse per-lane buffers.

- [ ] **Step 5: Implement normalization**

Convert bytes, micro-watts, milli/centi-Celsius, hertz, and source timestamps into canonical units. Validate finite/range semantics but never clamp. Apply kernel-first precedence and correct physical/XCP scope. Produce typed reducer batches; do not own freshness/history/health.

- [ ] **Step 6: Verify and commit kernel telemetry**

Run:

```bash
cargo fmt --check
cargo test --lib source::kernel:: -- --nocapture
cargo test --lib normalize:: -- --nocapture
cargo check --all-targets
```

Commit: `feat: collect kernel AMD GPU telemetry`

### Task 4: Implement optional AMD SMI and process attribution sources

**Files:**
- Create: `src/source/amdsmi.rs`
- Create: `src/source/process.rs`
- Create: `tests/fixtures/process/**`
- Modify: `src/source/mod.rs`
- Modify: `src/normalize.rs`

- [ ] **Step 1: Add failing AMD SMI loader tests**

Use a tiny test-only shared library built by the test harness or symbol-resolver seam to prove: no library is a normal disabled source, missing required symbols and unknown ABI disable only enrichment, initialization/shutdown happen exactly once, status maps preserve unsupported/permission/source error, and returned BDF/activity/memory values are copied into owned Rust records before the call returns.

Run: `cargo test --lib source::amdsmi:: -- --nocapture`

Expected: FAIL because runtime loader is absent.

- [ ] **Step 2: Implement contained runtime AMD SMI enrichment**

Try versioned SONAMEs followed by unversioned name without linking at build time. Resolve only read APIs: library version, init/shutdown, socket/processor enumeration, BDF, busy percent, and VRAM total/usage. Validate ABI-compatible major versions and every required symbol before init. Map each processor to canonical BDF/XCP identity; emit owned samples. Keep `Library`, raw handles, C pointers, function pointers, C strings, and all `unsafe` in this module. Never call mutating APIs and never spawn `amd-smi`.

- [ ] **Step 3: Add failing process fixture tests**

Reduce the live result into sanitized fixtures and add boundaries for missing names, unreadable fdinfo, malformed units, multiple fds for one process, two GPUs, KFD-only membership, and container cgroup identity. Assert resident VRAM/GTT is converted with 1024, fdinfo and KFD memory remain separate provenance fields, rows sort by attributed memory, and no process utilization or engine-time field exists.

Run: `cargo test --lib source::process:: -- --nocapture`

Expected: FAIL because parser/scanner is absent.

- [ ] **Step 4: Implement process scanning**

Enumerate numeric `/proc` entries only on demand. Read permitted `comm`/`exe`, fdinfo files, `drm-pdev`, non-deprecated `drm-resident-vram`/`gtt`, and cgroup container identity. Inspect KFD process directories for GPU membership and `vram_<gpuid>` as a separate reported quantity. Retain rows with permission-denied fields instead of hiding them. Do not read command lines. Deduplicate per PID/GPU source records without reconciling KFD and fdinfo totals.

- [ ] **Step 5: Verify and commit optional sources**

Run:

```bash
cargo fmt --check
cargo test --lib source::amdsmi:: -- --nocapture
cargo test --lib source::process:: -- --nocapture
cargo check --all-targets
```

Commit: `feat: add optional enrichment and process attribution`

### Task 5: Implement bounded monitor coordinator and public API

**Files:**
- Create: `src/monitor.rs`
- Modify: `src/lib.rs`
- Modify: `src/source/mod.rs`
- Modify: `src/state/**`
- Create: `tests/monitor_journey.rs`

- [ ] **Step 1: Add the failing public monitor journey**

Through a private fake-lane constructor exercised by the public API types, prove:

1. first sample primes and second sample produces sequence 1 within the deterministic 1-second budget;
2. fast requests schedule at 250 ms, slow at 1 second, process at 2 seconds only when scoped;
3. each request/result/command mailbox has capacity one and missed work is skipped;
4. a slow receiver observes a sequence gap rather than memory growth;
5. control is received before snapshot and fatal replaces a pending notice;
6. `SetProcessScope` and `ResetSessionPeaks` work;
7. shutdown joins/flushes within its bound.

Run: `cargo test --test monitor_journey -- --nocapture`

Expected: FAIL because `Monitor` is absent.

- [ ] **Step 2: Implement lanes and coordinator**

One coordinator thread owns schedules, reducer, discovery lifecycle, normalization order, sequence generation, publication, and shutdown. Discovery has one lane; each physical GPU has one fast-priority kernel lane; AMD SMI, process, and persistence each have isolated lanes. Every source request/result channel and command channel is `bounded(1)` with nonblocking dispatch. Deadlines are shorter than cadence; late results are discarded; no all-device barrier or replayed tick exists.

- [ ] **Step 3: Implement two outward mailboxes behind one receive API**

Keep latest snapshot and priority control in independent capacity-one mailboxes. `receive`/`receive_timeout` check control first. Snapshot send replaces the pending snapshot. Notice send never touches snapshots. Fatal replaces a pending notice, closes production, and guarantees no later snapshot.

- [ ] **Step 4: Implement topology lifecycle**

Startup requires one discovered GPU. Runtime confirmed disappearance removes that GPU after rescan, emits one notice, and permits empty snapshots while rediscovery continues. Return/new GPU re-probes. Confirmed partition configuration change emits fatal restart. Optional lane failure never terminates kernel operation.

- [ ] **Step 5: Verify and commit monitor runtime**

Run:

```bash
cargo fmt --check
cargo test --test monitor_journey -- --nocapture
cargo test --lib monitor:: -- --nocapture
cargo check --all-targets
```

Commit: `feat: add bounded monitor coordinator`

### Task 6: Implement text, JSON, NDJSON, and binary flow

**Files:**
- Create: `src/output.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `src/cli.rs`
- Create: `tests/output_contract.rs`

- [ ] **Step 1: Add failing semantic output tests**

Decode JSON and assert schema version, version, timestamps, sequence rules, tagged observations, nested physical/XCP scope, field-name units, no `null`/non-finite/stale value/unavailable zero, unknown string round-trip, pretty final newline, compact one-object-per-line NDJSON, and all-GPU scope. Assert human semantic order and every canonical unavailable phrase. Assert no ANSI bytes.

Run: `cargo test --test output_contract -- --nocapture`

Expected: FAIL because output formatters are absent.

- [ ] **Step 2: Implement output writers from `Snapshot` only**

`--once` writes one full line per physical GPU using primary partition values; `--tiny` writes one selected physical GPU line; `--json` writes one pretty snapshot; `--json-stream` writes flushed compact snapshots at production cadence. Output never sees render state/history. Broken pipe returns success; other writes are fatal. One-shot modes wait for the first exportable second sample.

- [ ] **Step 3: Wire binary exit semantics and selectors**

Preflight config/CLI errors exit 2 without stdout or terminal acquisition. Startup/no snapshot/non-pipe runtime failures exit 1. SIGINT exits 130. Success, partial observations, and broken pipe exit 0. `--gpu` selects by display index, stable ID, or BDF only for tiny/TUI initial selection and never filters all-GPU outputs.

- [ ] **Step 4: Verify and commit outputs**

Run:

```bash
cargo fmt --check
cargo test --test output_contract -- --nocapture
cargo run -- --help
cargo run -- --version
cargo check --all-targets
```

Commit: `feat: add canonical gruflo outputs`

### Task 7: Implement terminal safety and responsive Ratatui UI

**Files:**
- Create: `src/terminal.rs`
- Create: `src/ui/mod.rs`
- Create: `src/ui/layout.rs`
- Create: `src/ui/widgets.rs`
- Create: `src/ui/theme.rs`
- Create: `src/ui/format.rs`
- Modify: `src/lib.rs`
- Create: `tests/ui_contract.rs`
- Create: `tests/pty_restoration.rs`

- [ ] **Step 1: Add failing staged terminal tests**

Use an injected terminal-operations seam to prove each partial acquisition unwinds only completed stages and every restoration attempts cursor show, raw disable, and alternate-screen leave in that order even when one step fails.

Run: `cargo test --lib terminal:: -- --nocapture`

Expected: FAIL because terminal guard is absent.

- [ ] **Step 2: Implement RAII terminal ownership and signals**

Acquire only after monitor preflight. Record each completed stage. Restore cursor, raw mode, then alternate screen before any fatal diagnostic, monitor shutdown, persistence flush, or join. Install signal flags for SIGINT/SIGTERM/SIGHUP and a panic hook that best-effort restores without panicking. Noninteractive modes never instantiate the guard.

- [ ] **Step 3: Add failing UI contract tests**

Render canonical fixture snapshots at `120×40`, `80×24`, `60×16`, `40×8`, and `20×1`. Assert each expected surface, the six-row GRUFLO logo and exact tagline in mode, selected-GPU marker, factual health, explicit no-color markers, and no panic/overflow in a bounded sweep around breakpoints. Add direct braille gap/packing and spring finite-input tests.

Run: `cargo test --test ui_contract -- --nocapture`

Expected: FAIL because UI is absent.

- [ ] **Step 4: Port approved visual behavior onto canonical render data**

Implement buffalo/nord/monochrome semantic themes; no-color strips semantic/decorative colors while retaining labels and markers. Preserve centered mode, six-row logo, `Power in, tokens out.`, overview strip, activity/memory panels, support row, one health sentence, breathing spring, 125 ms render cadence, 240-point braille history with sub-cell interpolation, and arrow selection. Compact/mini/tiny omit whole semantic segments rather than clip. Session `t`/`m` never writes TOML.

- [ ] **Step 5: Implement detail, process, and help overlays**

`p` sends process scope only while open and shows PID, permitted name or exact state, GPU/XCP association, fdinfo resident memory, separately labelled KFD memory, and container identity. `d` shows supported secondary telemetry/source versions and exact unavailable states. `?` shows keys, active theme/preference/effective surface, and attribution limitations. Overlay failures never change mode health.

- [ ] **Step 6: Add exactly three failing PTY journeys**

Compile and drive the actual debug binary against the private fixture host root:

1. enter interactive mode, send `q`, assert restore sequences;
2. enter interactive mode, send SIGINT, assert restore and exit 130;
3. trigger injected post-acquisition fatal, assert restoration precedes stderr diagnostic and exit 1.

Run: `cargo test --test pty_restoration -- --nocapture`

Expected: FAIL before lifecycle wiring, then PASS after wiring.

- [ ] **Step 7: Verify and commit terminal/UI**

Run:

```bash
cargo fmt --check
cargo test --lib terminal:: -- --nocapture
cargo test --test ui_contract -- --nocapture
cargo test --test pty_restoration -- --nocapture
cargo check --all-targets
```

Commit: `feat: add safe responsive terminal UI`

### Task 8: Complete packaging, provenance, documentation, and release automation

**Files:**
- Create: `README.md`
- Create: `THIRD_PARTY_NOTICES.txt`
- Create: `.github/workflows/ci.yml`
- Create: `.github/workflows/release.yml`
- Modify: `Cargo.toml`
- Modify: source files only where copied/closely translated code requires provenance headers

- [ ] **Step 1: Fix package metadata and MSRV**

Pin exact direct dependency versions/features in `Cargo.toml`, record the oldest compiler demonstrated by the resolved graph as `rust-version`, and include repository/license/readme/minimal keywords/categories. Verify `cargo package --list` contains source, README, LICENSE, notices, and no fixtures/results that should not ship.

- [ ] **Step 2: Add license and generated notices**

Use the MIT project license. Generate `THIRD_PARTY_NOTICES.txt` from `Cargo.lock` with the accepted dependency-license set and include exact flow/rocm-cli/btop notices only for files actually copied or closely translated, with pinned upstream revisions. Add per-file provenance headers to those files. Do not claim copied-code provenance for ideas-only implementation.

- [ ] **Step 3: Write concise user documentation**

Document archive/crates.io installation, Linux+amdgpu prerequisite, first run, keys, exact output modes, config path and three keys, optional AMD SMI/process limitations, read-only/no-network guarantee, uninstall, observation states, fixture-vs-hardware qualification language, build/test commands, and no unsupported platform claim.

- [ ] **Step 4: Add CI and release workflows**

CI runs formatting, current stable build/test/clippy, selected MSRV build/test, and package verification. Tagged release reruns the deterministic gate, builds `x86_64-unknown-linux-gnu`, creates exactly `gruflo`, `LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `README.md` at archive root, writes `SHA256SUMS`, checks `gruflo --version`, and publishes artifacts. It does not publish crates automatically without repository credentials/approval.

- [ ] **Step 5: Verify and commit release artifacts**

Run:

```bash
cargo fmt --check
cargo check --all-targets
cargo package --allow-dirty
cargo run -- --version
```

Commit: `docs: prepare gruflo package and release`

### Task 9: Run the minimal release gate and actual smoke journeys

**Files:**
- Modify only concrete defects found by the gate
- Create: `validation/0.1.0-rc.1.md`

- [ ] **Step 1: Run deterministic code gates**

Run fresh and capture exact counts/results:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --target x86_64-unknown-linux-gnu
cargo package
```

- [ ] **Step 2: Exercise actual noninteractive binary paths**

Against the private fixture host root, run the compiled binary for `--once`, `--tiny`, `--json`, and a bounded `--json-stream` pipeline. Decode produced JSON/NDJSON separately. Prove broken-pipe exit 0, config/usage exit 2, startup no-GPU exit 1, and SIGINT exit 130.

- [ ] **Step 3: Exercise actual interactive PTY paths**

Run the compiled binary in a PTY at representative sizes. Exercise mode/compact/mini/tiny resize transitions, GPU arrows, theme and mode cycling, process/detail/help overlays, normal quit, signal, and fatal restoration. Record observed exit/status and terminal sequences; do not substitute unit-test results for the actual process.

- [ ] **Step 4: Record truthful validation status**

Write one concise candidate manifest with version/commit/target/compiler, deterministic gate, exact smoke commands, PTY results, and telemetry regime claims. Current non-amdgpu WSL execution is fixture validation only. Preserve the prior live process-attribution evidence as process evidence, but do not claim full live hardware qualification or fabricate latency/perturbation results. Name physical hardware qualification as the sole external release prerequisite if unavailable.

- [ ] **Step 5: Commit verified corrections and manifest**

Commit: `test: complete gruflo release validation`

### Task 10: Review, repair, push, and open the PR

**Files:**
- Modify any file implicated by confirmed review findings

- [ ] **Step 1: Run specification-compliance review**

Review the entire diff from `c761200` against every handoff definition-of-done item and canonical acceptance criterion. Specifically search for fake zeroes, `null`, stale values, wrong scope, unbounded channels, direct `amd-smi`, debugfs, mutating APIs, non-restored terminal paths, legacy full-view terminology, placeholder text, unsupported process-utilization claims, and public internal types.

- [ ] **Step 2: Run Rust/code-quality/security reviews**

Dispatch the mandatory Rust/code reviewer and a focused security reviewer for FFI, filesystem traversal, config parsing, terminal signals, and `/proc` handling. Fix every Critical and Important finding; re-review until both pass. Remove temporary scaffolding and dead code only after behavioral smoke tests pass.

- [ ] **Step 3: Re-run the complete fresh gate**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --target x86_64-unknown-linux-gnu
cargo package
```

Repeat the four noninteractive smoke paths and three PTY restoration journeys after final fixes.

- [ ] **Step 4: Verify repository and definition of done**

Confirm the normative module tree, one package/library/binary, locked graph, license/notices/readme/workflows, no tracked build artifacts, coherent commit series, no uncommitted source changes, and every handoff definition-of-done item either proven or identified solely as external physical-hardware qualification.

- [ ] **Step 5: Push and create ready-to-merge PR**

Push `feature/production-implementation` to `origin` and create a PR targeting `main`. The PR body lists architecture/output/UI changes, exact build/test/package/binary/PTY evidence, fixture-qualified regimes, the existing live process-attribution evidence, and the explicit absence of a full hardware qualification claim. Keep the worktree for PR iteration.

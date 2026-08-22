# Gpuflo Production Implementation Handoff

## Mission

Build the production implementation of **gpuflo** from the completed Wayfinder product and architecture specification.

Gpuflo is a small, read-only, local Rust instrument for Linux `amdgpu` hosts. Within one second it must answer:

1. What is the selected physical AMD GPU doing?
2. How much applicable GPU memory is occupied?
3. Is a source-backed fault, throttle, limit, pressure, sleep, permission, or telemetry condition active?

The Wayfinder effort is complete. All decision tickets and the canonical map are closed. The next agent is expected to plan and execute production implementation—not reopen settled product questions or stop after producing another specification.

## Repository state

- Repository: `https://github.com/mikeroysoft/gpuflo.git`
- Main checkout used for this handoff: `/home/miroy/git/gpuflo`
- Branch: `main`
- Published head: `0e34de3` (`research: apply live process attribution findings`)
- `main` matches `origin/main`.
- No open GitHub issues remain.
- Production `Cargo.toml` and `src/` do not yet exist on `main`.
- Root `HANDOFF.md` is an old, untracked prototype-era document. It is user-owned historical work: ignore it and do not modify or delete it.

Create implementation work on an isolated feature branch/worktree. Keep `main` as the integration baseline.

## Canonical inputs

Read these before planning or editing:

1. `CONTEXT.md` — canonical domain glossary.
2. `docs/superpowers/specs/2026-08-20-gpuflo-metric-health-design.md`
3. `docs/superpowers/specs/2026-08-20-gpuflo-machine-readable-output-design.md`
4. `docs/superpowers/specs/2026-08-20-gpuflo-capability-failure-design.md`
5. `docs/superpowers/specs/2026-08-20-gpuflo-rust-architecture-design.md`
6. `docs/superpowers/specs/2026-08-20-gpuflo-validation-release-design.md`
7. `docs/superpowers/specs/2026-08-20-gpuflo-packaging-release-design.md`
8. `docs/superpowers/specs/2026-08-20-gpuflo-presentation-configuration-design.md`
9. `research/process-attribution/results/20260821T181207370038903Z/` — live HIP process-attribution evidence.
10. `docs/superpowers/specs/2026-08-20-process-attribution-capture-harness-design.md` only when maintaining the research harness; it is not a production architecture input.

Tracker context remains available in the closed canonical map:

- <https://github.com/mikeroysoft/gpuflo/issues/1>

Detailed research remains on published branches:

- `origin/research/amd-telemetry-sources` → `research/amd-telemetry-sources.md`
- `origin/research/reuse-boundary` → `research/reuse-boundary.md`

## Approved prototype

The approved throwaway Ratatui prototype is not on `main`:

- Worktree: `/home/miroy/.config/superpowers/worktrees/gpuflo/responsive-dashboard`
- Branch: `prototype/responsive-dashboard`
- Approved commit: `699a425`
- Main implementation reference: `src/main.rs`

Use it for visual behavior only:

- warm Buffalo palette;
- centered full `mode` with six-row GPUFLO logo;
- exact tagline `Power in, tokens out.`;
- responsive mode/compact/mini/tiny composition;
- arrow-key GPU selection;
- breathing spring and braille interpolation.

Do not port its monolithic `App` architecture or synthetic observation behavior. Production follows the module ownership and canonical observation model in the architecture specification.

## Load-bearing decisions

### Product boundaries

- Strictly local and read-only.
- No tuning, reset, fan, power-cap, partition mutation, daemon, API, IPC, remote hosts, network access, or durable raw time-series storage.
- Busy activity and high memory occupancy are normal; neither is a health warning by itself.
- Health is one highest-priority factual source-backed sentence, never a score or inferred workload phase.
- Missing telemetry is never numeric zero.
- The full selected-GPU instrument cluster is called **mode**.

### Telemetry authority

- Kernel sysfs, hwmon, and versioned `gpu_metrics` are mandatory and authoritative.
- AMD SMI is optional runtime-loaded enrichment; it never overwrites a fresh kernel observation.
- Never sample through the `amd-smi` CLI or debugfs.
- A supported host needs only Linux plus one AMD PCI/DRM device bound to `amdgpu`.

### Observation model

Every metric is a value with source time or exactly one explicit state:

- `unsupported_hardware`
- `unsupported_driver_version`
- `permission_denied`
- `asleep`
- `reported_by_primary_partition`
- `stale`
- `source_error`

A stale observation keeps the last good source time but not its numeric value. Retained, stale, and failed observations never enter history, peaks, thresholds, rates, or summaries.

### Topology

- Physical GPUs own socket-scoped power and temperature.
- XCPs own partition-scoped activity, memory, clocks, and violations.
- Secondary partitions use `reported_by_primary_partition` where appropriate.
- Never synthesize aggregate node/physical utilization or double-count socket observations.
- Display indexes are presentation only, not identity.

### Sampling and delivery

- Fast kernel collection: 250 ms.
- Render-only animation: 125 ms.
- Slow health collection: 1 second.
- Process collection: 2 seconds, only while visible.
- History: preallocated 240 points / 60 seconds.
- Skip missed ticks; never replay or queue them.
- First public snapshot follows the second priming sample and remains within one second.
- Source request/result channels are capacity one.
- Outward delivery separates a lossy latest-snapshot mailbox from a priority notice/fatal mailbox.

### Process overlay

Live HIP evidence settled the initial claim:

- Show PID, permitted name, GPU/XCP association, resident GPU memory, and container identity.
- Do not show per-process utilization or engine-time activity.
- DRM fdinfo associated by `drm-pdev`; KFD exposed queues and memory accounting.
- No `drm-engine-*` field advanced and no KFD utilization share/`cu_occupancy` was exposed on the validated host.
- KFD and fdinfo memory values differed by 15%; do not reconcile or sum them.
- Process scanning averaged about 839 ms, so it must stay isolated from the 250 ms kernel lane.

### Architecture

One Cargo package named `gpuflo`, with library and binary targets; no workspace.

Normative module tree:

```text
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
├── source/{mod.rs,kernel.rs,amdsmi.rs,process.rs}
├── state/{mod.rs,reducer.rs,history.rs,health.rs}
└── ui/{mod.rs,layout.rs,widgets.rs,theme.rs,format.rs}
```

One coordinator thread owns schedules, normalization, reducer application, publication, discovery, and lifecycle. Potentially blocking work runs on bounded source-specific lanes. No Tokio, generic backend/plugin trait, DI framework, thread pool, daemon, database, or general application framework.

Only the canonical model and narrow `Monitor` interface are semver-supported. Source adapters, normalization events, reducer internals, histories, render models, persistence records, terminal/UI internals, and unsafe handles remain private.

All AMD SMI `unsafe`, symbol loading, ABI checks, handles, C conversion, and shutdown stay inside `source::amdsmi`; no pointer or borrowed C memory escapes.

### Configuration and UI

Sparse user-owned TOML:

```text
$XDG_CONFIG_HOME/gpuflo/config.toml
```

fallback `~/.config/gpuflo/config.toml`. Normal file is blank; defaults remain in code. Initial TOML keys are only:

```toml
theme = "buffalo"       # buffalo | nord | monochrome
mode = "auto"           # auto | mode | compact | mini | tiny
no_color = false
```

CLI visual overrides: `--theme`, `--mode`, `--no-color`. Any non-empty `NO_COLOR` disables color. Session `t`/`m` changes never rewrite TOML.

Output modes remain CLI-only: `--once`, `--json`, `--json-stream`, `--tiny`, and `--gpu`. Visual configuration never changes machine or human non-interactive semantics.

### Output and exits

- `--once`: one human line per physical GPU.
- `--json`: one pretty all-GPU schema-version-1 snapshot.
- `--json-stream`: compact NDJSON at the production cadence.
- `--tiny`: one selected-GPU human status line.
- Text output is ANSI-free and uses canonical unavailable phrases.
- JSON uses tagged observations, nested physical GPU/XCP scope, source timestamps, and units in field names.
- Exit `0`: success, partial telemetry, or broken pipe.
- Exit `1`: fatal runtime/no startup GPU/no snapshot/non-pipe output failure.
- Exit `2`: CLI or startup configuration error.
- Exit `130`: SIGINT.

### Terminal safety

Interactive preflight occurs before terminal takeover. Restoration order is cursor, raw mode, alternate screen, then fatal diagnostic, monitor shutdown, persistence flush, and bounded joins. Cover normal quit, catchable signals, fatal errors, and unwind. SIGKILL/abort remain inherently excluded.

### Dependencies

Approved focused set:

- `ratatui`, `crossterm`
- `serde`, `serde_json`, `toml`
- `lexopt`
- `time`
- `thiserror`
- `crossbeam-channel`
- `libloading`
- `signal-hook`

Pin versions/features during implementation using current compiler/MSRV evidence. Add no dependency without demonstrating that the standard library and approved set cannot carry the behavior.

## Validation bar

Stay proportional to a small tool. Required layers are the minimal risk-based set from the validation specification:

- fixtures for implemented source layouts and meaningful failure boundaries;
- focused normalization/reducer tests for unavailable states, freshness exclusion, topology scope, precedence, and health priority;
- one public `Monitor` integration journey;
- semantic text/JSON/NDJSON contract tests;
- representative Ratatui frames and one bounded responsive-size sweep;
- three PTY restoration journeys: normal, signal, fatal;
- no coverage target, exhaustive combinatorial matrix, custom test framework, or incidental formatting goldens.

On qualified hardware, first useful output must be within one second, fast kernel p95 below 125 ms with no operation reaching 250 ms, and repeatable workload regression no worse than 2%.

## Packaging target

Initial channels:

1. GitHub Release archive for qualified `x86_64-unknown-linux-gnu`.
2. The same package on crates.io for `cargo install gpuflo` and library reuse.

One SemVer version covers source, package, binary, crate, tag, archive, and validation evidence. No initial Cargo feature matrix, distro packages, musl claim, AArch64 binary without live qualification, installer, man pages, completions, self-update, or bundled ROCm.

Ship `LICENSE`, generated `THIRD_PARTY_NOTICES.txt`, `README.md`, and `SHA256SUMS`. Preserve per-file provenance for copied or closely translated flow/rocm-cli code.

## Implementation sequence guidance

The next agent owns decomposition, but the dependency order is constrained:

1. Establish the Cargo package, canonical model, CLI/config parsing, and deterministic time/source seams.
2. Implement pure source parsing, normalization, reducer/history/health, and focused fixtures before concurrency.
3. Implement coordinator/lane lifecycle and public `Monitor` interface against fake lanes.
4. Implement kernel discovery and fast/slow collection; then optional AMD SMI and process lanes.
5. Implement output surfaces from canonical snapshots.
6. Implement persistence and terminal safety.
7. Port the approved responsive visual behavior into the production UI without importing prototype state architecture.
8. Run the required contract/PTY/UI verification and package/release checks.

Prefer vertical, runnable increments, but do not narrow the final contract. Remove temporary scaffolding as each real path lands.

## Agent execution rules

- Start by reading every canonical input listed above.
- Create one production implementation plan under `docs/superpowers/plans/` before source edits.
- Execute the plan to completion; do not stop after writing it.
- Use an isolated branch/worktree and commit coherent increments.
- Reuse existing decisions exactly; ask only if two canonical specifications materially contradict one another and tools/repository evidence cannot resolve it.
- Treat unexpected repository changes as user work.
- Run the actual binary for CLI/TUI verification; unit tests alone are insufficient.
- Use the approved prototype only as a visual reference.
- Keep the code boring, bounded, allocation-conscious, and small.
- Do not reopen the closed Wayfinder map or create speculative extension points.
- Finish with a review pass, exact verification evidence, and a ready-to-merge PR or explicitly reported environmental blocker.

## Definition of done

The handoff is complete only when:

- the production Cargo package and normative module tree exist;
- the library and binary compile on the selected MSRV/current stable target;
- kernel-backed operation does not require ROCm/AMD SMI/root/network;
- all approved TUI and non-interactive surfaces work from one canonical model;
- observation, topology, freshness, failure, terminal, process, configuration, output, and persistence contracts are implemented;
- the minimal validation gate passes;
- the binary is smoke-run through the changed paths;
- required license/provenance/package artifacts exist;
- no placeholder, stub, fake fallback, or deferred production path remains; and
- the implementation branch is pushed with a PR or equivalent integration artifact.

## Ready-to-paste prompt

```text
Continue gpuflo from the completed Wayfinder effort and build the production implementation end to end.

Repository: https://github.com/mikeroysoft/gpuflo.git
Baseline: main at or after 0e34de3
Authoritative handoff: docs/superpowers/handoffs/2026-08-21-gpuflo-production-implementation.md

First read the entire handoff, CONTEXT.md, every canonical specification it lists, the live process-attribution result, and the approved Ratatui prototype at commit 699a425. The closed Wayfinder map is context only; do not reopen settled product decisions.

Then create an isolated feature branch/worktree, write a production implementation plan under docs/superpowers/plans/, and execute that plan fully. Do not stop after planning. Build the single-package Rust library+binary architecture exactly as specified, implement kernel-first telemetry with optional runtime AMD SMI, the canonical observation/reducer model, bounded monitor lanes, output modes, sparse configuration, persistence, terminal restoration, process overlay, and the approved responsive Ratatui UI. Keep gpuflo strictly local and read-only. Preserve explicit unavailable states and physical-GPU/XCP scope. Use mode, never legacy terminology, for the full instrument cluster.

Follow the minimal validation contract rather than inventing a huge test matrix. Run the actual binary and PTY/TUI paths, perform required code review, fix findings, and continue until every definition-of-done item in the handoff is satisfied. Commit coherent increments, push the branch, and produce a ready-to-merge PR with exact build/test/smoke evidence. Ask me only for a genuine unresolved contradiction or an external credential/hardware prerequisite; otherwise make the conservative boring decision and keep executing.
```

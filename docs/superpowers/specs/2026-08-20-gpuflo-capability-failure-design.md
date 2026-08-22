# Gpuflo Capability, Failure, and Permission Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gpuflo must remain useful when hardware, drivers, telemetry sources, permissions, and devices are only partially available. Failure is scoped to the smallest truthful observation or device. Missing telemetry is never numeric zero, optional enrichment never controls core availability, and terminal ownership is always restored before a fatal diagnostic.

This contract defines product behavior. Exact worker structure, deadlines, cooldown constants, and platform APIs remain for the architecture and validation decisions, subject to the invariants below.

## Supported host and startup gate

A supported host is:

- Linux; and
- a host with at least one AMD PCI/DRM device bound to the `amdgpu` driver.

A base physical-GPU identity is enough to start. Gpuflo does not require ROCm userspace, the `amd-smi` executable, the AMD SMI library, `/dev/kfd`, membership in `render` or `video`, or root privileges.

The host gate runs before raw mode, cursor hiding, or alternate-screen entry. If the host is not Linux or no `amdgpu` GPU is discoverable at startup, gpuflo prints one actionable stderr line and exits `1` without taking over the terminal.

If at least one physical GPU is identified, gpuflo starts even when every contracted metric is represented by an observation state. That is a valid partial snapshot, not startup failure.

## Device and capability discovery

Gpuflo discovers devices from AMD PCI/DRM entries bound to `amdgpu`, groups processor handles into physical GPUs and XCPs, and then probes each metric source. Hardware model-name lists never define capability.

The kernel path is authoritative:

- sysfs, hwmon, and versioned `gpu_metrics` provide core observations;
- fresh kernel observations are never overwritten by optional enrichment;
- AMD SMI may fill a genuinely unsupported kernel field or add richer health/detail observations; and
- the `amd-smi` executable is never a live source.

A missing or incompatible AMD SMI library disables enrichment only. A missing or unreadable `/dev/kfd` disables only KFD-dependent process attribution and event capabilities. When an affected surface is opened, an existing but unreadable source maps to `permission_denied`; an absent source maps to `source_error` unless explicit version evidence supports `unsupported_driver_version`. None of these conditions degrades a complete kernel-backed mode.

Structural capabilities are cached until the affected device is re-enumerated:

- `unsupported_hardware`;
- an explicit `unsupported_driver_version`; and
- `reported_by_primary_partition`.

Runtime states are retried on their bounded source schedules:

- `asleep`;
- `permission_denied`;
- `source_error`; and
- sources that have become `stale`.

A new or returned GPU is probed from scratch.

## Observation states

Every observation is either a value with its source time or one explicit observation state:

| State | Meaning |
| --- | --- |
| `unsupported_hardware` | The source explicitly reports unsupported, emits a documented sentinel, or the metric is structurally inapplicable to this hardware/topology. |
| `unsupported_driver_version` | An explicit driver, ABI, or `gpu_metrics` layout mismatch prevents this build from interpreting the metric. |
| `permission_denied` | The active source exists but access is denied. |
| `asleep` | Runtime-suspended amdgpu denied a read; polling must not wake the GPU. |
| `reported_by_primary_partition` | The observation is owned and reported at the primary XCP/physical-GPU scope. |
| `stale` | A previously good observation exceeded its approved freshness threshold. |
| `source_error` | A recognized or required source is unavailable, timed out, returned malformed/invalid data, or failed unexpectedly when no fresh value could be retained. |

The split is evidence-based:

- a bare absent node does not by itself prove driver incompatibility;
- `unsupported_driver_version` requires version/layout/ABI evidence;
- source `NOT_SUPPORTED`, documented sentinels, and structural inapplicability map to `unsupported_hardware`;
- amdgpu runtime-suspend `EPERM` maps to `asleep`; and
- access failure on an active source maps to `permission_denied`.

Unknown future observation-state strings remain additive under machine schema version 1.

## Permission behavior

Permission failure is feature-local. It never hides a GPU, suppresses healthy observations, or recommends running gpuflo as root.

- Mode observations remain kernel-backed and usable without supplemental groups.
- Missing process identity retains the PID/row and reports `permission_denied` at the missing field.
- Missing process attribution reports the exact state instead of an empty table.
- Detail/help may name the blocked source and suggest `render`/`video` membership only when that source actually requires it.
- Optional detail/process permission failures do not alter the mode's health sentence.

There is no persistent global “limited privilege” banner. The affected surface carries its own precise state.

## Freshness, retries, and timeouts

A failed read after a good observation does not immediately erase that value:

1. retain the last value and its original `observed_at` while it remains fresh;
2. do not append the retained value to history or treat it as a new peak;
3. retry only on later scheduled reads; and
4. at `max(1 second, 3 × source cadence)`, replace the value with `stale`.

Recovery appends the next fresh observation normally. No failed or stale observation contributes to history, peaks, thresholds, or rate derivation.

Every source operation is bounded:

- at most one operation is in flight per source/device scope;
- its deadline is shorter than that source's cadence;
- timeout never queues an overlapping retry;
- a late result is discarded; and
- timeout follows the same retained-value, stale, or `source_error` transition as any other read failure.

Exact deadlines and cooldown durations require measured architecture/validation evidence.

Optional AMD SMI initialization or sampling failure opens a bounded circuit breaker. During cooldown, kernel telemetry continues, enrichment-only observations retain/fall through according to their own states, and the optional source is probed again later. Optional enrichment can never stall the 250 ms kernel path.

## Malformed and changing source data

Gpuflo recognizes documented sentinels before numeric validation. It never clamps broken telemetry into a plausible value.

- An unknown `gpu_metrics` layout or incompatible AMD SMI ABI maps only affected observations to `unsupported_driver_version`.
- A recognized layout that is truncated, malformed, wrong-width, non-finite, impossibly negative, or invalid for that metric maps only affected observations to `source_error`.
- Legitimate source behavior remains valid even when it crosses a configured limit; for example, socket power may briefly exceed its cap.
- Unaffected fields and other devices continue.

Diagnostics are emitted on state transitions, not every failed tick. Repeated identical failures do not spam logs or the interface.

## Device disappearance and rediscovery

A read failure alone does not remove a GPU. It first follows normal freshness behavior and triggers topology verification.

When a topology rescan confirms that a physical GPU disappeared:

- remove it from subsequent snapshots;
- show one factual `GPU disconnected` notice in the TUI;
- if it was selected, choose the next physical GPU in display order, or the previous GPU when it was last; and
- remember the stable physical-GPU identity so a returning device can regain selection during the same session.

If every GPU disappears after startup:

- the TUI remains alive with `no AMD GPU currently detected` and continues discovery;
- JSON stream remains valid, emits snapshots with `gpus: []`, and continues discovery; and
- returning GPUs are probed from scratch and reappear without restarting.

This runtime empty state does not change the startup rule: one-shot commands and a newly launched TUI/stream still exit `1` when no GPU is discoverable initially.

## Partial multi-GPU failure

Failure is isolated at metric, source, and physical-GPU scope:

- one bad metric affects only that observation;
- one bad source affects only observations that depend on it;
- one failed physical GPU never suppresses healthy GPUs; and
- exports remain successful while at least the started session can produce its contracted snapshot stream.

The affected physical GPU's health sentence reports telemetry trouble only when a contracted mode observation or source-backed health signal is unavailable or stale. Optional fan, detail, process, `/dev/kfd`, or enrichment failures stay local to those surfaces. Gpuflo never creates a node-wide health score.

## Partition configuration changes

An accelerator/memory partition-mode change invalidates XCP handles and identity assumptions. Gpuflo does not join old and new XCPs by display index and does not attempt a live subtree migration in the initial design.

After confirming a partition configuration change, gpuflo:

1. stops output;
2. restores terminal state;
3. prints `GPU partition configuration changed; restart gpuflo` to stderr; and
4. exits `1`.

A fresh process re-enumerates the physical GPU and builds the new XCP topology from scratch.

## Diagnostics and health presentation

Recoverable failures appear through observation states, the existing health priority, and a detail/source-status surface.

- The mode reports telemetry trouble only for mode observations and source-backed health signals.
- Optional detail/process limitations do not make a functioning GPU look unhealthy.
- While the TUI owns the terminal, asynchronous source failures never write directly to stderr.
- Fatal diagnostics print to stderr only after terminal restoration.
- Logs record state transitions, not repeated samples of the same failure.

## Terminal restoration

Terminal ownership is acquired only after the startup gate passes. Restoration ownership is installed as each terminal mode is enabled so partial setup can unwind completed steps.

Best-effort restoration covers:

- normal quit;
- SIGINT, SIGTERM, and SIGHUP;
- input, draw, and collector-fatal errors;
- confirmed partition configuration change; and
- panic/unwind.

Restoration shows the cursor, disables raw mode, and leaves the alternate screen before fatal diagnostics are printed. Cleanup attempts every remaining step even if one restoration action fails; restoration failure contributes to exit `1` but must not cause a second panic. SIGKILL and process abort are inherently untrappable and are the only excluded cases.

Non-interactive text/JSON modes never acquire alternate-screen or raw-mode state.

## Acceptance criteria

The contract is satisfied when:

- a Linux host with one `amdgpu` GPU starts without ROCm userspace, AMD SMI, `/dev/kfd`, or root;
- no supported GPU fails before terminal takeover with an actionable exit `1` diagnostic;
- every unavailable observation maps to exactly one documented observation state;
- a sleeping GPU is not mislabeled as permission failure and is not awakened by polling;
- one transient read miss does not flicker a fresh value into an error;
- stale, invalid, and failed observations never enter history, peaks, thresholds, or rates;
- unknown source layouts and malformed recognized layouts remain distinct;
- optional backend failure cannot erase kernel telemetry or block the fast path;
- one failed metric/GPU does not suppress independent healthy data;
- confirmed hot-unplug, zero-device runtime, and rediscovery preserve a coherent current topology;
- partition configuration change restores the terminal and exits for restart; and
- every catchable exit path restores cursor, raw mode, and alternate screen.

## Evidence

- [Inventory AMD telemetry sources and support](https://github.com/mikeroysoft/gpuflo/issues/8)
- [AMD telemetry source research](https://github.com/mikeroysoft/gpuflo/blob/research/amd-telemetry-sources/research/amd-telemetry-sources.md)
- [Define the metric and health contract](https://github.com/mikeroysoft/gpuflo/issues/5)
- [Set sampling, smoothing, and history semantics](https://github.com/mikeroysoft/gpuflo/issues/3)
- [Set the process overlay contract](https://github.com/mikeroysoft/gpuflo/issues/4)
- [Define the machine-readable output contract](https://github.com/mikeroysoft/gpuflo/issues/11)
- [Prototype the responsive dashboard language](https://github.com/mikeroysoft/gpuflo/issues/7)

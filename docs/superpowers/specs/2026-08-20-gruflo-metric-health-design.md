# Gruflo Metric and Health Design

**Status:** Approved in brainstorming on 2026-08-20

## Purpose

Gruflo is a universal AMD GPU instrument for a user running local LLM inference in another terminal. A glance at the dashboard must answer, within one second:

1. Is the selected GPU working?
2. How much device memory is occupied?
3. Is the GPU limited, throttled, faulted, asleep, or reporting stale data?

Gruflo observes GPU behavior only. It does not connect to inference engines or infer application phases.

## Product principles

- Busy is successful. High activity and well-packed memory are not failures.
- Health statements must name source-backed conditions; no opaque score.
- Missing telemetry is information, not zero.
- One selected physical GPU owns the full mode.
- Secondary diagnosis stays one keypress away.
- The same metric meaning must serve every TUI mode and later machine-readable outputs.

## Mode

### GPU activity

The dominant instrument shows:

- current GFX activity, in percent;
- a breathing 60-second waveform;
- a short rising, steady, or falling trend;
- the session peak in subdued text.

The label is **GPU activity**, not per-process utilization and not application throughput.

### Memory occupancy

Show:

- used and total capacity;
- percentage occupancy;
- the memory-pool name.

Discrete GPUs use VRAM. APUs explicitly identify shared or GTT memory; they must not present it as dedicated VRAM.

High occupancy is neutral. Occupancy alone never creates a warning. Gruflo reports memory pressure only when a telemetry source explicitly reports allocation pressure or failure.

### Supporting instruments

Always visible when supported:

- hotspot temperature against the source-reported slowdown or critical limit;
- socket power against the source-reported power cap;
- GFX clock;
- memory-controller activity as an optional secondary signal.

An unsupported optional instrument remains explicitly unavailable; another metric must not silently replace it.

### Health sentence

One short sentence reports the highest-priority active condition. Normal text is factual, such as `no active limits or faults`; it must not claim the device is comprehensively “healthy.”

Priority order:

1. uncorrectable fault, reset-required state, or severe RAS condition;
2. active thermal, power, current, or other source-reported throttle;
3. a source-reported limit being reached;
4. telemetry unavailable, stale, permission-limited, or asleep;
5. source-reported memory pressure;
6. no active limits or faults.

Examples:

- `thermal throttle · hotspot 94 / 95°C`
- `power limit active · 318 / 320 W`
- `2 uncorrectable ECC errors`
- `GPU asleep`
- `telemetry stale · last sample 4.2s ago`
- `no active limits or faults`

The health sentence may use throttle/violation state, thermal limits, ECC/RAS, bad-page state, link-error deltas, reboot-required state, and AMD SMI events. It must not infer `loading`, `generating`, or another LLM phase.

## Multiple GPUs and partitions

The full mode describes one selected physical GPU. A compact overview strip shows every physical GPU with:

- identity or short model label;
- activity;
- memory occupancy;
- hotspot temperature;
- the most severe active health condition.

Arrow keys change the selected GPU.

XCPs appear beneath their physical socket rather than as independent physical GPUs. Socket-scoped temperature and power are displayed once and never summed across processor handles. A secondary partition that lacks a socket-wide metric reports `reported by primary partition`.

There is no aggregate node-utilization percentage: averaging or summing unlike devices would be ambiguous.

## Capability and freshness states

Every observation is either a value or one of these explicit states:

- `unsupported_hardware`
- `unsupported_driver_version`
- `permission_denied`
- `asleep`
- `reported_by_primary_partition`
- `stale`

A source sentinel, absent sysfs node, permission error, runtime-suspended read, partition ownership rule, and expired observation map to different states. None maps to numeric zero.

The UI may use a compact dash or marker, but detail/help text must expose the exact state.

## Process overlay

The process overlay is secondary to physical telemetry. For each process, show only evidence the host can attribute honestly:

- PID;
- process name when permitted;
- associated GPU or partition;
- attributed GPU memory;
- engine-time activity only if live ROCm validation proves it reliable.

Sort primarily by attributed GPU memory. When names or process data require additional permissions, show the privilege limitation rather than hiding rows.

Do not show per-process GPU-utilization percentages: no selected telemetry layer provides them reliably. Engine-time behavior remains gated by the separate live-hardware decision.

## Detail view

The detail view may show supported secondary telemetry:

- edge, hotspot, VRAM, and HBM temperatures;
- fan speed;
- memory clock and performance level;
- memory-controller and multimedia activity;
- PCIe link state, bandwidth, and error deltas;
- ECC/RAS counters and bad-page state;
- throttle/violation reason breakdown;
- XGMI and socket/XCP topology;
- board identity;
- driver, ROCm, kernel-metric, and telemetry-source versions.

Unsupported metrics remain absent-state observations, not zeroes.

## Explicit non-goals

This metric contract excludes:

- tokens per second, queue depth, KV-cache use, and inference-engine APIs;
- inferred workload phases;
- per-process utilization percentages;
- aggregate node utilization;
- user-defined warning thresholds in the initial design;
- GPU tuning, reset, fan, power-cap, or partition controls;
- remote and fleet telemetry;
- an opaque health score.

## Acceptance criteria

The contract is satisfied when:

- a user can identify activity, memory occupancy, and the highest-priority active condition within one second;
- a saturated but unthrottled LLM workload does not appear unhealthy;
- every unavailable observation retains its reason;
- multi-GPU and XCP layouts never double-count socket-scoped metrics;
- the process overlay never claims unsupported utilization precision;
- all responsive TUI and machine-readable modes use the same metric meanings.

The companion [machine-readable output design](./2026-08-20-gruflo-machine-readable-output-design.md) fixes the text, JSON, stream, tiny, topology, state, unit, timestamp, versioning, and exit contracts for those meanings.

## Evidence

- [Inventory AMD telemetry sources and support](https://github.com/michaelroy-amd/gruflo/issues/8)
- [AMD telemetry source research](https://github.com/michaelroy-amd/gruflo/blob/research/amd-telemetry-sources/research/amd-telemetry-sources.md)
- [Identify the reusable code boundary](https://github.com/michaelroy-amd/gruflo/issues/2)
- [Reuse boundary research](https://github.com/michaelroy-amd/gruflo/blob/research/reuse-boundary/research/reuse-boundary.md)

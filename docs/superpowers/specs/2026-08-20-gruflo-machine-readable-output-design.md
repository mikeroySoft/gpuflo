# Gruflo Machine-Readable Output Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gruflo's non-interactive surfaces expose the same observations, scope, health meaning, and unavailable states as the TUI. They do not create a second metric model, synthesize physical-GPU utilization, or turn missing telemetry into zero.

The initial contract covers the mode instruments only:

- physical-GPU and XCP identity and topology;
- GPU activity;
- memory pool, used capacity, total capacity, and occupancy;
- hotspot temperature and its source-reported limit;
- socket power and its source-reported cap;
- GFX clock;
- optional memory-controller activity; and
- the highest-priority source-backed health condition.

Detail-view and process-overlay telemetry are not part of schema version 1.

## Output surfaces

The default command remains the interactive TUI. Its non-interactive surfaces are:

| Surface | Behavior |
| --- | --- |
| `--once` | Print one human-readable line per physical GPU, then exit. |
| `--json` | Print one pretty, newline-terminated JSON snapshot containing every physical GPU, then exit. |
| `--json-stream` | Print compact NDJSON snapshots continuously. |
| `--tiny` | Print one human-readable status line for the selected physical GPU, then exit. |

A global `--gpu <index|id|bdf>` selector chooses the physical GPU for `--tiny` and may set the TUI's initial selection. Without it, display index `0` is selected. The selector does not filter `--once`, `--json`, or `--json-stream`; those surfaces always report every physical GPU.

All non-interactive modes prime counter-derived observations by waiting for a second fast sample. With the approved 250 ms fast cadence, first output remains inside the one-second product budget. Exported values are raw observations; animation, spring values, and graph smoothing never enter text or JSON.

## Human-readable text

`--once` emits one line per physical GPU in this semantic order:

1. GPU index, model, and PCI BDF;
2. primary-XCP label when the physical GPU has multiple XCPs;
3. GPU activity;
4. memory used/total, pool, and occupancy;
5. hotspot temperature/limit;
6. socket power/cap;
7. GFX clock; and
8. the health sentence.

For example:

```text
gpu 0 MI300X [0000:41:00.0] | xcp 0 | activity 97% | memory 182.4/192.0 GiB VRAM (95%) | hotspot 74/95°C | power 318/320 W | clock 1700 MHz | no active limits or faults
```

Spacing and separators may improve without changing this semantic order. Text output is human-facing, not a parser API.

`--tiny` keeps only identity, activity, memory, hotspot temperature, and the health sentence, in that order. It is a one-shot status-line surface; the responsive TUI's tiny fallback remains interactive.

Neither text mode emits ANSI escapes, even when stdout is a terminal. An unavailable observation renders its exact canonical phrase, never numeric zero, a bare dash, or `N/A`:

- `unsupported hardware`
- `unsupported driver version`
- `permission denied`
- `asleep`
- `reported by primary partition`
- `stale <age>`

Memory uses IEC human formatting while retaining its actual pool label. Percent, Celsius, watts, and megahertz retain the mode's existing display semantics.

## JSON envelope

All JSON names are lower snake case. One-shot JSON has this envelope:

```json
{
  "schema_version": 1,
  "gruflo_version": "0.1.0",
  "sampled_at": "2026-08-20T23:45:12.250Z",
  "gpus": []
}
```

Continuous records add `sequence` beside `sampled_at`. No other surface changes the payload shape.

- `schema_version` is the output schema's integer major version.
- `gruflo_version` is the producing binary's version and does not govern payload compatibility.
- `sampled_at` is when gruflo assembled the exportable snapshot.
- `sequence` counts produced exportable fast snapshots, starts at `1`, and resets on process start.
- `gpus` contains every physical GPU in display order.

## Observation shape

Every schema-defined metric is always present as exactly one of two tagged forms.

A current value contains the number and the time that source observed it:

```json
{
  "value": 97.0,
  "observed_at": "2026-08-20T23:45:12.247381Z"
}
```

An unavailable value contains its state:

```json
{
  "state": "unsupported_hardware"
}
```

The canonical version-1 states are:

- `unsupported_hardware`
- `unsupported_driver_version`
- `permission_denied`
- `asleep`
- `reported_by_primary_partition`
- `stale`

A stale observation contains the time of the last good observation but no numeric value:

```json
{
  "state": "stale",
  "observed_at": "2026-08-20T23:45:08.000Z"
}
```

Consumers derive stale age from `sampled_at - observed_at`. Gruflo never emits a stale `value`, JSON `null`, `NaN`, or infinity. Source sentinels and parse failures never leak as numbers.

UTC timestamps use RFC 3339 with `Z`. They carry milliseconds at minimum and preserve finer source precision when the source provides it; they do not add false precision. A snapshot can therefore contain observations with different timestamps from the approved fast and slow cadences.

## Units

JSON units are part of metric field names rather than repeated strings:

- percentages: `*_percent`;
- memory capacity: `*_bytes`, encoded as integer bytes;
- temperature: `*_celsius`;
- power: `*_watts`; and
- clock frequency: `*_mhz`.

Memory also carries an unknown-safe `pool` string. Version 1 defines `vram`, `shared`, and `gtt`. `used_bytes`, `total_bytes`, and `occupancy_percent` describe the same pool and remain separate observations so every surface uses one occupancy definition.

## Topology and scope

The JSON hierarchy follows hardware scope instead of flattening processor handles:

- each `gpus[]` entry is one physical GPU;
- socket-scoped hotspot temperature and socket power live on the physical GPU;
- each physical GPU always owns `partitions[]`, including one partition in SPX or otherwise unpartitioned operation;
- XCP-scoped activity, memory, GFX clock, and memory-controller activity live on partition entries; and
- no physical-GPU activity or memory aggregate is synthesized.

A physical GPU has:

- an opaque `id` stable enough for gruflo joins and persistence;
- an ephemeral display `index`;
- PCI `bdf`;
- model `name`;
- optional source UUID or serial identity;
- a `health` object;
- socket `temperature` and `power` objects; and
- nested `partitions`.

A partition has its own opaque `id`, display `index`, and `is_primary` marker. Backend processor handles are never exposed as identities.

Human one-line summaries use the primary partition's activity, memory, and clock. When more than one XCP exists, the line labels that XCP explicitly. Socket metrics remain owned by the physical GPU and are never copied onto or summed across partitions. An XCP-scoped metric structurally reported only by the primary partition uses `reported_by_primary_partition` on secondary XCPs rather than zero.

## Health

Each physical GPU carries the same highest-priority factual health sentence as the TUI:

```json
{
  "category": "none",
  "message": "no active limits or faults",
  "observed_at": "2026-08-20T23:45:12.000Z"
}
```

Version 1 defines these unknown-safe categories, in the metric contract's priority order:

1. `fault`
2. `throttle`
3. `limit`
4. `telemetry`
5. `memory_pressure`
6. `none`

The category supports automation; the message remains source-backed and human-readable. There is no health score and no inferred workload phase.

## Complete version-1 example

```json
{
  "schema_version": 1,
  "gruflo_version": "0.1.0",
  "sampled_at": "2026-08-20T23:45:12.250Z",
  "gpus": [
    {
      "id": "gpu-73fbc1",
      "index": 0,
      "bdf": "0000:41:00.0",
      "name": "AMD Instinct MI300X",
      "health": {
        "category": "none",
        "message": "no active limits or faults",
        "observed_at": "2026-08-20T23:45:12.000Z"
      },
      "temperature": {
        "hotspot_celsius": {
          "value": 74.0,
          "observed_at": "2026-08-20T23:45:12.247381Z"
        },
        "limit_celsius": {
          "value": 95.0,
          "observed_at": "2026-08-20T23:45:12.000Z"
        }
      },
      "power": {
        "socket_watts": {
          "value": 318.0,
          "observed_at": "2026-08-20T23:45:12.247381Z"
        },
        "cap_watts": {
          "value": 320.0,
          "observed_at": "2026-08-20T23:45:12.000Z"
        }
      },
      "partitions": [
        {
          "id": "gpu-73fbc1-xcp-0",
          "index": 0,
          "is_primary": true,
          "activity_percent": {
            "value": 97.0,
            "observed_at": "2026-08-20T23:45:12.247381Z"
          },
          "memory": {
            "pool": "vram",
            "used_bytes": {
              "value": 195850508697,
              "observed_at": "2026-08-20T23:45:12.247381Z"
            },
            "total_bytes": {
              "value": 206158430208,
              "observed_at": "2026-08-20T23:45:12.247381Z"
            },
            "occupancy_percent": {
              "value": 95.0,
              "observed_at": "2026-08-20T23:45:12.247381Z"
            }
          },
          "gfx_clock_mhz": {
            "value": 1700.0,
            "observed_at": "2026-08-20T23:45:12.000Z"
          },
          "memory_controller_activity_percent": {
            "state": "unsupported_hardware"
          }
        }
      ]
    }
  ]
}
```

## Continuous JSON

`--json-stream` is newline-delimited JSON:

- one compact, complete snapshot object per line;
- no surrounding array and no pretty printing;
- each line flushed after writing;
- nominal production at the approved 250 ms fast cadence;
- slow observations repeated with their original `observed_at` values;
- missed collection ticks skipped, never queued; and
- superseded snapshots allowed to drop rather than backpressure collection.

`sequence` increments for every produced exportable snapshot, including one superseded before it reaches stdout. A gap therefore tells consumers that output was dropped. Timestamps remain the source of elapsed-time truth; sequence is only run-local ordering.

A fatal stream failure does not inject an error object into stdout. Successfully written output remains a valid NDJSON prefix.

## Schema compatibility

`schema_version` is an integer major version.

Compatible version-1 evolution may:

- add optional fields;
- add new capability-state, health-category, memory-pool, or topology strings; and
- populate previously absent optional identity fields.

Consumers must ignore unknown fields and handle unknown strings generically without treating them as numeric zero or normal health.

A removal, rename, type change, scope change, unit change, or meaning change requires a new schema major version. `gruflo_version` changes independently.

## Errors and exit behavior

Unavailable or stale observations are data, not command failures. A snapshot containing partial telemetry exits successfully as long as gruflo discovered at least one physical GPU and produced the contracted output.

Non-interactive modes use these exits:

| Exit | Meaning |
| --- | --- |
| `0` | Contracted output completed, including partial telemetry and a downstream broken pipe. |
| `1` | No physical GPU was discoverable, no snapshot could be produced, or a fatal collection, serialization, or non-pipe output failure occurred. |
| `2` | Command-line usage error. |
| `130` | Interrupted by SIGINT. |

Fatal diagnostics go to stderr. JSON stdout contains snapshots only. A broken pipe is silent and exits `0`, so `gruflo --json-stream | head` behaves as a successful Unix pipeline.

## Explicit non-goals

Version 1 does not export:

- detail-view telemetry;
- process-overlay rows;
- per-process utilization;
- aggregate node or physical-GPU utilization;
- graph history, spring values, smoothed values, trends, or session peaks;
- inferred workload phases;
- a health score;
- remote-host or daemon protocol fields; or
- structured error events mixed into snapshot stdout.

## Acceptance criteria

The contract is settled when:

- text, JSON, stream, tiny, and TUI surfaces use one metric meaning;
- one-shot output appears inside one second after priming;
- all physical GPUs and nested XCPs retain their true scopes;
- socket metrics cannot be double-counted from the JSON shape;
- every required metric is a value or an explicit state;
- stale values cannot be consumed as current numbers;
- timestamps expose mixed source cadences without inventing precision;
- continuous output remains parseable through interruption or failure;
- unknown additive fields and enum strings do not break version-1 consumers; and
- all published specifications call the full instrument cluster the `mode`.

## Evidence

- [Define the metric and health contract](https://github.com/michaelroy-amd/gruflo/issues/5)
- [Set sampling, smoothing, and history semantics](https://github.com/michaelroy-amd/gruflo/issues/3)
- [Prototype the responsive dashboard language](https://github.com/michaelroy-amd/gruflo/issues/7)
- [Set the process overlay contract](https://github.com/michaelroy-amd/gruflo/issues/4)
- [Inventory AMD telemetry sources and support](https://github.com/michaelroy-amd/gruflo/issues/8)
- [AMD telemetry source research](https://github.com/michaelroy-amd/gruflo/blob/research/amd-telemetry-sources/research/amd-telemetry-sources.md)
- [Identify the reusable code boundary](https://github.com/michaelroy-amd/gruflo/issues/2)
- [Reuse boundary research](https://github.com/michaelroy-amd/gruflo/blob/research/reuse-boundary/research/reuse-boundary.md)

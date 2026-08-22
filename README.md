# gpuflo

*Power in, tokens out.*

gpuflo is a small, strictly local, read-only terminal instrument for Linux
hosts with AMD GPUs on the `amdgpu` driver. Within one second of starting it
answers three questions about the selected physical GPU:

1. What is the GPU doing? (GFX activity)
2. How much applicable GPU memory is occupied?
3. Is a source-backed fault, throttle, limit, pressure, sleep, permission, or
   telemetry condition active?

Busy is successful: high activity and well-packed memory are normal and are
never presented as warnings. Health is one factual, source-backed sentence —
never a score. Missing telemetry is shown as its exact reason, never as zero.

## Requirements

- Linux
- at least one AMD PCI/DRM device bound to the `amdgpu` driver

That is all. gpuflo reads kernel sysfs, hwmon, and the versioned
`gpu_metrics` interface directly. It does **not** require ROCm userspace,
the AMD SMI library, `/dev/kfd`, membership in `render`/`video`, root, or
network access, and it never writes to GPU or driver state.

## Install

From a GitHub release archive:

```sh
tar -xzf gpuflo-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
./gpuflo
```

Verify the download against the published `SHA256SUMS`. From crates.io:

```sh
cargo install gpuflo
```

To uninstall, delete the binary (or `cargo uninstall gpuflo`) and remove the
optional state file `~/.local/state/gpuflo/daily.json` and config file
`~/.config/gpuflo/config.toml` if you created one. Installation never
configures the host: no groups, udev rules, services, or driver changes.

## Use

`gpuflo` starts the interactive instrument. Keys:

| Key | Action |
| --- | --- |
| `←`/`→` (`h`/`l`) | select physical GPU |
| `t` | cycle theme (session only) |
| `m` | cycle preferred surface (session only) |
| `p` | toggle process overlay |
| `d` | toggle detail view |
| `?` | toggle help |
| `q` / `Esc` | quit |

The full selected-GPU instrument cluster is called **mode**. The layout
adapts to the terminal: mode ≥ 72×34, compact ≥ 62×17, mini ≥ 48×11, and a
one-line tiny surface below that.

Non-interactive output modes (see `gpuflo --help` for the authoritative
reference):

| Flag | Behavior |
| --- | --- |
| `--once` | one human-readable line per physical GPU, then exit |
| `--json` | one pretty schema-version-1 JSON snapshot of every GPU, then exit |
| `--json-stream` | compact NDJSON snapshots continuously at the 250 ms cadence |
| `--tiny` | one status line for the selected GPU, then exit |
| `--gpu <index\|id\|bdf>` | select the GPU for `--tiny` and the initial interactive selection |

Text and JSON output are ANSI-free. Exit codes: `0` success (including
partial telemetry and a downstream broken pipe), `1` fatal runtime failure or
no discoverable GPU, `2` usage or configuration error, `130` interrupted.

## Observations and unavailable states

Every metric is either a value with its source observation time or exactly
one explicit state: `unsupported_hardware`, `unsupported_driver_version`,
`permission_denied`, `asleep`, `reported_by_primary_partition`, `stale`, or
`source_error`. Physical GPUs own socket-scoped power and temperature; XCP
partitions own partition-scoped activity, memory, and clocks. gpuflo never
synthesizes aggregate utilization or double-counts socket observations.

## Configuration

`$XDG_CONFIG_HOME/gpuflo/config.toml` (fallback
`~/.config/gpuflo/config.toml`) is optional and normally absent. The
complete key set:

```toml
theme = "buffalo"       # buffalo | nord | monochrome
mode = "auto"           # auto | mode | compact | mini | tiny
no_color = false
```

CLI flags (`--theme`, `--mode`, `--no-color`) override the file, and any
non-empty `NO_COLOR` environment variable disables color unconditionally.
Session `t`/`m` changes never rewrite the file. Visual configuration never
changes machine or non-interactive output semantics.

gpuflo persists one small daily summary (per-GPU activity/memory peaks and
energy, when the source exposes an energy counter) at
`$XDG_STATE_HOME/gpuflo/daily.json` (fallback
`~/.local/state/gpuflo/daily.json`). Raw samples are never stored.

## Optional sources

- **AMD SMI enrichment.** When `libamd_smi.so` is present, gpuflo loads it
  at runtime to fill fields the kernel interfaces do not expose. A fresh
  kernel observation is never overwritten by AMD SMI, and a missing or
  incompatible library only disables enrichment.
- **Process overlay** (`p`). Shows PID, permitted process name, GPU
  association, DRM fdinfo resident memory, separately labelled KFD memory
  accounting, and container identity. The kernel exposes no per-process GPU
  utilization for HIP workloads, so gpuflo makes no such claim; the two
  memory accountings differ by design and are not reconciled. Reading other
  users' attribution requires elevated permissions; affected fields report
  `permission denied` instead of being hidden.

## Hardware qualification

Release claims distinguish **qualified** (the release candidate passed live
checks on representative hardware), **fixture-validated** (interfaces passed
committed fixtures only), and **unverified** telemetry regimes. See each
release's validation manifest for the current claims. Implemented
`gpu_metrics` layouts: v1.3–v1.8, v2.1–v2.4, and v3.0; the dynamic v1.9
layout is detected and reported as `unsupported_driver_version` while stable
text interfaces keep reporting.

## Build and test

```sh
cargo build --release
cargo test
```

Rust 1.96 or newer. The `gpuflo` library target exposes the canonical model
and the `Monitor` interface for reuse; everything else is private.

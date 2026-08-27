# GPUFlo

[![crates.io](https://img.shields.io/crates/v/gpuflo)](https://crates.io/crates/gpuflo)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

**A fast, truthful terminal dashboard for AMD GPUs on Linux.**

![GPUFlo dashboard monitoring an AMD Radeon AI PRO R9700 under load](https://raw.githubusercontent.com/mikeroySoft/gpuflo/main/assets/gpuflo.png)

GPUFlo answers three questions within one second:

1. What is the selected physical GPU doing?
2. How much of its applicable memory pool is occupied?
3. Is a source-backed fault, throttle, limit, pressure, sleep, permission, or telemetry problem active?

It is strictly local and read-only. GPUFlo reads Linux `amdgpu` kernel interfaces directly, never changes GPU state, never contacts a remote service, and never requires an inference engine integration. Busy activity and full memory are treated as useful work—not as errors.

## Highlights

- **Responsive Ratatui dashboard** with full, compact, mini, and one-line views.
- **Live GPU activity and memory graphs** with 60 seconds of bounded history and session peaks.
- **Physical-GPU and XCP-aware topology** without fabricated node-wide utilization.
- **Socket-scoped temperature and power** kept separate from partition-scoped activity, memory, and clocks.
- **Factual health reporting** for source-backed faults, throttles, limits, memory pressure, stale data, sleep, permission failures, and telemetry failures.
- **Honest unavailable states** instead of zeroes, blank values, or guessed data.
- **Process attribution overlay** for PID, name, GPU/XCP, VRAM, GTT, KFD memory, and container identity.
- **Human, JSON, and NDJSON output** from the same canonical telemetry model.
- **Three themes and complete no-color operation** without making color carry required meaning.
- **100 positive rotating taglines**—one is chosen randomly at launch and remains stable for that session.
- **Optional sleeping ASCII cat** (`--cat`)—naps in the margin once the selected GPU is warm; pure decoration, never touches telemetry.
- **Optional runtime AMD SMI enrichment** without a build-time or startup dependency.
- **Small daily summaries** containing peaks and energy when available; raw samples are never persisted.
- **Reusable Rust library API** exposing the canonical model and bounded `Monitor` interface.
- **Terminal-safe lifecycle** with cursor, raw-mode, and alternate-screen restoration on normal quit, catchable signals, and fatal errors.

## Requirements

The runtime requirements are deliberately small:

- Linux;
- at least one AMD PCI/DRM device bound to the in-kernel `amdgpu` driver.

GPUFlo does **not** require:

- ROCm userspace;
- the `amd-smi` executable;
- the AMD SMI library;
- `/dev/kfd`;
- root;
- membership in the `render` or `video` groups;
- a daemon, database, network connection, or cloud account.

Optional sources add information when available. Their absence never prevents kernel-backed monitoring from starting.

## Install

### From crates.io

```sh
cargo install gpuflo --locked
```

### Prebuilt binary (no Rust toolchain)

```sh
curl -fsSL https://raw.githubusercontent.com/mikeroySoft/gpuflo/main/install.sh | sh
```

The script downloads the latest `x86_64-unknown-linux-gnu` release archive
from GitHub, verifies its SHA-256 checksum, and installs the single binary
into `~/.local/bin` (override with `GPUFLO_INSTALL_DIR`). It never uses root
and never modifies groups, udev rules, services, or GPU settings.

The same archives can be downloaded and verified manually from the
[releases page](https://github.com/mikeroySoft/gpuflo/releases); each release
ships `SHA256SUMS`, a GLIBC baseline report, and its validation manifest.

### Build and install from source

Rust 1.96 or newer is required.

```sh
git clone https://github.com/mikeroySoft/gpuflo.git
cd gpuflo
cargo install --path . --locked
```

The binary is installed as `~/.cargo/bin/gpuflo` by default.

To build without installing:

```sh
cargo build --release --locked
./target/release/gpuflo
```

### Release packages

The release workflow produces a qualified `x86_64-unknown-linux-gnu` archive containing exactly:

```text
gpuflo
LICENSE
THIRD_PARTY_NOTICES.txt
README.md
```

Published archives are accompanied by `SHA256SUMS` and a validation manifest. Source publication supports the same package as a binary and as a Rust library; release notes state which hardware regimes were live-qualified, fixture-validated, or unverified.

### Uninstall

```sh
cargo uninstall gpuflo
```

Optional user-owned files may be removed separately:

```text
~/.config/gpuflo/config.toml
~/.local/state/gpuflo/daily.json
```

Installation and removal never modify groups, udev rules, services, drivers, or GPU settings.

## Interactive dashboard

Start GPUFlo with no output-mode flag:

```sh
gpuflo
```

The dashboard continuously samples telemetry while rendering animation independently. Arrow keys change the selected physical GPU without conflating display position with stable identity.

### Responsive views

| View | Minimum terminal area | Contents |
| --- | ---: | --- |
| `mode` | `72×34` | GPUFlo logo, random session tagline, GPU strip, activity and memory graphs, support telemetry, active health, and key hints |
| `compact` | `62×17` | Header, GPU strip, graphs, support telemetry, and active health without the logo block |
| `mini` | `48×11` | Activity and memory graphs |
| `tiny` | any size | One interactive selected-GPU status line |

`auto` chooses the richest view that fits. A forced preference still falls back safely when the terminal is too small; it never grants permission to clip content.

Launch a particular interactive view:

```sh
gpuflo --mode compact
gpuflo --mode mini
gpuflo --mode tiny
```

`--mode tiny` is the persistent interactive view. The separate `--tiny` flag prints one line and exits.

### Keyboard controls

| Key | Action |
| --- | --- |
| `←` / `→` or `h` / `l` | Select the previous or next physical GPU |
| `t` | Cycle Buffalo, Nord, and monochrome themes for this session |
| `m` | Cycle the preferred responsive view for this session |
| `p` | Toggle the process-attribution overlay |
| `d` | Toggle the selected-GPU detail overlay |
| `?` | Toggle help and current presentation state |
| `q` / `Esc` | Quit and restore the terminal |

Theme and view changes are session-only. GPUFlo never rewrites the user's configuration file.

## What GPUFlo measures

### Physical GPU identity and topology

- stable opaque GPU identity;
- display index;
- PCI BDF;
- model name;
- source UUID and serial when exposed;
- physical GPU to XCP partition hierarchy;
- primary-partition ownership of socket-scoped telemetry.

### Live instruments

- GFX activity percentage;
- applicable memory pool: VRAM, shared memory, or GTT;
- memory used, total, and occupancy percentage;
- hotspot temperature and source-reported limit;
- socket power and source-reported cap;
- GFX clock;
- optional memory-controller activity;
- fixed-capacity activity and occupancy histories;
- session activity peak;
- highest-priority active source-backed health condition.

The detail overlay exposes the selected GPU's identity, topology, health, activity, memory pool and capacities, occupancy, clocks, temperature, power, and exact unavailable reasons.

### Correct physical and partition scope

Physical GPUs own socket-wide temperature, power, identity, and health. XCP partitions own activity, memory, GFX clock, and memory-controller activity. GPUFlo does not average unrelated devices, synthesize a physical-GPU utilization percentage from partitions, or duplicate socket observations across XCPs.

## Process attribution

Press `p` to open the process overlay. It can show:

| Field | Meaning |
| --- | --- |
| PID | Linux process ID |
| name | Process name when permitted |
| GPU/XCP | Attributed physical GPU and partition |
| VRAM | Resident DRM fdinfo VRAM accounting |
| GTT | Resident DRM fdinfo system/GTT accounting |
| KFD | Separately sourced KFD VRAM accounting |
| container | Container identity inferred from process cgroups when available |

Important semantics:

- DRM fdinfo and KFD memory are independent accounting systems; GPUFlo does not add or reconcile them.
- The validated Linux interfaces expose process association and memory, but not a truthful per-process HIP utilization share. GPUFlo therefore does not display one.
- Other users' process details may require permissions. Affected fields remain visible as `permission denied` rather than silently disappearing.
- Process scanning runs on an isolated two-second lane and only while the overlay is open, so it cannot block the 250 ms kernel lane.

## Health and unavailable data

GPUFlo treats high activity and high memory occupancy as neutral. Health changes only when a telemetry source reports a condition or required telemetry becomes unavailable.

Active conditions are prioritized as:

1. fault or severe RAS condition;
2. thermal, power, current, or other source-reported throttle;
3. source-reported limit;
4. unavailable, stale, permission-limited, asleep, or failed telemetry;
5. source-reported memory pressure.

When no condition is active, the main health row remains empty rather than printing a generic reassurance.

Every metric is either a value with its source observation time or exactly one explicit state:

| State | Meaning |
| --- | --- |
| `unsupported_hardware` | Structurally inapplicable or explicitly unsupported by the source |
| `unsupported_driver_version` | A known ABI, driver, or `gpu_metrics` layout cannot be interpreted safely |
| `permission_denied` | The active source exists but cannot be read by this user |
| `asleep` | Runtime-suspended `amdgpu` denied the read; GPUFlo does not wake it |
| `reported_by_primary_partition` | The value belongs to the primary XCP or physical-GPU scope |
| `stale` | The last good observation exceeded its freshness deadline |
| `source_error` | A recognized source failed, timed out, or returned malformed data |

Missing, stale, malformed, unsupported, and denied telemetry never becomes numeric zero. A transient failed read retains the last value only while it remains fresh; retained and stale data never enter histories, peaks, rates, or daily summaries.

## Kernel and optional telemetry sources

The kernel path is authoritative:

- DRM and PCI sysfs discovery;
- textual `amdgpu` sysfs nodes;
- hwmon temperature and power nodes;
- versioned binary `gpu_metrics` payloads;
- RAS, throttle, and source-reported health information when implemented by the device;
- DRM fdinfo and optional KFD process evidence.

Implemented `gpu_metrics` families are v1.3–v1.8, v2.1–v2.4, and v3.0. Dynamic v1.9 is detected and represented as `unsupported_driver_version` rather than decoded with a guessed structure; independent stable text nodes continue reporting.

When `libamd_smi.so` is available, GPUFlo loads it at runtime and uses it only to enrich fields not supplied by a fresh kernel observation. A missing library, incompatible ABI, unavailable symbol, or runtime failure disables only enrichment and is retried behind a cooldown.

## Command-line output

Run `gpuflo --help` for the authoritative option reference.

| Flag | Behavior |
| --- | --- |
| `--once` | Print one human-readable line per physical GPU, then exit |
| `--json` | Print one pretty schema-version-1 JSON snapshot containing every physical GPU, then exit |
| `--json-stream` | Continuously print compact NDJSON snapshots at the production cadence |
| `--tiny` | Print one selected-GPU status line, then exit |
| `--gpu <index\|id\|bdf>` | Select the GPU for `--tiny` or the initial interactive selection |
| `--theme <buffalo\|nord\|monochrome>` | Select the interactive theme |
| `--mode <auto\|mode\|compact\|mini\|tiny>` | Select the preferred interactive view |
| `--no-color` | Disable interactive color |
| `--cat` | Show a sleeping ASCII cat once the selected GPU is warm |

Examples:

```sh
# Human summary for every GPU
gpuflo --once

# One machine-readable snapshot
gpuflo --json

# Continuous records; broken pipe is a successful exit
gpuflo --json-stream | head -n 10

# One selected GPU by PCI address
gpuflo --tiny --gpu 0000:03:00.0

# Start the TUI on display index 1
gpuflo --gpu 1
```

`--once`, `--json`, and `--json-stream` always report all physical GPUs; `--gpu` does not filter those all-GPU surfaces.

Human and machine output is always ANSI-free. Exit codes are:

| Code | Meaning |
| ---: | --- |
| `0` | Success, partial telemetry, or downstream broken pipe |
| `1` | Fatal runtime failure, no startup GPU, or output failure |
| `2` | Invalid CLI usage or configuration |
| `130` | Interrupted by SIGINT |

## JSON contract

JSON and NDJSON use the same nested physical-GPU/XCP model as the TUI. The envelope contains:

- integer `schema_version`;
- producing `gpuflo_version`;
- RFC 3339 UTC `sampled_at` time;
- optional run-local `sequence` for streamed records;
- every physical GPU and its nested partitions.

A current observation has a value and source time:

```json
{
  "value": 97.0,
  "observed_at": "2026-08-21T23:45:12.247Z"
}
```

An unavailable observation has its exact state:

```json
{
  "state": "permission_denied"
}
```

A stale observation preserves the last good source time but contains no numeric value:

```json
{
  "state": "stale",
  "observed_at": "2026-08-21T23:45:08.000Z"
}
```

GPUFlo never emits JSON `null`, `NaN`, infinity, a stale numeric value, or an unavailable numeric zero. NDJSON records are independently valid objects. Sequence gaps can reveal that a slow consumer missed superseded snapshots; collection remains bounded instead of accumulating an unbounded queue.

## Configuration

The optional configuration file is:

```text
$XDG_CONFIG_HOME/gpuflo/config.toml
```

with fallback:

```text
~/.config/gpuflo/config.toml
```

The complete schema is intentionally small:

```toml
theme = "buffalo"       # buffalo | nord | monochrome
mode = "auto"           # auto | mode | compact | mini | tiny
no_color = false
cat = false             # sleeping ASCII cat when the selected GPU is warm
```

Precedence is:

```text
built-in defaults → TOML → CLI
```

Any non-empty `NO_COLOR` environment variable disables color unconditionally. Unknown keys, invalid values, wrong types, and malformed TOML fail before terminal takeover with exit code `2`; typos are never silently ignored.

## Daily summaries and privacy

GPUFlo stores one small optional daily summary at:

```text
$XDG_STATE_HOME/gpuflo/daily.json
```

with fallback:

```text
~/.local/state/gpuflo/daily.json
```

It contains per-GPU activity and memory peaks plus energy when a source provides a usable counter. Writes are atomic. Raw samples, graphs, process rows, names, container identities, and command lines are never persisted.

GPUFlo performs no network access and has no telemetry upload, update check, remote API, service, or daemon.

## Desktop mini view

The persistent interactive mini view works well as a floating terminal dashboard:

```sh
alacritty \
  --class gpuflo,gpuflo \
  --title "GPUFlo Mini" \
  -o 'window.dimensions.columns=74' \
  -o 'window.dimensions.lines=15' \
  -e gpuflo --mode mini
```

For Niri, match the dedicated Wayland app ID:

```kdl
window-rule {
    match app-id="gpuflo"
    open-floating true
    open-focused false
    default-floating-position x=24 y=24 relative-to="bottom-right"
}
```

The same launch command can be placed behind a compositor keybinding or startup rule. `compact` provides more telemetry at `62×17` or larger; `tiny` supplies a persistent one-line widget.

## Multiple simultaneous instances

Multiple GPUFlo processes can run at once. They do not claim exclusive GPU access, ports, sockets, or singleton locks. Each instance owns its selection, tagline, graphs, session peaks, overlays, and sampling lanes.

One caveat: CLI instances normally share the daily summary file. Writes remain atomic, but concurrently exiting processes can replace one another's independently accumulated peaks. Isolate an occasional secondary instance with a session-local state root:

```sh
XDG_STATE_HOME="$XDG_RUNTIME_DIR/gpuflo-terminal" gpuflo
```

The normal configuration file remains shared because GPUFlo never writes it.

## Hotplug and lifecycle behavior

- New and returning GPUs are rediscovered and probed from scratch.
- A confirmed disconnected GPU is removed from subsequent snapshots without suppressing healthy devices.
- Selection moves to a surviving GPU and can return to the remembered stable identity when the device reappears.
- A running TUI or JSON stream can remain alive with zero currently detected GPUs and continue discovery.
- A confirmed XCP partition-configuration change stops output and requests a restart rather than joining incompatible old and new identities.
- Potentially blocking kernel, AMD SMI, process, discovery, and persistence work runs on bounded isolated lanes.
- Missed ticks are skipped, never replayed or queued indefinitely.

## Rust library

The package also exposes a narrow semver-supported Rust interface:

- canonical IDs, topology, observations, health, memory, power, temperature, timestamps, and snapshots;
- `Monitor` and `MonitorOptions`;
- owned `MonitorEvent::Snapshot`, `Notice`, and `Fatal` events;
- bounded receive, command, process-scope, peak-reset, and shutdown operations.

Minimal use:

```rust
use gpuflo::{Monitor, MonitorEvent, MonitorOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let monitor = Monitor::start(MonitorOptions::new())?;

    loop {
        match monitor.receive()? {
            MonitorEvent::Snapshot(snapshot) => {
                println!("observed {} physical GPU(s)", snapshot.gpus.len());
                break;
            }
            MonitorEvent::Notice(notice) => eprintln!("{}", notice.message),
            MonitorEvent::Fatal(error) => return Err(error.into()),
            _ => {}
        }
    }

    monitor.shutdown()?;
    Ok(())
}
```

`MonitorOptions::new()` disables daily persistence for embedders. Source adapters, reducers, histories, rendering state, terminal lifecycle, and unsafe AMD SMI handles remain private implementation details.

## Deliberate non-goals

GPUFlo does not:

- tune clocks, power, fans, partitions, or voltage;
- reset or mutate a GPU;
- infer LLM phases or report tokens per second;
- connect to inference-engine APIs;
- invent per-process GPU utilization;
- average unlike GPUs into a node-wide utilization percentage;
- hide missing telemetry behind zero or `N/A`;
- collect remote hosts or fleet telemetry;
- run a daemon, service, database, or web server;
- require root or recommend running the dashboard as root.

## Build, test, and validation

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
cargo package --locked
```

Validation is deliberately risk-based rather than exhaustive. It covers source fixtures, observation semantics, physical/XCP scope, bounded monitor journeys, text and JSON contracts, responsive rendering, actual PTY terminal restoration, packaging, and live hardware evidence. See [`validation/`](validation/) for the current manifest and exact qualification claims.

## License and notices

GPUFlo is licensed under the [MIT License](LICENSE). Locked dependency and reused-code notices are included in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt).

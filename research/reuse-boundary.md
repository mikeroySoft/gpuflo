# Reuse boundary: what gruflo should take from `flow` and `rocm-cli`

Research for [gruflo#2 — Identify the reusable code boundary](https://github.com/michaelroy-amd/gruflo/issues/2).

Every claim below is tagged **[F]** (fact verified against primary source, with a
citation) or **[R]** (recommendation / judgement by this research pass, not yet a
project decision). Nothing here resolves the ticket.

---

## 0. Sources inspected and how

| Source | What was read | Revision inspected |
| --- | --- | --- |
| `programmersd21/flow` | Full repository (4,065 lines of Go across 23 files) | `git clone --depth 1` of `main`, commit `60bec61911bc2eded48c3f2ba771600bcc4363a4`, dated 2026-08-19 |
| `ROCm/rocm-cli` | Local checkout `/home/miroy/git/rocm-cli`, crates `rocm-dash-core`, `rocm-dash-collectors`, `rocm-dash-tui`, `rocm-dash-daemon`, plus licensing/attribution tooling | Local `HEAD` = `eef5d7aaf7fb39d0fe103daafadf81af77b29e1d` (2026-08-18, "feat: add sampling controls for chat and serve commands (eai-7538) (#222)"); the checkout is **2 commits behind `origin/main`** (`origin/main` = `8cf6ec9`) |

**[F]** Both licenses were additionally verified against the live upstream, not
just the local copies:

- flow — <https://raw.githubusercontent.com/programmersd21/flow/main/LICENSE>
- rocm-cli — <https://raw.githubusercontent.com/ROCm/rocm-cli/main/LICENSE.TXT>

---

## 1. Licensing and attribution obligations

### 1.1 The two licenses

**[F]** `programmersd21/flow` is MIT. Exact notice line: `Copyright (c) 2026 flow
contributors` (`LICENSE`, upstream link above).

**[F]** `ROCm/rocm-cli` is MIT. Exact notice line: `Copyright (c) 2026 Advanced
Micro Devices, Inc.` (`LICENSE.TXT`, upstream link above).

**[F]** Both texts are the unmodified MIT template, including the operative
clause: *"The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software."* There is no
NOTICE-file clause (that is Apache-2.0), no patent grant, and no copyleft.

### 1.2 What that actually obliges gruflo to do

**[F]** MIT's only condition is preservation of the copyright notice + permission
notice when a copy or substantial portion of the covered software is
distributed. It attaches to *expression* (source text), not to ideas, layout
choices, sampling cadences, or metric field names.

**[R]** Concretely, gruflo should:

1. Ship its own `LICENSE` and, alongside it, a `THIRD_PARTY_NOTICES.md` (or
   `NOTICE`) containing the **verbatim MIT text plus the exact copyright line**
   of each upstream from which source was copied or translated — one block for
   `flow contributors`, one for `Advanced Micro Devices, Inc.` — each with the
   upstream URL and the commit SHA the code was taken at.
2. Add a per-file provenance header on any file that is a copy or a
   line-for-line translation, e.g.
   `//! Adapted from ROCm/rocm-cli crates/rocm-dash-tui/src/ui/sparkline.rs @ eef5d7a (MIT, © Advanced Micro Devices, Inc.)`.
   This is not strictly required by MIT when the notices file exists, but it is
   what makes the notices file auditable later.
3. Treat a **Go→Rust re-implementation of flow's algorithms as a derivative
   work** for attribution purposes when the port is close (same constants, same
   control flow, same edge-case handling). Ideas-only reuse (e.g. "have four
   responsive modes") is not.
4. Not use the AMD name, the ROCm mark, or "flow" as branding. **[F]** MIT
   grants no trademark rights; the licenses above are silent on marks, which
   means no permission is given.

### 1.3 Transitive lineage that needs diligence before copying

**[F]** Three rocm-cli files that are otherwise attractive to reuse carry
in-source statements of derivation from *other* projects:

- `crates/rocm-dash-tui/src/ui/sparkline.rs:11` — *"Inspired by btop's CPU graph
  and the matching widget in ctux."*
- `crates/rocm-dash-tui/src/ui/gradient.rs:13` — *"Inspired by btop's mem/cpu
  meters."*
- `crates/rocm-dash-tui/src/ui/theme.rs:14` — *"Pattern borrowed from ctux (see
  `../../../wiki/sources/ctux.md`)."*
- `crates/rocm-dash-tui/src/reconnect.rs:5` — *"Ported from ctux pattern."*
- `crates/rocm-dash-collectors/src/amd_smi.rs:7-8` — *"Field paths and the KFD
  pre-flight check are vendored from the TypeScript `AmdSmiProvider` in
  instinct-dash."*

**[F]** `aristocratos/btop` is **Apache-2.0**, not MIT
(<https://raw.githubusercontent.com/aristocratos/btop/main/LICENSE>).

**[F]** The `wiki/` directory those comments point at does **not exist** in the
rocm-cli checkout (`ls wiki` → no such file or directory), so `ctux` and
`instinct-dash` could not be located, licensed, or read from this workspace.
Neither appears to be a public repository reachable from the rocm-cli tree.

**[R]** Implications:

- The btop references read as *inspiration* ("inspired by"), and the code in
  `sparkline.rs` / `gradient.rs` is idiomatic Rust/ratatui with no C++ carried
  over; braille-cell packing is a well-known technique. Copying these two files
  under rocm-cli's MIT grant is defensible. If gruflo wants belt-and-braces, add
  a courtesy line crediting btop for the visual idea — Apache-2.0 attribution
  costs nothing and removes the argument.
- `amd_smi.rs` is the one file where the upstream-of-the-upstream (`instinct-dash`)
  is a non-public AMD project. rocm-cli distributes it under MIT with AMD's own
  copyright, which is the strongest possible position (the same entity owns both),
  so relying on rocm-cli's grant is sound. **[R]** Cite rocm-cli, not
  instinct-dash, as the provenance.

### 1.4 Attribution machinery worth copying wholesale

**[F]** rocm-cli enforces license hygiene with two mechanisms gruflo can mirror:

- `licenserc.toml` — [hawkeye](https://github.com/korandoru/hawkeye) config that
  requires an SPDX header (`Copyright © …` + `SPDX-License-Identifier: MIT`) on
  every `**/*.rs`, `**/*.py`, `**/*.sh`, `**/*.md`, with `third_party/**`
  excluded.
- `about.toml` + `cargo xtask tpn` → generated `THIRD_PARTY_NOTICES.txt`
  (619 KB, generated by `cargo-about` from the lockfile; header states "do not
  edit it by hand"). The `accepted = [...]` SPDX allowlist doubles as a tripwire:
  a dependency under an unlisted license fails `cargo xtask tpn --check`.

**[R]** For a tool the size of gruflo, adopt the `cargo-about` allowlist +
generated notices file from day one (cheap, and it makes the dependency-license
story automatic), and adopt the SPDX per-file header convention. The hawkeye
wrapper is optional; a pre-commit grep is enough at gruflo's scale.

---

## 2. `programmersd21/flow` — inventory

### 2.1 The single most important fact about reusing flow

**[F]** flow is **Go**, not Rust. `go.mod` declares `go 1.24.2` with direct
dependencies `charmbracelet/bubbletea v1.3.10`, `charmbracelet/bubbles v1.0.0`,
`charmbracelet/lipgloss v1.1.0`, `BurntSushi/toml v1.6.0`,
`shirou/gopsutil/v3 v3.24.5`.

**[F]** There is therefore **no ratatui code in flow at all**, and no Rust of any
kind. Every "widget" in flow is a string-building function rendered through
lipgloss (`internal/ui/views.go`), and the runtime model is Bubble Tea's
Elm-style `Init/Update/View` (`internal/ui/model.go`).

**[R]** Consequences for gruflo (Rust + ratatui): nothing from flow can be
*copied*; everything must be *re-derived*. What is genuinely portable is the
**numeric core** (ring buffer, sliding-window rate, spring easing, braille
rasterization, slope/trend glyph, unit formatting) and the **policy decisions**
(mode-selection algorithm, refresh ladder, history sizing, output modes). The
Bubble Tea/lipgloss layer maps onto ratatui's immediate-mode `Frame` draw with a
crossterm event loop, which is a different architecture — do not try to port
`model.go` structurally.

### 2.2 Types, collectors, and sampling policy

| Item | Location | Facts | Verdict **[R]** |
| --- | --- | --- | --- |
| `history.Ring` | `internal/history/ring.go:8-56` | Fixed-capacity `[]float64` ring, pre-allocated, `Push`/`Slice`/`Len`/`Cap`/`Reset`/`Last`. `Slice()` allocates a fresh copy per call. | **Adapt.** In Rust, prefer `VecDeque<f32>` with a `while len > cap { pop_front() }` trim (the pattern rocm-cli already uses, §3.3) and expose `.iter()` instead of `Slice()` so rendering does not allocate per frame. Take the idea, not the copy-on-read. |
| `history.Tracker` | `internal/history/ring.go:59-99` | Session peaks + "today" totals; `Record(down, up, intervalSecs)` **resets today's totals on a calendar-day change** by comparing `y/m/d`; accumulates `rate × interval` as a Riemann sum. `nowFunc` is an injectable clock (`ring.go:9`) used by tests. | **Adapt — high value.** This is exactly the "bounded session history + small persisted daily summaries" shape gruflo#1 already wants. The injectable clock is the reason the day-rollover tests exist (`ring_test.go:8-90` covers same-day, month change with same day-number, year change) — keep that seam. Note the Riemann sum is only meaningful for *rates*; for GPU it maps to energy (W×s → Wh), not to utilization. |
| Persistence | `internal/history/persist.go` | Single `stats.json` under `os.UserConfigDir()/flow/`, encode/decode via `encoding/json`, `MkdirAll 0o755`, missing file is a normal error path (`TestLoadMissing`, `persist_test.go:37`). Save is a plain `os.Create` + encode — **not atomic**. | **Adapt with a fix.** Same tiny-JSON shape is right for gruflo's daily summaries. Write via temp-file + `rename` for atomicity; flow's version can truncate on crash. |
| `collector.Collector` | `internal/collector/collector.go` | Reads counters via `gopsutil.net.IOCounters(true)`; `"auto"` picks the interface with the highest `BytesRecv+BytesSent`, skipping loopback and `docker`/`br-`/`veth`/`virbr`/`vmnet`/`vbox` prefixes (`isLoopback`, `collector.go:121-134`). Returns a `Snapshot{Interface, RxBytes, TxBytes}` of **cumulative counters**, never rates. | **Pattern only.** The device-selection heuristic is domain-specific to networking. The transferable rule is: *the collector returns raw cumulative readings; rate derivation lives one layer up*. |
| `sampler.Sampler` | `internal/sampler/sampler.go` | The sampling policy. A 4-slot sliding window (`const windowSlots = 4`, `sampler.go:25`) over per-tick counter deltas; rate = `Σdeltas / Σdt` so the reported figure is a ~1 s average at the 250 ms default. Guards counter wrap (`if snap.RxBytes >= prev.RxBytes`) and non-positive `dt`. Primes with a 10 ms throwaway read before the first real sample (`sampler.go:59-72`) so the first displayed value is not garbage. Emits on a buffered channel with a **non-blocking `default:` drop** so a slow UI never back-pressures sampling. | **Adopt the policy, port the code.** All four behaviors — window-averaging, priming read, monotonic-counter guard, lossy send — are directly applicable to GPU counters (energy accumulators, PCIe/throughput counters). For instantaneous gauges (temperature, power draw, utilization %) the window is an *optional smoother*, not a necessity. |
| `processes` overlay | `internal/processes/processes.go` | Enumerates TCP connections via gopsutil, groups by PID, resolves names, sorts by connection count desc. Header comment states plainly that **per-process bandwidth is unavailable without elevated privileges or eBPF/netlink**, so it shows connection counts instead. | **Copy the honesty, not the code.** gruflo's process overlay has the same shape of problem; `amd-smi process` gives per-process VRAM but not per-process utilization. The precedent: show what is genuinely measurable, and say in the code why the richer number is absent. |
| `ping.Measure` | `internal/ping/pinger.go` (16 lines) | TCP-dial latency probe to a configurable host (default `1.1.1.1`), re-run on a 5 s `tea.Tick` (`model.go:578-586`). | **Avoid.** Network egress in a local read-only GPU tool is scope creep and a privacy surprise. |

### 2.3 Animation, rendering, and "breathing"

| Item | Location | Facts | Verdict **[R]** |
| --- | --- | --- | --- |
| `animate.Spring` | `internal/animate/ease.go:32-58` | Critically-ish damped spring, `stiffness = 180.0`, `damping = 12.0` (`ease.go:6-7`); integrates `velocity += stiffness*(target-current)*dt`, then `velocity *= exp(-damping*dt)`, snaps velocity to 0 below `1e-4`, and defends against NaN/Inf on every input. Driven at `dt = 0.13` from the UI tick (`model.go:194-195`), with `tick()` firing every 130 ms (`model.go:570-574`). | **Port verbatim (translated).** ~25 lines, no dependencies, and it *is* the "breathing" quality gruflo#1 asks to preserve. The NaN/Inf hardening is the non-obvious part worth keeping. Note the coupling: `dt` is hard-coded to match the tick, so the animation is frame-rate-dependent by design. |
| `animate.Lerp` / `ColorLerp` / `Clamp01` | `internal/animate/ease.go:10-30` | Scalar and 8-bit RGB interpolation with clamping; `Clamp01` treats NaN as 0. | **Port**, or use rocm-cli's `lerp2`/`lerp3_t` (§3.4) which do the same job already in Rust. Prefer the latter — one less translation. |
| `sparkline.Render` (block) | `internal/sparkline/sparkline.go:12-61` | 8-level block ramp `' ▂▃▄▅▆▇█`, right-aligned to width, normalizes to the window max (floored at 1), optional 1-2-1 kernel smoothing when `len > 3`, maps ratio through `easeOutQuad` before bucketing. | **Adapt for the tiny/status-line mode.** Single-row block sparklines are the correct primitive for a one-line status output. |
| `sparkline.RenderBraille` | `internal/sparkline/sparkline.go:67-200` | Multi-row braille area graph: `width×2` dot columns, `height×4` dot rows, explicit U+2800 bit-mask table, **sub-character horizontal scrolling via a `frac` parameter** with linear interpolation between samples (`sampleAt`, `:222-260`), a 5-tap weighted smoothing pass (weights 1-2-3-2-1), and `easeOutQuad` shaping of peaks. `frac` is computed in the view as `elapsed / refreshInterval`, clamped to `[0,1]` (`views.go:463-475`). | **This is flow's signature effect.** The `frac`-driven sub-cell scroll is what makes the graph *flow* between samples instead of stepping. rocm-cli's `BrailleSparkline` (§3.4) has no equivalent. **[R]** If gruflo wants flow's feel, port `frac` interpolation *onto* rocm-cli's ratatui widget rather than choosing one or the other. |
| `sparkline.Slope` / `VelocityGlyph` | `sparkline.go:283-322` | Least-squares slope over the last `n` samples (`slopeWindow = 6`, `model.go:45`); glyph is `↗`/`↘`/`→` with a **relative** threshold of `5 %` of the current value, and a floor (`cur < 1 → →`) so noise near zero does not flicker. | **Port.** A trend glyph is a genuinely cheap way to answer "is it climbing?" in the one-second budget. The relative threshold + zero-floor are the details that make it not-annoying. |
| Rolling max with decay | `internal/ui/model.go:483-493` | `rollingMaxDown *= 0.995` every sample, then `max` with the new value. At 100 ms sampling that is a ~2.3 min half-life. Used as the graph's `maxVal` and as the color-intensity denominator. | **Adapt with care.** For GPU, several metrics have *known* absolute ceilings (utilization 0-100 %, VRAM total, power cap, junction-temp limit). **[R]** Use the true ceiling where one exists — a decaying observed max would make a 30 % load look like a full graph. Keep the decaying max only for unbounded quantities. |
| Theme model | `internal/theme/theme.go` | 8 built-in themes (`default`, `nord`, `dracula`, `gruvbox`, `forest`, `monochrome`, `catppuccin`, `tokyo-night`; `theme.go:36-318`). Each theme is 8 text/border/accent hex slots + **5-stop RGB gradients** for download and upload + 2-stop border gradients + a 4-stop logo gradient. `fiveStopGradient` (`:463-476`) segments `t` into 4 spans. `SpeedRatio(current, rollingMax)` (`:477-483`) is the single intensity input for all value/graph coloring — i.e. **color encodes "how fast relative to recent peak", not an absolute threshold**. | **Take the structure, change the semantics.** The 5-stop-per-signal gradient with one intensity scalar is a clean, tiny design. **[R]** For gruflo, health colouring should be *threshold*-based (temp/power/VRAM have meaningful danger zones) as rocm-cli does (§3.4), with relative-intensity gradients reserved for utilization-style signals. |
| Custom themes | `internal/theme/custom.go` | `$CONFIG/flow/themes/*.toml` scanned at `init()`; parse failures are silently skipped (`custom.go:42-62`). | **Adapt, but do not swallow errors.** Silent skip on a malformed theme file is a bad debugging experience. |

### 2.4 Responsive behavior — the highest-value idea in flow

**[F]** flow defines four modes: `ViewHero`, `ViewCompact`, `ViewMini`, `ViewTiny`
(`internal/ui/model.go:49-56`).

**[F]** The selection algorithm (`views.go:400-427`, `pickViewModeAndContent`) is:

1. An explicitly forced mode (CLI flag / `m` key) short-circuits everything.
2. `width < 40` **or** `height < 6` → `ViewTiny`.
3. Candidate list is `[Hero, Compact, Mini]`, narrowed to `[Compact, Mini]` when
   `width < 60`.
4. **Render each candidate's content and pick the first whose line count fits
   `height`**; if none fits, fall back to `ViewTiny`.

**[F]** Layout constants: `heroInnerMaxWidth = 80`, `compactInnerMax = 68`,
`HorizontalMargin = 4`, panel chrome `PanelExtraWidth = 6` (border 2 + padding
2×2) (`views.go:14-24`). Content is clamped to `min(termW - margin, maxInner)`
with a floor of 40 columns. Additional height-gated elements: the ASCII logo only
at `termH >= 28`, the "today" totals line only at `termH >= 20`, the ping line
only at `contentW >= 42` (`views.go:490-576`). Graph height is 4 rows, 3 in Mini.

**[F]** Two regression tests defend this, and they are unusually well-reasoned
(`internal/ui/resize_test.go`):

- `TestViewNeverExceedsTerminalHeight` — sweeps 8 widths × 16 heights and asserts
  the chosen mode's **untruncated** line count never exceeds the terminal height.
- `TestEffectiveViewModeFitsBeforeClamping` — exists because a prior bug measured
  the *already-clamped* `centerFrame` output, which made every candidate "fit"
  and silently disabled adaptivity while the first test still passed. It
  deliberately re-derives ground truth from `dashboardContentLines` so it catches
  a regression *in the measurement function itself*.

**[R]** Copy this whole approach — the measure-then-choose algorithm, the width
*and* height gating, the per-element height thresholds, and **both** tests
(including the second one's reasoning, which is the kind of thing a fresh
implementation always gets wrong once). In ratatui the equivalent is to build the
candidate layout, compute its required height, and only then commit to a `Layout`
split. This is the single most transferable thing in the flow repository.

### 2.5 Configuration, keys, and machine-readable output

**[F]** Config (`internal/config/config.go`): TOML at `os.UserConfigDir()/flow/config.toml`
with `XDG_CONFIG_HOME` fallback; **the file is created with defaults on first run**
and a write failure is non-fatal (warning to stderr, defaults used); a parse
failure *is* fatal-ish (returned as an error). Fields: `refresh` (duration string
via a `duration` newtype implementing `UnmarshalText`), `history` (seconds),
`theme`, `unit` (`auto|kb|mb|gb`), `interface`, `no_color`, `bits`, `ping_target`.
The default file is emitted from a `const defaultTOML` format string **with inline
comments** — so the generated config is self-documenting.

**[F]** History sizing couples config to sample rate: `histCap := cfg.History * 4`,
floored at 60 (`model.go:123-126`) — i.e. `history` is *seconds* and 4 is the
assumed samples-per-second at the 250 ms nominal interval.

**[F]** CLI (`cmd/flow/main.go:22-33`): `--tiny`, `--mini`, `--compact`, `--json`
(one-shot pretty JSON then exit), `--json-stream` (NDJSON lines, continuous),
`--once` (one-shot plain text), `--interface`, `--refresh`, `--no-color`,
`--bits`, `--ping`, `--version`. Flags override config after load
(`main.go:52-68`). `--no-color` is implemented by setting `NO_COLOR=1`, which
lipgloss honours automatically (`main.go:62-64`).

**[F]** All three non-interactive modes **discard the first sample and print the
second** (`runOnce`/`runTiny`, `main.go:134-201`) — because the first reading has
no delta to difference against.

**[F]** The one-shot JSON payload is a flat object with both machine and human
fields: `status`, `timestamp` (RFC3339), `interface`, `download_bps`,
`upload_bps`, `download_human`, `upload_human`, `peak_down_bps`, `peak_up_bps`,
`unit_display`. The stream payload drops `status`/`peak*` and uses
RFC3339**Nano**.

**[F]** Alt-screen is used for Hero mode only; `--compact`/`--mini` run inline
with no `tea.WithAltScreen()` (`main.go:117-120`), so they compose with shell
scrollback.

**[F]** Keymap (`internal/ui/keys.go`): `q`/`ctrl+c` quit, `esc` back, `r` reset
peaks (**two-press confirm**, 2 s timeout — `model.go:46`, `:323-339`), `i` cycle
interface, `I` interface info, `c` cycle units, `p` pause, `?` help, `m` cycle
mode, `n` processes, `b` bits/bytes, `d` cycle display filter, `+`/`-` refresh
ladder, `t` theme picker. The refresh ladder is a fixed 12-step list from 50 ms
to 300 s (`model.go:405-419`), and changing it **tears down and restarts the
sampler goroutine** (`:440-451`).

**[F]** Theme picker previews live (arrow keys re-apply the theme immediately),
`esc` restores the original, `enter` **persists to the config file**
(`model.go:248-275`).

**[R]** Worth copying: the four output modes and their exact ergonomics
(one-shot JSON / NDJSON stream / one-shot text / status-line), the
discard-first-sample rule, dual machine+human fields in JSON, config file
generated-with-comments on first run, non-fatal config write failure, `NO_COLOR`
support, inline (non-alt-screen) rendering for the small modes, two-press
destructive-action confirm, and live-preview-then-persist for theme selection.

**[R]** Worth *not* copying: restarting the collector goroutine to change the
interval (just re-arm the timer); the 12-step ladder down to 50 ms (an `amd-smi`
subprocess cannot sustain 50 ms — see §3.2); coupling `history` in seconds to a
hard-coded ×4 (make the ring capacity derive from the *actual* interval).

### 2.6 flow's CI

**[F]** `.github/workflows/ci.yml`: three jobs — `golangci-lint`; `go vet` +
`go test ./... -race -count=1` on ubuntu/macos/windows; and a build+artifact
upload on the same matrix. Concurrency group cancels in-progress runs per ref.

**[R]** The Rust analogue (clippy + `cargo test` + `cargo build` matrix, with
`cancel-in-progress`) is the right shape for gruflo. gruflo is Linux/ROCm-only
per gruflo#1, so the OS matrix collapses to `ubuntu-latest`; keep the job split.

---

## 3. `ROCm/rocm-cli` — inventory

**[F]** Scale context: the `rocm-dash-tui` crate alone is 35,479 lines across 62
files; `rocm-dash-core` is 3,658; `rocm-dash-collectors` is 3,959;
`rocm-dash-daemon` is 4,687. `crates/rocm-core/src/lib.rs` is a single file of
over 7,300 lines. gruflo wants a "small, read-only, local" tool — so the reuse
here is necessarily **surgical extraction, never a fork**.

### 3.1 The GPU metric types — `crates/rocm-dash-core/src/metrics.rs`

**[F]** `GpuMetrics` (`metrics.rs:38-45`) is exactly seven fields, snake_case,
**units encoded in the field names** (a stated convention, `metrics.rs:5`):
`device_id: String`, `vram_used_mb: u64`, `vram_total_mb: u64`,
`gpu_utilization_pct: f32`, `temperature_c: f32`, `power_w: f32`,
`clock_mhz: Option<f32>`. Only the clock is optional.

**[F]** `GpuSystemInfo` (`:49-62`): `rocm_version`, `driver_version` (both
`Option<String>`), `gpu_model: String`, `physical_gpu_count`,
`logical_gpu_count`, `partition_mode`, `memory_partition_mode`,
`compute_partition_mode`, `vram_per_logical_gpu_mb`, plus four engine-specific
version fields (`lemond_version`, `llama_server_build`, `ccr_version`,
`llamacpp_backend`).

**[F]** `Snapshot` (`:14-21`) is `{ timestamp: DateTime<Utc>, host: SystemMetrics,
gpus: Vec<GpuMetrics>, gpu_system_info: Option<GpuSystemInfo>, instances:
Vec<Instance>, warnings: Vec<String> }` — note the **`warnings` channel carried
with the data**, so degradation is part of the payload rather than a log line.

**[F]** `ObservationFreshness { Fresh, Held }` + `ObservationMetadata {
observed_at, freshness }` (`:158-175`) exist so a displayed value can say "this
number is carried forward from a prior scrape". The doc comment is explicit that
absent metadata means *unknown*, and is **never fabricated as `Fresh`**.

**[R]** Adopt:
- The seven-field `GpuMetrics` shape and the units-in-field-names convention —
  it is already the right vocabulary for gruflo, and matching it keeps any future
  interop cheap.
- `Option<T>` for "capability may be absent" (gruflo#1's "model capabilities
  explicitly rather than inventing values"). **[R]** Go *further* than rocm-cli:
  `vram_used_mb`, `gpu_utilization_pct`, `temperature_c` and `power_w` are
  non-optional here and default to `0` on parse failure (see §3.2) — for gruflo
  those must be `Option`, because `0 °C` and `0 W` are lies, not measurements.
- The `warnings: Vec<String>` channel on the snapshot.
- The `Fresh`/`Held`/unknown freshness triple, and the `*` "held" marker
  convention (`ui/format.rs:175-179`, `HELD_MARKER` / `HELD_LEGEND`).

**[R]** Drop: `SystemMetrics` (CPU/mem/disk/net — gruflo is a GPU tool),
`Instance`/`InstanceStatus`/`StartupPhase` and every serving-related field
(`kv_cache_usage_pct`, `gen_tps`, `ttft_ms`, `tpot_ms`, `tokens_per_watt`,
`tensor_parallel_size`, …), and the four engine-version fields on
`GpuSystemInfo`.

### 3.2 The amd-smi collector — `crates/rocm-dash-collectors/src/amd_smi.rs`

This is the highest-value single file in either repository for gruflo.

**[F]** Safety pre-flight (`amd_smi.rs:36-63`): `detect()` returns `Some` **only
if `/dev/kfd` is readable AND `amd-smi version` succeeds**. The doc comment
states the reason bluntly: *"The KFD pre-flight is mandatory: without it,
`amd-smi` blocks in uninterruptible kernel sleep (D-state) that no signal can
escape."*

**[F]** Timeouts: `DETECT_TIMEOUT = 5 s`, `RUN_TIMEOUT = 10 s`
(`amd_smi.rs:23-24`); every JSON invocation is wrapped in `tokio::time::timeout`
(`:104-111`).

**[F]** Binary resolution is injectable — `detect_with_binary()` exists because
"the managed ROCm SDK ships `amd-smi` inside the runtime wheel's bin directory
rather than on `PATH`" (`:40-45`). The resolver itself
(`rocm_core::resolve_amd_smi_binary`, `crates/rocm-core/src/lib.rs:7295-7345`)
walks a managed-runtime registry, sorted by install time.

**[F]** Commands used: `amd-smi metric --json`, `amd-smi process --json`,
`amd-smi version --json`, `amd-smi static --json`, `amd-smi topology --json`. The
three system-info calls run concurrently via `tokio::join!` and each is
**tolerated independently** (`.ok()` per call — mirrors a `Promise.allSettled`,
`:79-89`).

**[F]** Metric field paths (`parse_metrics`, `:132-159`), from
`gpu_data[]`: `mem_usage.used_vram.value`, `mem_usage.total_vram.value`,
`usage.gfx_activity.value`, `power.socket_power.value`,
`clock.gfx_0.clk.value`, and temperature preferring
`temperature.hotspot.value` with fallback to `temperature.edge.value` —
because *"edge is N/A on MI300X SR-IOV"* (`:139`). `device_id` is synthesized as
`format!("gpu-{id}")` from the `gpu` index.

**[F]** Missing fields **default to `0.0`/`0`** via `unwrap_or`, and a test
enshrines this (`missing_fields_default_safely`, `:373-380`).

**[F]** Process parsing (`parse_processes`, `:221-253`) is deliberately defensive
across amd-smi versions: accepts `{gpu_data: [...]}` *or* a bare top-level array;
`process_list` *or* `processes`; items flat *or* wrapped in `process_info`; `pid`
raw *or* `{value}`-wrapped; VRAM under four candidate paths (`VRAM_PATHS`,
`:163-168`) as `{value, unit}` *or* a raw number. `mem_unit_to_mb` (`:174-182`)
normalizes `B/BYTES/KB/KIB/GB/GIB` case-insensitively, treats binary and decimal
prefixes as aliases, and defaults unknown units to MB; a bare number is
**documented as bytes**. Entries with no resolvable PID are skipped; unknown
shapes yield an empty `Vec`, never a panic.

**[F]** Partition-mode parsing (`:310-337`) is case-insensitive over
`SPX/DPX/QPX/CPX` and `NPS1/NPS2/NPS4`, with an `Unknown` fallback, and reads
`partition_mode` or `compute_partition_mode` interchangeably.

**[F]** Tests in-file (`:340-511`): fixture-driven parse tests using inline JSON
constants, an edge-fallback test, a case-insensitivity test, a
market-name-vs-product-name test, four process-shape tests, a garbage-input test
(`{}`, `[]`, `"garbage"`, `42` all → empty), and a hardware-only test marked
`#[ignore = "requires a real AMD GPU + amd-smi; run manually on hardware"]` whose
asserted contract is simply *"does not panic"*.

**[R]** Verdict: **copy this file more or less wholesale**, then change three
things.

1. Replace `unwrap_or(0.0)` with `Option<T>` propagation. gruflo#1 explicitly
   requires modelling capability rather than inventing values; rocm-cli's
   zero-defaults are the one place its behavior contradicts gruflo's stated
   product rule.
2. Drop `rocm_core::resolve_amd_smi_binary` entirely — it is the runtime
   dependency on rocm-cli that gruflo#1 forbids. Keep the *seam*
   (`detect_with_binary`) and resolve via `PATH` + a `--amd-smi` / config
   override.
3. Decide the process-collection cadence separately from metrics; `amd-smi
   process` is a second subprocess.

**[R]** Also carry over the **10 s timeout and the `/dev/kfd` pre-flight verbatim,
including the comments**. That D-state hazard is exactly the kind of hard-won
knowledge a fresh implementation loses, and it is the difference between a tool
that degrades and one that hangs unkillably.

**[R]** Sampling-rate implication **[F-grounded]**: metrics come from a
subprocess spawn + JSON parse with a 10 s worst case. rocm-cli's default
`gpu_tick` is `1 s` (`crates/rocm-dash-daemon/src/runner.rs:85`). flow's 100 ms
default and 50 ms floor are not reachable through `amd-smi`. gruflo's refresh
ladder should therefore start at ~250 ms–1 s, and the UI animation tick must be
decoupled from the sample tick (flow already separates them: 130 ms render tick
vs configurable sample interval, `model.go:570`).

### 3.3 Sampling cadence, history, and state

**[F]** Tick defaults (`runner.rs:83-88`): `gpu_tick = 1 s`,
`instance_tick = 2 s`, `discovery_tick = 5 s`, plus
`SYSINFO_REFRESH_SECS = 30` (`runner.rs:35`). Slower cadences are expressed as
**integer multiples of the base tick** and fired with
`tick_count.is_multiple_of(n)` (`runner.rs:134-137`, `:262`, `:468`) rather than
as independent timers.

**[F]** `ticker.set_missed_tick_behavior(MissedTickBehavior::Skip)`
(`runner.rs:132`) — a stalled collector does not produce a burst of catch-up
ticks.

**[F]** The base tick is injectable for tests: `tick_override: Option<Duration>`
*"lets tests run faster than `opts.gpu_tick`; production passes None"*
(`runner.rs:118-130`).

**[F]** History caps: `SNAPSHOT_RING_CAP = 300` snapshots in the reducer
(`crates/rocm-dash-core/src/state.rs:16`) — i.e. 5 minutes at 1 Hz;
`BENCH_RING_CAP = 10_000`; `JOB_OUTPUT_RING_CAP = 1_000` (`state.rs:19-22`). The
ring is a `VecDeque<Snapshot>` trimmed with
`while self.history.len() > SNAPSHOT_RING_CAP { pop_front() }` (`state.rs:154-157`).

**[F]** `State::apply(StateEvent) -> Vec<SideEffect>` (`state.rs:148`) is a pure
reducer: state mutation returns a list of effects for an outer layer to perform;
the core crate has **no tokio and no ratatui** (`rocm-dash-core/Cargo.toml`
depends only on serde, serde_json, chrono, thiserror, toml, dirs, tracing).

**[F]** `Pause` makes `Tick` a no-op that returns no side effects
(`state.rs:180-183`) — pausing drops samples rather than buffering them; a test
pins this (`pause_drops_ticks`).

**[F]** `SnapshotRing` in the daemon (`crates/rocm-dash-daemon/src/snapshot_ring.rs`)
is a second, simpler bounded `VecDeque` with `push`/`latest`/`iter`/`len`.

**[F]** The TUI's own event/render loop ticks at 250 ms
(`crates/rocm-dash-tui/src/app/mod.rs:1657`: `interval(Duration::from_millis(250))`).

**[R]** Adopt: the pure-reducer core with a dependency-free crate boundary (it is
what makes the 611-line characterization test suite possible); multiples-of-a-base-tick
scheduling; `MissedTickBehavior::Skip`; the injectable tick for tests; bounded
`VecDeque` history with an explicit named capacity constant; pause-drops-samples
semantics. **[R]** Size gruflo's ring from *wall-clock coverage × rate* rather
than a bare count, and state the coverage in the constant's doc comment (300 @ 1 Hz
= 5 min is only obvious once you know the tick).

### 3.4 ratatui widgets and rendering — `crates/rocm-dash-tui/src/ui/`

| Widget / helper | File | Facts | Verdict **[R]** |
| --- | --- | --- | --- |
| `BrailleSparkline` | `ui/sparkline.rs` (~250 lines with tests) | Real `impl Widget for BrailleSparkline`. Packs 2 samples per cell × 4 dot rows (`2N` x-bins at `4×height` resolution); **right-aligns** by taking the last `cols*2` samples; builder API `.max(u64)`, `.style(Style)`, `.gradient(start, mid, end)`; per-cell color chosen from the **larger of the cell's two samples** so peaks keep the end color; hand-written U+2800 bit-mask table documented with an ASCII diagram. Six unit tests including exact braille code-point assertions and two `Buffer`-level gradient tests. Data type is `&[u64]`. | **Copy.** This is the ratatui equivalent of flow's braille renderer, already written, already tested. **[R]** Two changes: take `&[Option<f64>]` or carry a companion validity mask so "no reading" renders as a gap rather than as zero; and port flow's `frac` sub-cell scrolling (§2.3) onto it for the breathing quality. |
| `GradientGauge` | `ui/gradient.rs` (~250 lines with tests) | Three-stop horizontal gradient bar, written because *"ratatui's built-in `Gauge` only supports a single color"*. Defaults green `#1aa01a` → amber `#f59e0b` → red `#ed1c24`. Fill length tracks the ratio **and** the fill color sweeps across the width. Optional centered bold label. Non-RGB colors degrade to mid-grey (`rgb_of`). Six tests covering endpoint exactness, midpoint arithmetic, zero/full ratio, and label centering. | **Copy.** Directly serves gruflo's VRAM / utilization / power-cap bars. **[R]** Note the semantic: color varies by *position across the bar*, so a 100 % bar always shows red at its right end. For a health read-out, gruflo probably wants color by *value* (threshold), which is what `lerp3_t` gives — see next row. |
| `lerp3_t` / `lerp2` / `blend` | `ui/gradient.rs:130-152`, `ui/panel.rs:71-83` | `lerp3_t(stops, t)` interpolates a 3-stop ramp by a unit-interval **value** (first half stop0→stop1, second half stop1→stop2), explicitly exposed *"for widgets that need to color samples by their value, not their position"*. `blend(a, b, t)` is the `f32` two-color mix used for focus highlighting. | **Copy.** These four small functions replace flow's `ColorLerp`/`fiveStopGradient` and are already Rust. |
| `Heatmap` / `HeatmapRow` | `ui/heatmap.rs` (346 lines) | "metric × time" matrix; one char per cell, background from `lerp3_t(value / row_max)`; per-row max and optional per-row gradient override; optional left labels; **right-aligns and drops oldest columns when wider than the area** — stated as matching how the rest of the dashboard presents history. | **[R] Consider, don't commit.** A util/temp/power/vram × time heatmap is a strong answer to "what is my GPU doing" for multi-GPU hosts in a small vertical budget. But it is a second history renderer alongside the sparkline; only take it if a multi-GPU layout actually needs it. |
| `panel` / `BoxRole` | `ui/panel.rs` (476 lines) | Single source of box chrome: rounded corners, a btop-style inline title whose border "dips down" around the label, and a **semantic `BoxRole`** (`Primary`, `Secondary`, `Success`, `Warning`, `Danger`, `Muted`, `Neutral`) that drives both border color and a faint per-role surface tint, so *"color means something (telemetry vs action vs warning)"*. Focus brightens the border by blending 30 % toward `fg`. `padding_for(area)` is **adaptive**: left padding 2 at `width >= 8`, 1 at `>= 3`, else 0; top padding 1 only at `height >= 5`. | **Adopt the concept; write your own chrome.** The `BoxRole` idea (semantic role → color, one helper for all boxes) and adaptive padding are excellent and small. The 476-line implementation carries scrollbar plumbing gruflo may not need, and gruflo#1 says not to clone flow's/rocm-dash's exact layout. |
| `Theme` | `ui/theme.rs` (604 lines) | 11 semantic slots (`bg, surface, surface_2, fg, muted, accent, accent_2, ok, warn, err, border`) + a `StatusTone` enum (`Neutral, Muted, Info, Accent, Warning, Success, Error, Alert`). **Two construction paths**: bespoke `const fn` constructors, and `from_palette(&Palette16)` which reduces a canonical 16-color ANSI palette to the 11 slots. A `REGISTRY: &[(&str, ThemeCtor)]` of 15 themes with `from_name` and `next_name` (cycling). | **Copy the model, trim the catalogue.** The 11-slot semantic palette + `Palette16` reduction is a better foundation than flow's per-signal gradient stops, because adding a theme becomes "paste 18 hex values". **[R]** Ship 3-5 themes, not 15, and keep the `next_name` cycling key. |
| `ui/format.rs` | 578 lines | Small pure formatters: `mib`, `mib_pair(used,total)`, `pct`, `pct_opt`, `si`, `bps`, `duration`, `watts`, `celsius`, `mhz`, plus `HELD_MARKER = "*"` / `HELD_LEGEND = "* = held (prior scrape window)"` and `gen_tps_cell`/`gen_tps_compact`/`gen_tps_aggregate` variants that append the held marker. | **Copy the generic half** (`mib`, `mib_pair`, `pct_opt`, `watts`, `celsius`, `mhz`, `duration`, `si`) and the held-marker convention. Leave the `gen_tps_*` family (serving-specific). The `_opt` suffix convention — an `Option` formatter that renders `—` for absent — is exactly what gruflo's capability modelling needs. |
| Responsive "truncation by omission" | `ui/tabs/hardware.rs:379-424` (`gpu_compact_line`) | One dense GPU line: marker, id (truncated to 7), util bar, util %, threshold-colored temperature and power; the VRAM segment is appended **only if `base_width + vram_width <= width`**. Test `gpu_compact_line_colors_and_truncates` (`hardware.rs:931-950`) asserts the err color at 95 °C/740 W and that the `GB` segment is present at width 120 and absent at width 24. | **Copy the technique and the test.** Dropping whole semantic segments beats ellipsizing text: the line stays parseable at every width. Combined with flow's measure-then-choose mode selection (§2.4), these two give gruflo its complete responsive story. |
| Layout gating | `ui/mod.rs:283-289`, `ui/modal.rs:407-416`, `ui/tabs/home.rs:158-219` | Layout helpers return `Option` and **decline to render** when the area is too small — e.g. the three-column body returns `None` when `body.width < left + right + min_center || body.height < 5`; `menu_fits(w,h)` requires `w >= 31 && h >= MENU_ITEMS_Y + MENU_ITEM_COUNT`. `modal.rs:411` documents a fixed bug where a too-loose height guard painted a logo with no menu under it. | **Adopt the idiom**: layout functions return `Option<Layout>`; callers fall back. It makes "too small" a first-class, testable state instead of a rendering accident. |
| Threshold coloring | `ui/widgets.rs:13-26` (constants), `:38-60` (`temperature_style`, `power_style`) | Named constants `TEMP_WARN_C = 60.0`, `TEMP_CRIT_C = 80.0`, `POWER_WARN_W = 525.0`, `POWER_CRIT_W = 700.0` map a value to `theme.warn`/`theme.err`. The doc comment states the rationale outright: *"Fixed semantic thresholds make heatmap/gauge colors mean 'near the limit' rather than 'near the largest value seen this session'"*, tuned for MI355X-class parts (TDP ~750 W), and notes that **no per-GPU TDP exists in the data model**, so these are constants. Tests pin the mapping (`:178-195`). | **Adopt — and note the caveat.** This is the health-color semantics gruflo needs, and the explicit counterpoint to flow's relative-intensity coloring (§2.3). **[R]** gruflo should do better than hard-coded constants where `amd-smi static` exposes a real power cap / temperature limit per ASIC, falling back to constants only when it does not. |

### 3.5 Pure derivation helpers worth lifting

**[F]** `crates/rocm-dash-core/src/efficiency.rs:12-26` — `normalize_gpu_id(id)`
strips a `gpu-` prefix so amd-smi's `"gpu-0"` joins against bare indices `"0"`
from `HIP_VISIBLE_DEVICES`; anything else passes through unchanged.
`device_in(device_id, gpu_ids)` is the normalized membership test.

**[F]** `crates/rocm-dash-core/src/vram.rs` — pure attribution math, no I/O.
`device_vram(gpu_ids, gpus)` sums used/total over matching devices;
`aggregate_process_vram(procs, resolve)` takes an **injected `Fn(u32) -> Option<String>`**
PID resolver so all `/proc` I/O stays out of the core and the aggregation is unit
testable. Documented degradation: an instance matching nothing resolves to
`(0, 0)`, *"never a panic and never a confidently-wrong number."*

**[F]** `crates/rocm-dash-collectors/src/host.rs` — `sysinfo`-based host metrics;
notable detail: the constructor performs a priming refresh because *"the very
first read is meaningless"* for CPU deltas (`host.rs:26-28`) — the same
priming rule flow's sampler uses.

**[F]** `crates/rocm-dash-collectors/src/sysfs.rs` is a **39-line stub** for
Strix Halo (gfx1151) — every method returns `CollectorError::Unsupported`. There
is no working sysfs/hwmon collector in rocm-cli.

**[R]** Take `normalize_gpu_id` / `device_in` (5 lines, saves a real class of
join bug), take the injected-resolver pattern from `vram.rs` for any `/proc`
work, and take the priming-read rule. **[R]** Do **not** expect a sysfs fallback
from rocm-cli: if gruflo wants to work without `amd-smi`, that collector must be
written from scratch (and is a separate research question — see §6).

### 3.6 Configuration and persistence

**[F]** `crates/rocm-dash-core/src/config.rs` — TOML at
`dirs::config_dir()/rocm-dash/config.toml`. **Missing file → defaults with a
`debug!`; unreadable/unparseable → `warn!` + defaults** (`load_default`,
`:33-44`) — note this differs from flow, which surfaces a parse error. Durations
serialize as seconds through a `duration_secs` serde module. `DaemonConfig`
carries `listen`, `token`, the three ticks, `bench_results_dir`; `TuiConfig`
carries `connect`, `theme`, and three `chat_*` fields.

**[F]** Socket-path resolution (`config.rs:107-185`) is a three-tier chain
(`$XDG_RUNTIME_DIR` → `$HOME/.rocm/data/telemetry` → `temp_dir()/rocm-<user>`)
with the username **sanitized to alphanumerics/hyphen/underscore so a path
separator or `..` cannot escape the subdirectory**, and it is factored into a
pure `socket_path(...)` function taking explicit env inputs *"so the precedence
is testable without mutating process-global env vars (unsafe and racy under
parallel tests in edition 2024)."*

**[F]** `crates/rocm-dash-core/src/persist.rs` — NDJSON session format, one
`PersistedEntry { ts_us, event }` per line, wallclock microseconds recorded so a
replayer can pace playback against real-time deltas.

**[R]** Adopt: the pure-function-over-explicit-env-inputs testing idiom (it
applies to gruflo's config path resolution too), the sanitization habit, and
NDJSON-with-timestamps if gruflo ever adds a record/replay mode. **[R]** Prefer
flow's louder handling of a *malformed* config over rocm-cli's silent
warn-and-default. **[R]** Skip the daemon socket machinery entirely (§4).

### 3.7 Test patterns

**[F]** `crates/rocm-dash-tui/tests/dash_characterization.rs` (611 lines) —
freezes `ui::draw` output for every tab as **`TestBackend` buffer-text
assertions**, built from a synthetic `Snapshot` fixture, plus *"a squeezed-height
no-panic sweep"*. Its header explicitly says: reuse the existing
`TestBackend → Terminal → ui::draw → flatten-buffer` pattern, no new framework.

**[F]** `crates/rocm-dash-tui/tests/dash_journeys.rs` — drives real state
transitions through the public `AppState` API and then renders, *"rather than a
single static frame"*, because the E2E cucumber suite is black-box over the CLI
and cannot drive an interactive terminal. It also records that the key→action
reducer is module-private, so keystroke tests live in-module.

**[F]** Widget tests assert on the `Buffer` directly (e.g.
`gradient_colors_peak_cell_with_end_stop` reads `buf.cell((0,0)).style().fg`;
`render_fills_expected_cells_per_ratio` counts non-`Reset` backgrounds).

**[F]** Hardware-dependent tests are `#[ignore]`d with a reason string and assert
only "does not panic" (`amd_smi.rs:503-510`).

**[R]** This is the test strategy gruflo should adopt verbatim, at gruflo's
smaller scale:
1. `TestBackend` render characterization per screen, from synthetic snapshots.
2. A **squeezed-size sweep** asserting no panic and no overflow — merged with
   flow's width×height mode-selection sweep (§2.4), this is one table-driven test.
3. `Buffer`-level assertions for custom widgets (braille code points, gauge fill
   counts, gradient endpoints).
4. Fixture-JSON parse tests for the amd-smi collector, including garbage input.
5. `#[ignore]`d hardware smoke tests with an explicit reason.
6. Injectable clock / tick / env inputs so none of the above needs real time,
   real hardware, or process-global mutation.

---

## 4. Couplings to avoid

**[F]** = the coupling exists as described; **[R]** = the avoidance judgement.

### From rocm-cli

| Coupling | Evidence **[F]** | Why avoid **[R]** |
| --- | --- | --- |
| The daemon / IPC architecture | `rocm-dash-daemon` (4,687 lines): `server.rs`, `registry.rs`, unix/tcp `listen` with a shared-secret `token`, `protocol.rs` `Event` wire type, `SnapshotRing` for *"late-joining clients to hydrate"* | gruflo#1 puts "a daemon/API service" out of scope. A local read-only tool should sample in-process. Taking `protocol.rs` would drag the client/server split in with it. |
| `rocm-core` | `crates/rocm-core/src/lib.rs` is a single file >7,300 lines; `resolve_amd_smi_binary` lives at `:7295` | Depending on it for one path-resolution function imports a managed-runtime registry, TheRock SDK manifests, install/uninstall logic, and the whole crate graph. It is also the concrete mechanism by which gruflo would end up *requiring rocm-cli at runtime* — explicitly forbidden by gruflo#1. |
| Container / serving discovery | `rocm-dash-collectors` depends on `bollard 0.17` (Docker), `reqwest 0.12`, `csv`, `regex`, `tokio` **`features = ["full"]`**; `docker.rs` (435), `vllm_prom.rs` (287), `lemonade.rs` (332), `bench_load.rs` (1,509), `cgroup.rs`, `engine_registry.rs` | Every one of these is inference-serving telemetry. gruflo#1 puts process/container/engine telemetry out of the primary dashboard. Dropping them removes Docker, HTTP, CSV and regex from gruflo's dependency tree outright. |
| Agent / chat / LLM surfaces | `rocm-dash-tui/src/agent.rs` (2,494), `llm.rs` (549), `skills.rs` (632), `app/chat.rs` (669), `app/slash.rs` (536), `TuiConfig.chat_url/chat_model/chat_auth_header` | Entirely orthogonal to "what is my GPU doing". |
| The manager/wizard UI family | `onboarding.rs` (1,034), `serve_wizard.rs` (971), `runtime_manager.rs` (797), `install_manager.rs` (609), `automations_manager.rs` (543), `services_manager.rs` (489), `config_manager.rs` (485), `engine_manager.rs` (456), `update_manager.rs` (454), `folder_browser.rs` (385), `model_picker.rs` (325) | ~6,500 lines of install/configure/mutate UI. gruflo is strictly read-only; these are the mutation surfaces. |
| The 6,796-line `app/mod.rs` | `crates/rocm-dash-tui/src/app/mod.rs` | The tab/modal/job/chat state machine for a 5-tab IA. Reading it for ideas is fine; importing its structure would import the product it belongs to. |
| Job/bench subsystem in core | `state.rs` `JobState`/`JobStatus`/`SpawnJob` side effect, `BENCH_RING_CAP`, `bench_rollup.rs`, `bench_schema.rs` | gruflo spawns nothing. Lifting the reducer means deleting the job arms, not keeping them "for later". |
| `sysinfo` host metrics | `rocm-dash-collectors/src/host.rs`, `SystemMetrics` | CPU/mem/disk/net is a different tool. Keep the priming-read *lesson*, drop the dependency. |
| Zero-defaulting absent telemetry | `amd_smi.rs` `unwrap_or(0.0)`; test `missing_fields_default_safely` | Directly contradicts gruflo#1's "model capabilities explicitly rather than inventing values". This is the one rocm-cli behavior gruflo must deliberately *not* inherit. |

### From flow

| Coupling | Evidence **[F]** | Why avoid **[R]** |
| --- | --- | --- |
| Bubble Tea / lipgloss architecture | `internal/ui/model.go` `Init/Update/View`, `tea.Cmd`, `tea.Msg` | Elm-style message passing with commands vs ratatui's immediate-mode draw + crossterm poll. A structural port fights both frameworks. Port functions, not the model. |
| gopsutil | `go.mod`, `internal/collector`, `internal/processes` | Cross-platform host stats; irrelevant to an AMD/ROCm/Linux tool. |
| Network egress (`ping`) | `internal/ping/pinger.go`, 5 s tick in `model.go` | Unexpected outbound traffic from a local monitoring tool. |
| Goroutine-restart-to-reconfigure | `model.go:426-446` (interval change), `:346-364` (interface change) | Tears down and rebuilds the sampler to change a number. Re-arm the timer instead. |
| `Slice()` copying the ring every frame | `internal/history/ring.go:33-39`, called from `views.go:454-455` | An allocation per graph per frame. Iterate the `VecDeque` in place. |
| Silent config/theme failure paths | `theme/custom.go:53-60` (parse error → `continue`) | Malformed user files should say so. |
| Non-atomic stats write | `history/persist.go:35-46` | Temp-file + rename instead. |
| 50 ms refresh floor | `model.go:400-414` | Unreachable through an `amd-smi` subprocess (§3.2). |

---

## 5. Consolidated recommendation **[R]**

A minimal-surface reuse plan, ordered by confidence:

**Copy (translate to Rust where needed), with attribution:**

1. `amd_smi.rs` collector — KFD pre-flight, timeouts, field paths, hotspot/edge
   fallback, defensive process parsing with unit normalization, partition
   parsing, and its fixture tests. *(rocm-cli, MIT)* — modified to return
   `Option` instead of zero-defaults, and with the `rocm-core` binary resolver
   replaced by `PATH` + explicit override.
2. `BrailleSparkline` and `GradientGauge` widgets + `lerp2`/`lerp3_t`/`blend`,
   with their `Buffer`-level tests. *(rocm-cli, MIT; visual lineage: btop,
   Apache-2.0)* — extended with `Option`-aware gaps and flow's `frac` sub-cell
   scroll.
3. `animate.Spring` + `Clamp01`, including the NaN/Inf hardening. *(flow, MIT)*
4. `sparkline.Slope` / `VelocityGlyph` trend glyph with its relative threshold
   and zero-floor. *(flow, MIT)*
5. `normalize_gpu_id` / `device_in`. *(rocm-cli, MIT)*
6. Generic formatters from `ui/format.rs` (`mib`, `mib_pair`, `pct_opt`,
   `watts`, `celsius`, `mhz`, `duration`, `si`) and the `_opt`/`—` and held-`*`
   conventions. *(rocm-cli, MIT)*

**Adapt (same design, gruflo's own code):**

7. Mode-selection: flow's measure-then-choose over an ordered candidate list,
   width **and** height gates, per-element height thresholds — plus rocm-cli's
   `Option`-returning layout helpers and truncation-by-omission. Bring **both**
   of flow's resize regression tests, including the second one's rationale.
8. History: bounded `VecDeque` with a named capacity constant documented in
   wall-clock terms; a `Tracker` with an injectable clock for daily rollover;
   atomic JSON persistence of the small daily summary.
9. Sampling: window-averaged rates for counters, priming read, monotonic-counter
   guard, lossy hand-off to the UI; base tick with integer multiples for slower
   collectors; `MissedTickBehavior::Skip`; injectable tick for tests; a refresh
   ladder starting no faster than ~250 ms; render tick decoupled from sample tick.
10. Theme: rocm-cli's 11-slot semantic palette + `Palette16` reduction +
    name registry/cycling, trimmed to 3-5 themes; threshold coloring for health
    metrics, relative-intensity gradients for utilization-style signals.
11. Chrome: the `BoxRole` semantic-role idea and adaptive padding, in gruflo's
    own (much smaller) box helper — not rocm-cli's 476-line `panel.rs`.
12. Config/CLI: TOML under XDG with a commented default file written on first
    run; flags override config; `NO_COLOR`; `--json` / `--json-stream` /
    `--once` / status-line modes with dual machine+human fields and the
    discard-first-sample rule; inline (non-alt-screen) rendering for small modes;
    two-press confirm for destructive-ish actions; live-preview-then-persist
    theme selection.
13. Tests: `TestBackend` characterization per screen + squeezed-size sweep +
    widget `Buffer` assertions + fixture-JSON parse tests + `#[ignore]`d hardware
    smoke tests.

**Do not take:** everything in §4.

---

## 6. Limitations of this research

1. **`ctux` and `instinct-dash` could not be inspected.** The `wiki/` directory
   referenced by `theme.rs:14`, `sparkline.rs:11`, `protocol.rs:6` and
   `reconnect.rs:5` does not exist in the rocm-cli checkout, and neither project
   is reachable as a public repository from this workspace. Their licenses are
   therefore unverified. This does not block reuse (rocm-cli grants MIT under
   AMD's own copyright), but a formal review may want to confirm the lineage of
   `theme.rs` and `reconnect.rs` before those two are copied.
2. **The rocm-cli checkout is 2 commits behind `origin/main`** (`eef5d7a` vs
   `8cf6ec9`). Nothing read here is likely to have changed, but the exact commit
   used for attribution should be re-pinned at copy time.
3. **No code was executed.** No build, no test run, no `amd-smi` invocation —
   per the research-only scope. All behavioral claims come from reading source
   and its in-tree tests, not from observed runtime behavior.
4. **`bench_load.rs` (1,509 lines), `replay.rs` (814) and `demo.rs` (832) were
   not read in depth.** They were classified as out-of-scope (benchmark load
   generation, session replay, demo data) from their module docs and names; if
   gruflo later wants a record/replay or demo mode, `replay.rs` +
   `core/persist.rs` deserve a dedicated pass.
5. **flow's `theme.go` was read structurally, not exhaustively** — the type, the
   registry, the gradient/ratio functions and the catalogue names were read; the
   ~280 lines of per-theme hex tables were not transcribed.
6. **No legal review.** §1 is an engineering reading of two MIT texts, not
   counsel. The AMD-copyright side in particular may have internal
   redistribution process requirements that this pass cannot see.
7. **Untouched by this ticket:** which metrics `amd-smi` actually exposes per
   ASIC (that is the sibling telemetry-sources research), and whether a
   sysfs/hwmon fallback is viable — rocm-cli's `sysfs.rs` is a stub, so there is
   no prior art to reuse there.

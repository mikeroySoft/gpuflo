# Changelog

Notable changes to GPUFlo are recorded here in [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) style. The Rust library follows Cargo semver; JSON and NDJSON output are versioned separately by `schema_version`. See the [README compatibility policy](README.md#compatibility-policy).

## [0.2.1] - 2026-10-06

### Added

- `theme = "terminal"` / `--theme terminal` follows the terminal's own foreground, background, and ANSI palette (#31).
- Omarchy 4 (Quattro) launch, menu, and status-bar instructions, plus the `scripts/omarchy-bar` wrapper in the repository (#30).

### Fixed

- Discrete RDNA 3/RDNA 4 GPUs no longer report a permanent `thermal throttle active`. Their firmware keeps `TEMP_HOTSPOT` (`indep_throttle_status` bit 36) set even at idle; GPUFlo now ignores that bit in `gpu_metrics` v1.3, matching MangoHud, and decides throttling from `indep_throttle_status` whenever the kernel supplies it. Power, current, and other temperature throttles still report, so health, `daily.json` throttle episodes, and the Omarchy bar `active` class stop flagging idle GPUs.

## [0.2.0] - 2026-10-05

### Added

- `gpus[].platform` in `--json` and every `--json-stream` record: `id` is `strix_halo`, `generic_apu`, `generic_discrete`, or `unknown`; `memory_pool` is `gtt`, `vram`, or `unknown`. This is additive within `schema_version` 1. Platform classifier credit: Matt Elliott (0.2.0, #3); JSON/NDJSON contract tests and docs: #10.
- Public `Platform`, `PlatformId`, and `PhysicalGpu::platform` in the Rust library (#3).
- `--json-stream --for <DURATION>` for bounded captures (#16).
- Source-reported throttle episodes in `daily.json` (#15).
- The `l` key toggles the dashboard logo (#8).
- A `schema_version` compatibility policy in the README (#14).

### Changed

- `l` no longer selects the next GPU. `→` selects the next GPU; `←` or `h` selects the previous GPU (#8).
- **Breaking (library):** `Snapshot`, `PhysicalGpu`, `Partition`, `Memory`, `Health`, `Temperature`, `Power`, and `Platform` are `#[non_exhaustive]`. Obtain them from `Monitor` or by deserializing; struct literals no longer compile outside the crate (#20).
- The daily summary writer refuses records larger than the 64 KiB it can load back, preserving the previous file (#15).
- Strix Halo GTT accounting now uses the platform classifier, with unchanged results (#3).
- Project tooling, not shipped in the crate or binary: AI-factory tooling and factory dashboard themes (#4, #5, #6, #17); project website pages; CI and release runners pinned to `ubuntu-24.04` (#18); README daily-summary disclosure (#19).

### Fixed

- No user-facing bug fixes.

## [0.1.1] - 2026-08-27

### Added

- Fixture-backed Strix Halo (`1002:1586`) GTT accounting; other devices retain their existing KFD classification. Credit: Matt Elliott (0.1.1, #1).
- Opt-in `--cat` / `cat = true` sleeping-cat TUI decoration, kept off instrument surfaces, overlays, tiny mode, and terminals without enough free space.
- Repository GitHub Pages site and animated dashboard presentation.

## [0.1.0] - 2026-08-24

### Added

- First public release of GPUFlo, a read-only Linux `amdgpu` terminal instrument for physical GPUs and their XCP partitions. The dashboard shows activity, memory occupancy, supporting telemetry, and source-backed health.
- `--once`, `--tiny`, `--json`, and `--json-stream` output; JSON and NDJSON use `schema_version` 1 with explicit observation states.
- Interactive responsive views, GPU selection, themes, and process and detail overlays, with terminal restoration on quit and SIGINT.
- Rotating session taglines, selected from 100 static strings once at launch.

### Changed

- Renamed the crate, binary, and documentation from gruflo to gpuflo before the public release.
- Documentation, screenshot, installer script, and release-workflow gate repair. The gate compares the tagged tree with the validated commit outside `validation/`.

[0.2.1]: https://github.com/mikeroysoft/gpuflo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/mikeroysoft/gpuflo/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/mikeroysoft/gpuflo/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/mikeroysoft/gpuflo/releases/tag/v0.1.0

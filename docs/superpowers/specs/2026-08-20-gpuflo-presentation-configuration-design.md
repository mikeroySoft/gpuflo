# Gpuflo Presentation Configuration Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gpuflo permits a few presentation preferences without making metric meaning, responsive safety, or machine output depend on personal configuration. The initial configuration surface contains only theme, preferred responsive mode, and color disablement.

Everything affecting telemetry collection, health semantics, topology, units, history, output schema, or script behavior remains fixed.

## Sparse configuration

The approved sparse TOML file remains:

```text
$XDG_CONFIG_HOME/gpuflo/config.toml
```

with fallback:

```text
~/.config/gpuflo/config.toml
```

The normal file is blank. Defaults live in code, every key is optional, and gpuflo never writes preferences back to the file.

The complete initial TOML key set is:

```toml
theme = "buffalo"
mode = "auto"
no_color = false
```

These lines illustrate available overrides; gpuflo does not generate them. Removing or commenting a key restores its built-in default.

## Precedence

Presentation options resolve once at startup:

```text
built-in default → sparse TOML override → CLI override
```

`NO_COLOR` is an unconditional final color disable rather than a competing preference. Any non-empty `NO_COLOR`, `--no-color`, or `no_color = true` disables color. There is no `--color` option that defeats `NO_COLOR`.

Sources, output formatters, and UI modules receive typed resolved options. They do not reread environment variables or TOML.

## Themes

Three built-in themes ship initially:

| Name | Purpose |
| --- | --- |
| `buffalo` | Approved default: warm cream, rust, amber, and restrained fault red. |
| `nord` | Restrained cool alternative with the same semantic roles. |
| `monochrome` | Neutral palette for terminals and users preferring minimal color. |

A theme maps semantic roles—background, foreground, muted text, accent, warning, fault, borders, and graph intensity—to terminal colors. It cannot change health priority, metric meaning, wording, layout content, or unavailable-state semantics.

Theme names describe semantic palettes, not guaranteed RGB values. Gpuflo uses truecolor when the terminal supports it and degrades to suitable ANSI colors otherwise. Required distinctions remain visible through text, symbols, emphasis, and layout.

No initial support exists for:

- custom theme files or directories;
- arbitrary RGB values in TOML;
- imported terminal palettes;
- user-defined gradients;
- a large theme catalogue; or
- configurable color depth.

The `t` key cycles built-in themes for the current interactive session. It does not modify TOML or create a save prompt.

## Responsive mode preference

The built-in default is:

```toml
mode = "auto"
```

Accepted values are:

- `auto`
- `mode`
- `compact`
- `mini`
- `tiny`

`mode` is the canonical name for the full selected-GPU instrument cluster. No legacy alias is accepted.

`auto` chooses the richest approved surface that fits the current terminal. A forced value is a preference, not permission to clip or overflow. If the preferred surface does not fit, gpuflo selects the largest fitting fallback. A later resize may restore the preferred surface when it fits again.

The `m` key cycles responsive preferences for the current session. It does not persist. Responsive breakpoints and fit calculations remain implementation-owned fixed policy rather than user settings.

## Color disablement

The built-in default is color enabled. Color is disabled when any of these is present:

```text
NO_COLOR=<non-empty value>
--no-color
no_color = true
```

No-color mode removes decorative and semantic foreground/background colors. It may retain non-color terminal emphasis where useful, but:

- selection and focus remain explicit through markers or labels;
- health severity remains explicit in its factual sentence and symbols;
- unavailable observations retain their exact reason in detail/help;
- graphs and instrument values remain understandable without hue; and
- no required distinction depends on color alone.

The `monochrome` theme and no-color mode are distinct. `monochrome` is a restrained palette; no-color removes color styling.

## CLI surface

The visual CLI flags are exactly:

```text
--theme <buffalo|nord|monochrome>
--mode <auto|mode|compact|mini|tiny>
--no-color
```

They apply only to the interactive TUI. Supplying them with a non-interactive output mode is accepted but has no effect on stdout; output remains ANSI-free. Help text states this rather than creating mode-specific flag errors.

The existing output-selection and GPU-selection flags remain CLI-only:

```text
--once
--json
--json-stream
--tiny
--gpu <index|id|bdf>
```

TOML cannot select an output surface, select/filter a GPU, or redirect output. A script therefore behaves the same regardless of the invoking user’s presentation file.

There is no initial `--config` path override. Gpuflo has one documented XDG location, keeping path behavior and support diagnostics predictable.

## Configuration errors

The configuration schema is closed. Gpuflo rejects:

- unknown keys;
- wrong TOML types;
- unknown theme names;
- unknown mode names; and
- malformed TOML.

A configuration error:

1. prints one concise diagnostic to stderr with the file path and line/key when available;
2. emits no stdout payload;
3. occurs before terminal takeover and monitor startup; and
4. exits `2`.

Gpuflo does not silently ignore typos or replace an invalid preference with a default. A missing or blank file remains valid and uses built-in defaults.

## Interactive behavior

Current-session controls are:

- `t` — cycle built-in theme;
- `m` — cycle responsive preference;
- arrow keys — select physical GPU;
- the approved overlay/detail keys; and
- `q`/Escape — quit as defined by the final input map.

Only theme and responsive preference are relevant to this configuration decision. Session changes affect presentation state only. They never alter canonical observations, history, selection identity, exported snapshots, or the user-owned TOML file.

Help may display the active theme, effective responsive surface, and preferred responsive mode when they differ. The main mode does not carry a persistent configuration/status banner.

## Fixed visual semantics

The following remain fixed product behavior rather than settings:

- metric names, meanings, units, and order;
- health categories, priority, wording inputs, and thresholds;
- observation-state vocabulary and absence semantics;
- physical-GPU/XCP scope and selected-GPU overview meaning;
- graph horizon, history capacity, collection cadence, render cadence, and smoothing algorithm;
- spring constants and graph interpolation;
- responsive breakpoints, fit policy, and fallback order;
- process-overlay fields and sorting;
- detail-view field meanings;
- default keyboard actions; and
- session-peak and daily-summary definitions.

Implementation may tune purely visual constants while matching the approved prototype and validation contract. Such tuning does not become a user setting merely because it is represented by a constant.

## Fixed non-interactive behavior

Configuration cannot change:

- `--once`, `--json`, `--json-stream`, or `--tiny` semantic field order;
- JSON fields, nesting, units, tagged observation shape, schema version, or timestamps;
- pretty one-shot JSON versus compact NDJSON framing;
- stream production cadence or sequence behavior;
- all-GPU scope of `--once`, `--json`, and `--json-stream`;
- canonical unavailable phrases;
- ANSI-free output;
- raw rather than smoothed exported observations;
- stdout/stderr separation; or
- exit codes and broken-pipe behavior.

There is no configurable unit system, JSON indentation, field filter, quiet mode, header switch, unavailable-value suppression, aggregate GPU value, or stream interval in the initial design.

## Explicit non-goals

The initial presentation configuration excludes:

- telemetry or performance tuning;
- custom thresholds or health rules;
- configurable metric selection/order;
- custom themes and arbitrary colors;
- persisted interactive changes;
- a configuration editor or save prompt;
- per-output defaults in TOML;
- output filtering or schema customization;
- alternative units;
- configurable key bindings;
- configurable layout breakpoints;
- aliases for obsolete terminology; and
- automatic config migration that rewrites the user’s file.

## Acceptance criteria

The presentation configuration is settled when:

- a blank file preserves the approved default dashboard and output behavior;
- only `theme`, `mode`, and `no_color` are configurable in TOML;
- CLI presentation flags override TOML and `NO_COLOR` always disables color;
- forced modes fall back safely when they do not fit;
- interactive theme/mode cycling remains session-only;
- no-color presentation preserves every required distinction;
- malformed or unknown configuration fails clearly before output or terminal takeover;
- machine and human non-interactive output remain independent of visual configuration; and
- no telemetry, schema, cadence, threshold, unit, or layout-policy knob is exposed.

## Evidence

- [Define sparse configuration behavior](https://github.com/mikeroysoft/gpuflo/issues/13)
- [Prototype the responsive dashboard language](https://github.com/mikeroysoft/gpuflo/issues/7)
- [Define the metric and health contract](./2026-08-20-gpuflo-metric-health-design.md)
- [Define the machine-readable output contract](./2026-08-20-gpuflo-machine-readable-output-design.md)
- [Define capability, failure, and permission behavior](./2026-08-20-gpuflo-capability-failure-design.md)
- [Choose the minimal Rust architecture](./2026-08-20-gpuflo-rust-architecture-design.md)
- [Define the validation and release contract](./2026-08-20-gpuflo-validation-release-design.md)

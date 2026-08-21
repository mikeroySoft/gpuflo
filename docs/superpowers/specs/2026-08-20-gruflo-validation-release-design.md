# Gruflo Validation and Release Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gruflo is a small local instrument, not a testing platform. Its release evidence must prove the few failures that would make the product dishonest or unsafe: incorrect telemetry meaning, unavailable values becoming numbers, broken physical-GPU/XCP scope, machine-output incompatibility, failure to answer within one second, terminal corruption, or material workload perturbation.

Validation is risk-based and deliberately small. It does not attempt every hardware, driver, terminal, or failure combination. Deterministic fixture validation and live hardware qualification are distinct claims.

## Release claims

Every published release passes one deterministic release gate. Hardware support is reported separately by telemetry regime:

- **qualified** — the release candidate passed the live contract on representative physical hardware;
- **fixture-validated** — implemented interfaces passed committed fixtures, but this release candidate was not run on representative physical hardware; or
- **unverified** — neither current live evidence nor adequate fixtures support a claim.

The four telemetry regimes are:

1. CDNA 1/2 without XCP partitioning;
2. CDNA 3/4, including an XCP-partitioned configuration;
3. discrete RDNA; and
4. Ryzen APU shared/GTT-memory behavior.

These are validation groups, not model allowlists. Gruflo still discovers capability from current interfaces rather than GPU names.

A release may contain fixture-validated regimes, but release notes must not describe them as hardware-qualified. A known regression on a previously qualified regime blocks release until fixed or truthfully downgraded with the affected behavior removed from the supported claim. The first public release requires at least one qualified `amdgpu` host.

## Minimal deterministic release gate

The gate contains only tests that defend an approved observable contract.

### Source fixtures

Commit small, provenance-labelled fixtures for each source layout the implementation actually parses. Fixtures cover:

- each implemented fixed or dynamic `gpu_metrics` layout;
- representative sysfs/hwmon topology and sensor values;
- SPX and multi-XCP scope when those parsers are implemented;
- discrete VRAM and APU shared/GTT pool shapes;
- implemented RAS/throttle/violation sentinels; and
- implemented KFD/fdinfo process records.

For each parser, add boundary fixtures only where behavior differs: valid, documented unsupported sentinel, truncated/malformed, and unknown version or shape. Do not manufacture a combinatorial fixture matrix.

Every fixture records whether it was captured from hardware, reduced from an upstream public source, or synthesized for a named boundary. Captured data is sanitized of usernames, command lines, container identifiers, serials, UUIDs, and host identifiers.

### Load-bearing semantic tests

Keep focused tests for these invariants:

1. every unavailable reading remains its exact observation state and never becomes numeric zero;
2. retained, stale, or failed readings do not enter history, peaks, thresholds, rates, or daily summaries;
3. socket observations remain on the physical GPU and XCP observations remain on their partition, without duplication or synthetic aggregation;
4. kernel observations retain precedence over optional AMD SMI enrichment;
5. health selects the highest-priority active source-backed condition; and
6. a failure in one metric, source, or GPU preserves independent current observations.

Use deterministic clock and source seams already required by the architecture. Prefer table tests for named transitions. Do not add a model-testing framework, broad property suite, or fuzz target unless a concrete parser defect later justifies one.

### Public monitor journey

One integration journey exercises the supported `Monitor` interface with fake lanes and deterministic clocks:

- startup and priming;
- receipt of an owned snapshot;
- priority notice/fatal delivery, including fatal replacement of a pending notice;
- a visible snapshot sequence gap under a slow receiver;
- one command; and
- bounded shutdown.

Private lane combinations do not each need an end-to-end test when their source and reducer contracts are already covered.

### Output contract

Machine-output tests decode output and assert semantics rather than incidental formatting:

- required schema-version-1 fields and tagged observation forms;
- nested physical-GPU/XCP scope;
- canonical units and source timestamps;
- no JSON `null`, non-finite number, stale numeric value, or unavailable numeric zero;
- compact one-object-per-line NDJSON framing;
- unknown additive fields and unknown string values remain consumable; and
- broken pipe, usage error, SIGINT, and fatal-runtime exits follow the approved codes.

Retain the normative specification examples as readable fixtures. Do not freeze JSON key order or pretty-print whitespace beyond the required final newline and NDJSON record boundaries. Human text tests assert semantic field order and canonical unavailable phrases, not exact spacing.

### Responsive rendering

Use Ratatui `TestBackend` for:

- one representative frame for each responsive surface;
- one bounded width/height sweep around actual layout breakpoints, asserting no panic or overflow; and
- direct buffer assertions only for custom widgets whose cell behavior carries meaning, such as observation gaps or braille packing.

Representative terminal sizes are `120×40`, `80×24`, `60×16`, `40×8`, and `20×1`. The breakpoint sweep replaces an exhaustive terminal-size matrix.

### Terminal restoration

Drive the compiled interactive binary through a pseudoterminal for exactly three lifecycle journeys:

1. normal quit;
2. one catchable signal exit; and
3. one injected fatal exit after terminal acquisition.

Each journey verifies cursor restoration, raw-mode exit, alternate-screen exit, and diagnostic ordering where applicable. Unit tests cover staged partial acquisition. SIGKILL and process abort remain inherently outside the restoration contract. Separate pseudoterminal journeys for every signal or collector failure are unnecessary.

## Accessibility and no-color behavior

`NO_COLOR` and `--no-color` disable decorative and semantic color. Selection, focus, health severity, and unavailable states remain understandable through labels, symbols, and layout. No required meaning may depend on color alone.

Human text and machine-readable modes never emit ANSI escapes. The release gate renders representative TUI frames with color disabled and checks the text/symbol distinctions. Gruflo makes no WCAG conformance claim for terminal emulators it does not control.

## Live hardware qualification

Qualification runs the release candidate, not a debug-only substitute, on a representative host for the claimed regime. Record:

- gruflo version and commit;
- Rust compiler and target;
- kernel, amdgpu, ROCm, and AMD SMI versions when present;
- GPU identity, observed `gpu_metrics` layout, memory pool, and partition mode;
- permissions used;
- checks performed, results, and justified skips; and
- measured startup, collection, resource, and perturbation results.

A qualification run checks:

1. discovery and topology match the host;
2. current observations agree in unit and scope with their kernel files and, when available, AMD SMI as a non-authoritative cross-check;
3. unavailable and permission-limited values retain their reasons;
4. `--once`, `--json`, `--json-stream`, `--tiny`, and the TUI start and remain coherent;
5. runtime sleep or an equivalent safely available failure does not become zero;
6. terminal restoration succeeds; and
7. the performance budgets below hold.

Hot-unplug, partition-mode mutation, RAS fault injection, and destructive/admin operations are not release prerequisites. Those behaviors use deterministic seams unless a safe lab procedure independently provides evidence.

Process-attribution qualification remains owned by **Verify process attribution on a live ROCm workload**. Until that work passes, release evidence must not imply live qualification of process association or resident-memory attribution merely because the overlay passed fixtures.

## Performance budgets

The one-second answer is the hard product budget. On each qualified host:

- process start to the first complete TUI frame or one-shot output is at most one second;
- the second priming sample remains part of that interval;
- optional AMD SMI initialization or sampling cannot delay the first kernel-backed result; and
- no source operation overlaps its next scheduled operation.

For a 60-second qualification run, record fast kernel collection latency and missed production ticks. The p95 fast collection latency must remain below 125 ms, leaving half of the 250 ms cadence for coordination and variance. Any operation reaching its 250 ms cadence is a failure regardless of percentile.

Measure one representative stable GPU workload with and without gruflo using the same command and conditions. A repeatable throughput regression greater than 2% blocks qualification. Record CPU and resident memory for regression tracking, but do not impose arbitrary fixed limits before measured implementation evidence exists.

These are qualification checks, not a benchmark framework. Twenty startup samples and three matched workload samples are sufficient unless results are noisy near a limit.

## Compatibility checks

The implementation selects and documents a minimum supported Rust version after dependency versions are fixed. CI then tests that compiler and current stable Rust.

The release gate builds and tests `x86_64-unknown-linux-gnu`. It compile-checks `aarch64-unknown-linux-gnu` when the selected CI runner/toolchain can do so without target-specific stubs. A target becomes supported only after a live qualification; cross-compilation alone is not qualification.

A musl target is not promised until packaging demonstrates that the binary, optional AMD SMI loading, and host integration work together. Native Windows, WSL-specific behavior, macOS, and non-amdgpu backends remain out of scope.

Machine schema compatibility follows its existing major-version rule. Public Rust model and `Monitor` changes receive a semver review because they are the supported reuse seam. Private modules and exact human-readable spacing carry no compatibility promise.

## Packaging prerequisites

Before a packaging channel may publish a release candidate, it must provide:

- the one `gruflo` binary produced from the tagged source;
- the project license and required third-party notices;
- reproducible build instructions and a locked dependency graph;
- version output that identifies the release;
- no mandatory ROCm, AMD SMI, daemon, network, or root dependency;
- a clean install/uninstall path that does not mutate GPU or permission configuration; and
- a way to verify the artifact checksum.

The separate packaging decision chooses channels, archive/package formats, signing, and whether the library is published. This contract does not preselect them.

## Release evidence

Attach or commit one concise validation manifest per release candidate. It contains:

- release version, commit, target, and compiler;
- deterministic gate result;
- qualification state for each telemetry regime;
- one entry per live host with the recorded environment and measured budgets;
- skipped checks with reasons;
- known limitations or downgraded qualification claims; and
- artifact checksums or links supplied by the packaging channel.

CI logs and benchmark output may support the manifest but do not replace its concise claims. The manifest can be Markdown or another existing repository-native format; do not create a custom validation service, database, dashboard, schema, or results framework.

## Release blocking policy

A release is blocked by:

- failure of a deterministic gate;
- first output exceeding one second on hardware claimed as qualified;
- material telemetry-scope or observation-state disagreement;
- terminal restoration failure on a catchable path;
- machine-schema incompatibility without a major-version change;
- repeatable workload perturbation above the qualification budget;
- a known regression on a previously qualified regime that remains claimed; or
- missing license/notices/checksum evidence for the chosen artifact.

An unavailable hardware lab does not block an otherwise valid release, except that the first public release still needs one qualified host. The affected regimes remain fixture-validated or unverified. A skipped deterministic check is not a pass. Flaky checks must be fixed, narrowed to their real contract, or removed with an explicit contract change; repeated retries are not release evidence.

## Explicit non-goals

This contract does not require:

- exhaustive model, kernel, ROCm, terminal, locale, or permission combinations;
- one test per module, type, worker lane, signal, or observation state;
- broad snapshot/golden testing of incidental UI or formatting;
- a custom test framework, schema service, benchmark harness, dashboard, or hardware farm;
- destructive fault injection, partition mutation, tuning, reset, fan, or power-cap operations;
- a durable telemetry/results database;
- code-coverage percentage targets; or
- hardware qualification claims unsupported by a recorded physical run.

## Acceptance criteria

The contract is satisfied when:

- every release passes the small deterministic gate;
- release claims distinguish qualified, fixture-validated, and unverified regimes;
- the first public release has at least one qualified physical `amdgpu` host;
- load-bearing observation, topology, output, and terminal contracts have direct evidence;
- first useful output arrives within one second on qualified hosts;
- gruflo does not materially perturb a representative workload;
- no-color operation preserves meaning;
- packaging can consume explicit license, build, version, and checksum prerequisites; and
- validation remains proportionate to a small read-only tool.

## Evidence

- [Inventory AMD telemetry sources and support](https://github.com/michaelroy-amd/gruflo/issues/8)
- [AMD telemetry source research](https://github.com/michaelroy-amd/gruflo/blob/research/amd-telemetry-sources/research/amd-telemetry-sources.md)
- [Identify the reusable code boundary](https://github.com/michaelroy-amd/gruflo/issues/2)
- [Reuse boundary research](https://github.com/michaelroy-amd/gruflo/blob/research/reuse-boundary/research/reuse-boundary.md)
- [Define the metric and health contract](./2026-08-20-gruflo-metric-health-design.md)
- [Define the machine-readable output contract](./2026-08-20-gruflo-machine-readable-output-design.md)
- [Define capability, failure, and permission behavior](./2026-08-20-gruflo-capability-failure-design.md)
- [Choose the minimal Rust architecture](./2026-08-20-gruflo-rust-architecture-design.md)

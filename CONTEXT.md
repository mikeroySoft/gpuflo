# Gruflo Domain Glossary

## Observation state

The reason an observation has no current numeric value. Canonical states are `unsupported_hardware`, `unsupported_driver_version`, `permission_denied`, `asleep`, `reported_by_primary_partition`, `stale`, and `source_error`.

## Detail view

The secondary diagnostic surface containing supported telemetry that is not required for the one-second scan.

## GPU activity

The device-level GFX activity reported by the selected telemetry source. It is not per-process utilization and does not imply a particular application phase.

## Health condition

A source-backed limit, throttle, fault, degradation, or telemetry-availability condition expressed as a factual sentence. It is not a composite score.

## Hardware qualification

Evidence from running the release candidate on a representative physical AMD GPU telemetry regime. It is stronger than fixture validation and may be `qualified`, `fixture-validated`, or `unverified` for each regime.

## Mode

The full always-visible instrument cluster for one selected physical GPU: activity, memory occupancy, core supporting instruments, and the highest-priority health condition.

## Memory occupancy

Used capacity relative to the applicable GPU memory pool. The pool is dedicated VRAM on a discrete GPU and explicitly labelled shared or GTT memory on an APU.

## Memory pressure

An allocation-pressure or allocation-failure condition explicitly reported by a telemetry source. High occupancy alone is not pressure and is not unhealthy.

## Observation

A metric at one hardware scope, represented either by a numeric value with its source observation time or by an observation state. A stale observation retains the last good observation time but not its numeric value.

## Physical GPU

One physical AMD GPU package or socket. It owns socket-scoped power and temperature observations and may contain multiple XCPs.

## Primary partition

The XCP whose device-wide telemetry source reports socket-scoped observations for a physical GPU. This reporting role does not make socket power or temperature XCP-scoped.

## Process overlay

The secondary surface for honestly attributable process identity, GPU association, and memory use. It does not claim per-process GPU-utilization percentages.

## Snapshot

One exportable view of every discovered physical GPU at a stated assembly time. Its observations retain their own source times and may therefore come from different collection cadences.

## Structural capability

Whether the current hardware, driver interface, and topology can report a metric at all. It is distinct from a temporary runtime failure of an otherwise supported source.

## Supported host

A Linux host with at least one AMD PCI/DRM device bound to the `amdgpu` driver. ROCm userspace, AMD SMI, `/dev/kfd`, supplemental groups, and root privileges are not part of this minimum.

## XCP

A logical accelerator partition within a physical GPU. XCP-scoped observations belong to the partition; socket-scoped observations remain owned by the physical GPU.

# Gruflo Domain Glossary

## Capability state

The reason a metric has no current numeric value. Canonical states are unsupported hardware, unsupported driver version, permission denied, asleep, reported by the primary partition, and stale.

## Detail view

The secondary diagnostic surface containing supported telemetry that is not required for the one-second scan.

## GPU activity

The device-level GFX activity reported by the selected telemetry source. It is not per-process utilization and does not imply a particular application phase.

## Health condition

A source-backed limit, throttle, fault, degradation, or telemetry-availability condition expressed as a factual sentence. It is not a composite score.

## Hero view

The minimal always-visible instrument cluster for one selected physical GPU: activity, memory occupancy, core supporting instruments, and the highest-priority health condition.

## Memory occupancy

Used capacity relative to the applicable GPU memory pool. The pool is dedicated VRAM on a discrete GPU and explicitly labelled shared or GTT memory on an APU.

## Memory pressure

An allocation-pressure or allocation-failure condition explicitly reported by a telemetry source. High occupancy alone is not pressure and is not unhealthy.

## Physical GPU

One physical AMD GPU package or socket. It owns socket-scoped power and temperature observations and may contain multiple XCPs.

## Process overlay

The secondary surface for honestly attributable process identity, GPU association, and memory use. It does not claim per-process GPU-utilization percentages.

## XCP

A logical accelerator partition within a physical GPU. XCP-scoped observations belong to the partition; socket-scoped observations remain owned by the physical GPU.

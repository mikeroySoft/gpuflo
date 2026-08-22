# Process Attribution Capture Harness Design

**Status:** Approved in discussion on 2026-08-20

## Purpose

Provide one read-only command that a user can run after cloning gpuflo onto a representative Linux ROCm host. The harness gathers sanitized evidence for the Wayfinder task **Verify process attribution on a live ROCm workload** without requiring manual PID discovery, source inspection, timing loops, or result assembly.

This is temporary research tooling, not production gpuflo code.

## Invocation

From the repository root:

```bash
./research/process-attribution/capture.sh
```

No arguments, root access, configuration, or interactive prompts are required.

## Workload selection

The harness selects the first available workload path:

1. ROCm-enabled PyTorch through `python3`; or
2. an embedded HIP workload compiled with `hipcc` in a temporary directory.

It verifies that the selected workload can see an AMD GPU before collecting evidence. If neither path is usable, it exits without partial claims and prints one actionable diagnostic naming the missing prerequisite.

The workload runs long enough to allocate device memory, execute continuous compute, expose process accounting, and produce a stable throughput measurement. Temporary source and binaries are removed on exit.

## Evidence collection

For the harness-owned workload PID, collect only:

- kernel, distribution, and relevant tool versions;
- current user group names and device-node permission modes without username;
- DRM render-node to PCI BDF mapping;
- matching KFD process files and nested statistics;
- matching `/proc/<pid>/fdinfo` DRM/AMD/PASID fields;
- two samples separated by a fixed interval to identify advancing counters;
- device-memory values needed to compare attribution sources;
- repeated complete process-scan latency; and
- workload throughput with and without a two-second process scan.

The harness records absent, unreadable, permission-denied, and malformed sources distinctly. It never converts absence to zero.

## Safety and privacy

The harness:

- runs as the invoking unprivileged user;
- performs no GPU tuning, reset, fan, power-cap, partition, driver, group, udev, or permission changes;
- never invokes `sudo`;
- does not read unrelated processes’ command lines or environment;
- limits detailed fdinfo/KFD capture to the workload it launched;
- omits hostname, username, home path, serial numbers, UUIDs, container identifiers, and full command lines;
- uses a temporary directory with owner-only permissions;
- terminates its workload and removes temporary files on normal exit and catchable signals; and
- writes results only beneath `research/process-attribution/results/`.


The latency and perturbation passes inspect readable DRM field names across the process list because that is the production discovery cost being measured, but discard every unrelated value immediately. They never read command lines or environments, and only the harness workload's detailed fdinfo/KFD values enter the result bundle.
PCI BDF, GPU model, kernel/driver versions, KFD GPU IDs, and source field names remain because they are necessary technical evidence.

## Result shape

One timestamped result directory contains plain text and TSV files:

```text
research/process-attribution/results/<utc-timestamp>/
├── summary.txt
├── environment.txt
├── drm-mapping.tsv
├── permissions.tsv
├── fdinfo-before.txt
├── fdinfo-after.txt
├── fdinfo-diff.txt
├── kfd-before.txt
├── kfd-after.txt
├── kfd-diff.txt
├── scan-timing.tsv
├── workload-baseline.tsv
├── workload-polled.tsv
└── manifest.sha256
```

A neighboring `.tar.gz` contains the same directory for transfer. `summary.txt` gives factual results only: workload path, observed GPU/BDF, association evidence, resident-memory fields, advancing engine fields, KFD occupancy availability, permission outcomes, scan latency, and measured perturbation. It marks unsupported conclusions as `not observed` or `not tested`.

The result is deterministic enough to review with ordinary text tools and contains no custom schema, upload client, database, or dashboard.

## Error behavior

Prerequisite failure occurs before creating a final result directory. Collection failures are recorded per source while independent collection continues. The harness returns nonzero when:

- the platform is not Linux;
- no `amdgpu` DRM device exists;
- no supported workload path can execute;
- the launched workload exits before evidence collection; or
- no fdinfo or KFD association evidence can be captured.

A failed perturbation budget is a successful capture with a failing result in `summary.txt`; the evidence must remain available.

## Local verification

Development-host verification does not require a GPU:

- `bash -n` validates syntax;
- a no-hardware run exits nonzero with the expected prerequisite diagnostic;
- shell cleanup leaves no workload or temporary directory; and
- a focused source review confirms that every GPU and system operation is read-only.

Live validity is established only by running the committed harness on the remote ROCm host.

## Non-goals

The harness does not:

- implement any gpuflo production module;
- install Python, PyTorch, ROCm, compilers, or packages;
- accept arbitrary workload commands;
- inspect every user’s process data;
- test cross-user traversal that requires a second cooperating account;
- mutate the host to manufacture permission failures;
- claim per-process utilization percentages;
- upload results; or
- close the Wayfinder task before the returned evidence is reviewed.

## Acceptance criteria

- A user with a cloned repository runs one command without arguments.
- The harness automatically runs ROCm PyTorch or falls back to HIP.
- All collection is local, read-only, unprivileged, bounded, and cleaned up.
- The result distinguishes association, resident memory, engine-time counters, KFD occupancy, permissions, latency, and perturbation.
- Captured files are sanitized and ready to commit or attach to the issue.
- A host lacking prerequisites receives one actionable failure instead of a misleading partial report.

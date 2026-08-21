# Process Attribution Capture Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one zero-argument, read-only command that runs a representative ROCm workload and emits sanitized process-attribution evidence.

**Architecture:** A single Bash orchestrator owns prerequisite checks, temporary files, workload selection, source capture, timing, summarization, cleanup, and result publication. It embeds one PyTorch workload and one HIP fallback in its private temporary directory; no production gruflo module or runtime dependency is introduced.

**Tech Stack:** Bash, Linux procfs/sysfs, ROCm PyTorch or `hipcc`, standard POSIX/GNU utilities.

---

### Task 1: Implement the capture harness

**Files:**
- Create: `research/process-attribution/capture.sh`

- [ ] **Step 1: Add strict startup and cleanup**

Use `set -Eeuo pipefail`, resolve the repository root from the script path, reject non-Linux/no-amdgpu hosts, create an owner-only temporary directory, and install EXIT/INT/TERM/HUP cleanup that terminates the harness-owned workload.

- [ ] **Step 2: Add automatic workloads**

Embed a Python program that verifies `torch.version.hip` and `torch.cuda.is_available()`, allocates matrices, signals readiness, runs `torch.mm`, synchronizes, and prints iterations per second. Embed a HIP fallback that performs the same lifecycle with `hipMalloc`, a simple kernel, events/synchronization, and iterations-per-second output. Select PyTorch first, then `hipcc`, otherwise fail with one prerequisite diagnostic.

- [ ] **Step 3: Add bounded evidence collection**

Capture sanitized environment/device metadata, render-node/BDF mappings, the workload PID's relevant fdinfo fields, matching KFD files, before/after diffs, ten complete process-scan timings, and three baseline versus three polled throughput samples. Never invoke `sudo` or write under `/sys`, `/proc`, or `/dev`.

- [ ] **Step 4: Add summary and artifact publication**

Derive advancing engine fields, association evidence, occupancy availability, scan latency, and throughput delta into `summary.txt`. Write a SHA-256 manifest, atomically move the complete directory under `research/process-attribution/results/<UTC timestamp>/`, and create the neighboring `.tar.gz`.

### Task 2: Verify local behavior

**Files:**
- Test: `research/process-attribution/capture.sh`

- [ ] **Step 1: Check syntax**

Run:

```bash
bash -n research/process-attribution/capture.sh
```

Expected: exit `0`, no output.

- [ ] **Step 2: Check no-hardware failure**

Run on the current WSL development host:

```bash
research/process-attribution/capture.sh
```

Expected: nonzero exit with one `no AMD GPU bound to amdgpu` diagnostic; no final result directory, surviving workload, or temporary directory.

- [ ] **Step 3: Inspect read-only operations**

Review every command that addresses `/sys`, `/proc`, and `/dev`. Expected: reads/stat/realpath only; no writes, permission changes, module operations, `sudo`, or GPU mutation command.

### Task 3: Review and publish

**Files:**
- Create: `research/process-attribution/capture.sh`
- Existing: `docs/superpowers/specs/2026-08-20-process-attribution-capture-harness-design.md`

- [ ] **Step 1: Run shell diagnostics**

Run `bash -n`, `git diff --check`, and the no-hardware smoke path. Fix any reported issue.

- [ ] **Step 2: Review failure cleanup and privacy**

Confirm traps kill only the recorded workload PID/process group, temporary paths are mode 0700, detailed collection is limited to that PID, and result files omit hostname, username, home path, serials, UUIDs, command lines, and unrelated process contents.

- [ ] **Step 3: Commit and push**

```bash
git add research/process-attribution/capture.sh docs/superpowers/plans/2026-08-20-process-attribution-capture-harness.md
git commit -m "research: add process attribution capture harness"
git push origin main
```

Expected: `main` and `origin/main` point to the new commit; existing untracked `HANDOFF.md` remains untouched.

# Gpuflo Production Implementation Prompt

```text
Continue gpuflo from the completed Wayfinder effort and build the production
implementation end to end.

Repository: https://github.com/mikeroysoft/gpuflo.git
Baseline: main at or after c761200
Authoritative handoff:
docs/superpowers/handoffs/2026-08-21-gpuflo-production-implementation.md

First read the entire handoff, CONTEXT.md, every canonical specification it
lists, the live process-attribution result, and the approved Ratatui prototype
at commit 699a425. The closed Wayfinder map is context only; do not reopen
settled product decisions.

Then create an isolated feature branch/worktree, write a production
implementation plan under docs/superpowers/plans/, and execute that plan fully.
Do not stop after planning. Build the single-package Rust library+binary
architecture exactly as specified, implement kernel-first telemetry with
optional runtime AMD SMI, the canonical observation/reducer model, bounded
monitor lanes, output modes, sparse configuration, persistence, terminal
restoration, process overlay, and the approved responsive Ratatui UI. Keep
gpuflo strictly local and read-only. Preserve explicit unavailable states and
physical-GPU/XCP scope. Use mode for the full instrument cluster.

Follow the minimal validation contract rather than inventing a huge test
matrix. Run the actual binary and PTY/TUI paths, perform required code review,
fix findings, and continue until every definition-of-done item in the handoff
is satisfied. Commit coherent increments, push the branch, and produce a
ready-to-merge PR with exact build/test/smoke evidence.

Ask me only for a genuine unresolved contradiction or an external
credential/hardware prerequisite; otherwise make the conservative boring
decision and keep executing.
```

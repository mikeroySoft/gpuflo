#!/usr/bin/env python3

"""Stateless AI-factory dispatcher.

One pass per invocation: pick claimable issues (or --ticket N), run a worker
agent in a git worktree, gate, open a PR, review with codex, bounce once.
All state lives in GitHub and .factory/ on disk.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def origin_repo() -> str:
    url = subprocess.run(
        ["git", "-C", str(ROOT), "remote", "get-url", "origin"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    return url.rsplit("github.com", 1)[-1].strip(":/").removesuffix(".git")


REPO = origin_repo()
FACTORY = ROOT / ".factory"
LOGS = FACTORY / "logs"
GATE = Path(__file__).resolve().parent / "agent_gate.py"
MAX_ACTIVE = 2
MAX_ATTEMPTS = 3

STANDING_INSTRUCTIONS = """
## Instructions

- Implement exactly what the ticket above asks for; nothing more.
- Use TDD where practical: failing test first, then the fix.
- Commit incrementally with `git commit -s`. Stage only files you created or
  edited for the ticket; never `git add -A`, and never commit
  `.factory-prompt.md`, `.lock`, or gate reports.
- NEVER use `git stash` — the stash is shared with the user's other worktrees.
- Finish by running `python {gate} --report .factory/gate-report-{n}.md`
  and fixing any failures it reports.
"""


def log(msg: str) -> None:
    print(f"[dispatch] {msg}", flush=True)


def run(cmd: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=cwd, check=check, capture_output=True, text=True)


def gh_json(args: list[str]) -> object:
    out = run(["gh", *args]).stdout
    return json.loads(out)


def lock_held(lockfile: Path) -> bool:
    """True if another process holds an flock on lockfile."""
    if not lockfile.exists():
        return False
    with lockfile.open("r") as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(f, fcntl.LOCK_UN)
            return False
        except OSError:
            return True


def active_ticket_count() -> int:
    if not FACTORY.is_dir():
        return 0
    return sum(1 for d in FACTORY.glob("wt-*") if d.is_dir() and lock_held(d / ".lock"))


def issue_is_open(number: int) -> bool:
    data = gh_json(["issue", "view", str(number), "--repo", REPO, "--json", "state"])
    return data["state"].upper() == "OPEN"


def open_blockers(number: int, body: str) -> list[int]:
    blockers: set[int] = set()
    # GitHub issue-dependency API; 404 means the feature/edges are absent.
    proc = run(["gh", "api", f"repos/{REPO}/issues/{number}/dependencies/blocked_by"], check=False)
    if proc.returncode == 0:
        for dep in json.loads(proc.stdout):
            if dep.get("state", "").lower() == "open":
                blockers.add(dep["number"])
    # "Blocked by: #n" lines in the body.
    for line in re.findall(r"(?im)^blocked by:(.*)$", body or ""):
        for ref in re.findall(r"#(\d+)", line):
            n = int(ref)
            if n not in blockers and issue_is_open(n):
                blockers.add(n)
    return sorted(blockers)


def frontier() -> list[dict]:
    issues = gh_json([
        "issue", "list", "--repo", REPO, "--state", "open",
        "--label", "ready-for-agent",
        "--json", "number,title,body,labels,assignees",
    ])
    ready = []
    for issue in issues:
        n = issue["number"]
        if issue["assignees"]:
            log(f"#{n}: skipped (assigned)")
            continue
        blockers = open_blockers(n, issue.get("body", ""))
        if blockers:
            log(f"#{n}: skipped (blocked by {', '.join(f'#{b}' for b in blockers)})")
            continue
        ready.append(issue)
    return ready


def build_prompt(n: int, extra: str = "") -> str:
    issue = gh_json(["issue", "view", str(n), "--repo", REPO,
                     "--json", "title,body,comments"])
    parts = [f"# Issue #{n}: {issue['title']}", "", issue.get("body") or "(no body)"]
    for c in issue.get("comments") or []:
        author = (c.get("author") or {}).get("login", "unknown")
        parts += ["", f"## Comment by {author}", "", c.get("body", "")]
    parts.append(STANDING_INSTRUCTIONS.format(n=n, gate=GATE))
    if extra:
        parts += ["", extra]
    return "\n".join(parts) + "\n"


def worker_cmd(labels: set[str], promptfile: Path, wt: Path) -> list[str]:
    if "chore" in labels:
        return ["droid", "exec", "-f", str(promptfile), "--auto", "medium", "--cwd", str(wt)]
    return ["omp", "-p", "--cwd", str(wt), f"@{promptfile}"]


def run_worker(cmd: list[str], wt: Path, logfile: Path) -> int:
    log(f"worker: {' '.join(cmd)} -> {logfile}")
    with logfile.open("a") as out:
        return subprocess.run(cmd, cwd=wt, stdout=out, stderr=subprocess.STDOUT).returncode


def ensure_worktree(n: int) -> Path:
    wt = FACTORY / f"wt-{n}"
    if wt.is_dir():
        return wt
    run(["git", "fetch", "origin"], cwd=ROOT)
    branch = f"agent/{n}"
    exists = run(["git", "rev-parse", "--verify", branch], cwd=ROOT, check=False).returncode == 0
    if exists:
        run(["git", "worktree", "add", str(wt), branch], cwd=ROOT)
    else:
        run(["git", "worktree", "add", str(wt), "-b", branch, "origin/main"], cwd=ROOT)
    return wt


def commit_leftovers(wt: Path, n: int, title: str) -> None:
    if run(["git", "status", "--porcelain"], cwd=wt).stdout.strip():
        run(["git", "add", "-A", "--",
             ":(exclude).factory-prompt.md", ":(exclude).lock",
             ":(exclude).factory"], cwd=wt)
        run(["git", "commit", "-s", "-m", f"agent/{n}: {title}"], cwd=wt)


def run_gate(wt: Path, n: int) -> tuple[bool, str]:
    report_rel = f".factory/gate-report-{n}.md"
    (wt / ".factory").mkdir(exist_ok=True)
    # GPU serialization is the gate's job: agent_gate.py flocks the GPU lock
    # itself. Locking here too deadlocks the gate subprocess (seen in run #7).
    proc = subprocess.run(
        [sys.executable, str(GATE), "--base", "origin/main",
         "--report", report_rel],
        cwd=wt, capture_output=True, text=True)
    report = wt / report_rel
    text = report.read_text() if report.exists() else proc.stdout + proc.stderr
    return proc.returncode == 0, text


def escalate(n: int, reason: str, log_path: Path | None) -> None:
    log(f"#{n}: escalating to human ({reason})")
    run(["gh", "issue", "edit", str(n), "--repo", REPO,
         "--remove-assignee", "@me", "--add-label", "ready-for-human"], check=False)
    body = f"Factory dispatcher escalating: {reason}."
    if log_path:
        body += f"\n\nWorker logs: `{log_path}`"
    run(["gh", "issue", "comment", str(n), "--repo", REPO, "--body", body], check=False)


def review(wt: Path, n: int) -> tuple[str, str]:
    """Run codex two-axis review. Returns (verdict, findings markdown)."""
    prompt = (
        f"Review `git diff origin/main..HEAD` in this repository on two axes:\n"
        f"1. Standards: does the code follow this repo's documented conventions "
        f"(AGENTS.md, docs/)?\n"
        f"2. Spec: does the diff satisfy the text and acceptance criteria of "
        f"GitHub issue #{n} in {REPO}?\n"
        f"Output findings as markdown. End with exactly one line: "
        f"`VERDICT: APPROVE` or `VERDICT: REVISE`."
    )
    proc = subprocess.run(["codex", "exec", prompt], cwd=wt, capture_output=True, text=True)
    findings = proc.stdout.strip() or proc.stderr.strip()
    m = re.search(r"VERDICT:\s*(APPROVE|REVISE)", findings)
    verdict = m.group(1) if m else "REVISE"
    return verdict, findings


def push_and_pr(wt: Path, n: int, title: str, gate_report: str) -> None:
    run(["git", "push", "-u", "origin", f"agent/{n}"], cwd=wt)
    existing = gh_json(["pr", "list", "--repo", REPO, "--head", f"agent/{n}",
                        "--json", "number"])
    if existing:
        log(f"#{n}: PR already exists (#{existing[0]['number']})")
        return
    body_file = FACTORY / f"pr-body-{n}.md"
    body_file.write_text(f"Closes #{n}\n\n## Gate report\n\n{gate_report}\n")
    run(["gh", "pr", "create", "--repo", REPO, "--head", f"agent/{n}",
         "--title", f"agent/{n}: {title}", "--body-file", str(body_file)])


def pr_comment(n: int, text: str) -> None:
    body_file = FACTORY / f"review-{n}.md"
    body_file.write_text(text + "\n")
    run(["gh", "pr", "comment", f"agent/{n}", "--repo", REPO,
         "--body-file", str(body_file)], check=False)


def worker_round(n: int, wt: Path, labels: set[str], title: str, extra: str,
                 attempt: int, deadline: float) -> tuple[bool, str, Path]:
    """One worker + gate cycle. Returns (gate_ok, report, logfile)."""
    promptfile = wt / ".factory-prompt.md"
    promptfile.write_text(build_prompt(n, extra))
    logfile = LOGS / f"{n}-attempt-{attempt}.log"
    run_worker(worker_cmd(labels, promptfile, wt), wt, logfile)
    commit_leftovers(wt, n, title)
    if time.monotonic() > deadline:
        return False, "budget exceeded before gate", logfile
    ok, report = run_gate(wt, n)
    return ok, report, logfile


def process_ticket(issue: dict, budget_min: int, dry_run: bool) -> None:
    n, title = issue["number"], issue["title"]
    labels = {l["name"] for l in issue.get("labels", [])}
    wt = FACTORY / f"wt-{n}"
    worker = "droid" if "chore" in labels else "omp"

    if lock_held(wt / ".lock"):
        log(f"#{n}: skipped (in flight, lock held on {wt / '.lock'})")
        return
    if dry_run:
        log(f"#{n}: would claim (assign @me), create worktree {wt} on branch agent/{n}, "
            f"run {worker} worker, gate, push, open PR, codex-review")
        return

    deadline = time.monotonic() + budget_min * 60
    run(["gh", "issue", "edit", str(n), "--repo", REPO, "--add-assignee", "@me"])
    wt = ensure_worktree(n)
    LOGS.mkdir(parents=True, exist_ok=True)

    lock_fd = (wt / ".lock").open("w")  # held for the life of this pipeline
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        log(f"#{n}: skipped (lost lock race)")
        lock_fd.close()
        return

    try:
        # Attempts 1..MAX_ATTEMPTS: worker + gate, feeding the failed report back.
        extra, logfile, report = "", None, ""
        for attempt in range(1, MAX_ATTEMPTS + 1):
            ok, report, logfile = worker_round(n, wt, labels, title, extra, attempt, deadline)
            if ok:
                break
            if time.monotonic() > deadline:
                escalate(n, f"wall-clock budget ({budget_min} min) exceeded", logfile)
                return
            extra = f"## Previous gate report (attempt {attempt} failed)\n\n{report}"
        else:
            escalate(n, f"gate failed {MAX_ATTEMPTS} times; worktree kept at {wt}", logfile)
            return

        push_and_pr(wt, n, title, report)
        verdict, findings = review(wt, n)
        pr_comment(n, findings)
        if verdict == "APPROVE":
            log(f"#{n}: done (approved)")
            return

        # One review bounce.
        if time.monotonic() > deadline:
            escalate(n, f"wall-clock budget ({budget_min} min) exceeded before bounce", logfile)
            return
        extra = f"## Reviewer findings (address these)\n\n{findings}"
        ok, report, logfile = worker_round(n, wt, labels, title, extra, MAX_ATTEMPTS + 1, deadline)
        if not ok:
            escalate(n, f"gate failed after review bounce; worktree kept at {wt}", logfile)
            return
        run(["git", "push", "origin", f"agent/{n}"], cwd=wt)
        verdict, findings = review(wt, n)
        pr_comment(n, findings)
        if verdict != "APPROVE":
            escalate(n, "second REVISE verdict from reviewer", logfile)
        else:
            log(f"#{n}: done (approved after bounce)")
    finally:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        lock_fd.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="AI-factory dispatcher (one pass, stateless)")
    parser.add_argument("--ticket", type=int, help="process exactly this open issue")
    parser.add_argument("--budget-min", type=int, default=90,
                        help="per-ticket wall-clock budget in minutes (default 90)")
    parser.add_argument("--dry-run", action="store_true",
                        help="print planned actions; no side effects")
    args = parser.parse_args()

    if args.ticket:
        if not issue_is_open(args.ticket):
            log(f"#{args.ticket}: not open, nothing to do")
            return 1
        issue = gh_json(["issue", "view", str(args.ticket), "--repo", REPO,
                         "--json", "number,title,body,labels,assignees"])
        process_ticket(issue, args.budget_min, args.dry_run)
        return 0

    active = active_ticket_count()
    capacity = MAX_ACTIVE - active
    log(f"active tickets: {active}, capacity: {max(capacity, 0)}")
    if capacity <= 0:
        log("at capacity, nothing to do")
        return 0
    ready = frontier()
    if not ready:
        log("frontier empty, nothing to do")
        return 0
    for issue in ready[:capacity]:
        log(f"claimable: #{issue['number']} {issue['title']}")
    for issue in ready[:capacity]:
        process_ticket(issue, args.budget_min, args.dry_run)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

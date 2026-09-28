#!/usr/bin/env python3

"""Local dashboard for the AI factory: activity, progress, history.

    python scripts/factory_dashboard.py [--host 0.0.0.0] [--port 8766] [--no-open]

Serves scripts/factory_dashboard.html (loopback by default) plus a JSON snapshot
assembled from one GitHub GraphQL call (issues, timelines, PRs, CI), the
.factory/ state dir, git worktrees, systemd, and the journal. Human answers
go through POST /api/act, which runs the same gh calls the dispatcher does.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import webbrowser
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from factory_dispatch import (
    FACTORY,
    LOGS,
    MAX_ACTIVE,
    MAX_ATTEMPTS,
    REPO,
    ROOT,
    lock_held,
    ticket_lock,
)
from factory_triage import LLM_MODEL, LLM_URL

TRIAGE = Path(__file__).with_name("factory_triage.py")
FACTORY_LABELS = {
    "needs-triage",
    "needs-info",
    "ready-for-agent",
    "ready-for-human",
    "wontfix",
}
ACT_LABELS = FACTORY_LABELS
CLOSE_REASONS = {"completed", "not planned"}

HTML = Path(__file__).with_name("factory_dashboard.html")
ATLAS = ROOT / "site" / "factory.html"
UNIT = "gpuflo-factory"
GATE_CHECKS = ["conflict-markers", "fmt", "clippy", "tests", "notices", "leak-scan"]
SNAPSHOT_TTL = 15  # seconds; /api/snapshot?fresh=1 bypasses
FILE_CAP = 2_000_000  # bytes served per /api/file request
AGENT_BRANCH = re.compile(r"agent/(\d+)$")
ATTEMPT_LOG = re.compile(r"(\d+)-attempt-(\d+)\.log$")
GATE_LINE = re.compile(r"^- ([\w-]+): (PASS|FAIL|SKIP)$", re.M)
VERDICT = re.compile(r"VERDICT:\s*(APPROVE|REVISE)")

# ponytail: first 100 issues / 100 PRs, no pagination; add cursors when the
# tracker outgrows that.
GRAPHQL = """
query($owner:String!,$name:String!){
  repository(owner:$owner,name:$name){
    issues(first:100,orderBy:{field:CREATED_AT,direction:DESC}){
      nodes{
        number title state url createdAt updatedAt closedAt body
        labels(first:20){nodes{name color}}
        assignees(first:5){nodes{login}}
        timelineItems(first:100,itemTypes:[LABELED_EVENT,UNLABELED_EVENT,
          ASSIGNED_EVENT,UNASSIGNED_EVENT,ISSUE_COMMENT,CROSS_REFERENCED_EVENT,
          CLOSED_EVENT,REOPENED_EVENT]){
          nodes{
            __typename
            ... on LabeledEvent{createdAt label{name} actor{login}}
            ... on UnlabeledEvent{createdAt label{name} actor{login}}
            ... on AssignedEvent{createdAt assignee{... on User{login}}}
            ... on UnassignedEvent{createdAt assignee{... on User{login}}}
            ... on IssueComment{createdAt author{login} body}
            ... on CrossReferencedEvent{createdAt source{... on PullRequest{number}}}
            ... on ClosedEvent{createdAt actor{login}}
            ... on ReopenedEvent{createdAt actor{login}}
          }
        }
      }
    }
    pullRequests(first:100,orderBy:{field:CREATED_AT,direction:DESC}){
      nodes{
        number title state url headRefName createdAt mergedAt closedAt isDraft
        additions deletions changedFiles body reviewDecision
        labels(first:10){nodes{name}}
        comments(first:30){nodes{createdAt author{login} body}}
        commits(last:1){nodes{commit{statusCheckRollup{state
          contexts(first:60){nodes{__typename
            ... on CheckRun{name conclusion status}
            ... on StatusContext{context state}}}}}}}
      }
    }
  }
}
"""


def iso(ts: float) -> str:
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sh(cmd: list[str], cwd: Path | None = None) -> str:
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    return proc.stdout if proc.returncode == 0 else ""


def read_text(path: Path) -> str:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return ""


def file_meta(path: Path) -> dict | None:
    try:
        st = path.stat()
    except OSError:
        return None
    return {
        "path": str(path.relative_to(FACTORY)),
        "size": st.st_size,
        "mtime": iso(st.st_mtime),
    }


# ---------------------------------------------------------------- GitHub


def github() -> dict:
    owner, name = REPO.split("/", 1)
    proc = subprocess.run(
        ["gh", "api", "graphql", "-f", f"query={GRAPHQL}", "-F", f"owner={owner}", "-F", f"name={name}"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode:
        raise RuntimeError(proc.stderr.strip() or "gh api graphql failed")
    return json.loads(proc.stdout)["data"]


def pr_checks(pr: dict) -> dict:
    commits = pr.get("commits", {}).get("nodes") or []
    rollup = (
        (commits[0].get("commit") or {}).get("statusCheckRollup") if commits else None
    )
    checks = []
    for ctx in (rollup or {}).get("contexts", {}).get("nodes") or []:
        if ctx["__typename"] == "CheckRun":
            result = ctx["conclusion"] or ctx["status"]
            checks.append({"name": ctx["name"], "result": result})
        else:
            checks.append({"name": ctx["context"], "result": ctx["state"]})
    counts: dict[str, int] = {}
    for check in checks:
        counts[check["result"]] = counts.get(check["result"], 0) + 1
    return {"state": (rollup or {}).get("state"), "counts": counts, "list": checks}


def pr_record(pr: dict) -> dict:
    verdicts = [
        {
            "at": c["createdAt"],
            "verdict": VERDICT.search(c["body"] or "").group(1),
            "body": c["body"],
        }
        for c in pr.get("comments", {}).get("nodes") or []
        if VERDICT.search(c["body"] or "")
    ]
    body = pr.get("body") or ""
    gate_text = (
        body.split("## Gate report", 1)[1].strip() if "## Gate report" in body else ""
    )
    return {
        "number": pr["number"],
        "state": pr["state"],
        "url": pr["url"],
        "draft": pr["isDraft"],
        "created_at": pr["createdAt"],
        "merged_at": pr["mergedAt"],
        "closed_at": pr["closedAt"],
        "additions": pr["additions"],
        "deletions": pr["deletions"],
        "changed_files": pr["changedFiles"],
        "checks": pr_checks(pr),
        "verdicts": verdicts,
        "gate_text": gate_text,
        "labels": [lab["name"] for lab in pr.get("labels", {}).get("nodes") or []],
        "review_decision": pr.get("reviewDecision"),
    }


def issue_events(issue: dict) -> list[dict]:
    events = []
    for item in issue.get("timelineItems", {}).get("nodes") or []:
        kind = item["__typename"]
        at = item.get("createdAt")
        if not at:
            continue
        if kind == "LabeledEvent":
            events.append(
                {"at": at, "kind": "labeled", "detail": item["label"]["name"]}
            )
        elif kind == "UnlabeledEvent":
            events.append(
                {"at": at, "kind": "unlabeled", "detail": item["label"]["name"]}
            )
        elif kind == "AssignedEvent":
            events.append(
                {
                    "at": at,
                    "kind": "assigned",
                    "detail": (item.get("assignee") or {}).get("login", "?"),
                }
            )
        elif kind == "UnassignedEvent":
            events.append(
                {
                    "at": at,
                    "kind": "unassigned",
                    "detail": (item.get("assignee") or {}).get("login", "?"),
                }
            )
        elif kind == "IssueComment":
            body = item.get("body") or ""
            author = (item.get("author") or {}).get("login", "?")
            if body.startswith("Factory dispatcher escalating"):
                reason = body.split("escalating:", 1)[-1].split("\n", 1)[0].strip(" .")
                events.append(
                    {"at": at, "kind": "escalated", "detail": reason, "body": body}
                )
            elif body.startswith("Triage"):
                events.append(
                    {
                        "at": at,
                        "kind": "triaged",
                        "detail": body.split("\n", 1)[0],
                        "body": body,
                    }
                )
            else:
                events.append(
                    {
                        "at": at,
                        "kind": "comment",
                        "detail": f"{author}: {body[:120]}",
                        "body": body,
                    }
                )
        elif kind == "CrossReferencedEvent":
            number = (item.get("source") or {}).get("number")
            if number:
                events.append({"at": at, "kind": "referenced", "detail": f"#{number}"})
        elif kind == "ClosedEvent":
            events.append(
                {
                    "at": at,
                    "kind": "closed",
                    "detail": (item.get("actor") or {}).get("login", "?"),
                }
            )
        elif kind == "ReopenedEvent":
            events.append(
                {
                    "at": at,
                    "kind": "reopened",
                    "detail": (item.get("actor") or {}).get("login", "?"),
                }
            )
    return events


# ---------------------------------------------------------------- disk


def worktree_state(wt: Path) -> dict | None:
    if not wt.is_dir():
        return None
    head = sh(["git", "rev-parse", "--short", "HEAD"], cwd=wt).strip()
    commits = []
    for line in sh(
        ["git", "log", "--format=%h%x1f%s%x1f%cI", "origin/main..HEAD"], cwd=wt
    ).splitlines():
        sha, subject, at = line.split("\x1f")
        commits.append({"sha": sha, "subject": subject, "at": at})
    return {
        "path": str(wt),
        "head": head,
        "commits": commits,
        "diffstat": sh(
            ["git", "diff", "--shortstat", "origin/main..HEAD"], cwd=wt
        ).strip(),
        # Same exclusions as commit_leftovers: the prompt and gate dir are expected.
        "dirty": bool(
            sh(
                [
                    "git",
                    "status",
                    "--porcelain",
                    "--",
                    ".",
                    ":(exclude).factory-prompt.md",
                    ":(exclude).factory",
                ],
                cwd=wt,
            ).strip()
        ),
    }


def disk_state(n: int) -> dict:
    wt = FACTORY / f"wt-{n}"
    attempts = []
    for log in sorted(LOGS.glob(f"{n}-attempt-*.log")):
        meta = file_meta(log)
        if meta:
            meta["attempt"] = int(ATTEMPT_LOG.search(log.name).group(2))
            attempts.append(meta)
    attempts.sort(key=lambda a: a["attempt"])

    gate = None
    gate_path = wt / ".factory" / f"gate-report-{n}.md"
    meta = file_meta(gate_path)
    if meta:
        text = read_text(gate_path)
        gate = {**meta, "checks": dict(GATE_LINE.findall(text)), "text": text}

    review = None
    review_path = FACTORY / f"review-{n}.md"
    meta = file_meta(review_path)
    if meta:
        text = read_text(review_path)
        m = VERDICT.search(text)
        review = {**meta, "verdict": m.group(1) if m else None, "text": text}

    return {
        "lock_held": lock_held(ticket_lock(n)),
        "attempts": attempts,
        "gate": gate,
        "review": review,
        "prompt": file_meta(wt / ".factory-prompt.md"),
        "pr_body": file_meta(FACTORY / f"pr-body-{n}.md"),
        "worktree": worktree_state(wt),
    }


def disk_ticket_numbers() -> set[int]:
    numbers = set()
    for wt in FACTORY.glob("wt-*"):
        if wt.name[3:].isdigit():
            numbers.add(int(wt.name[3:]))
    for log in LOGS.glob("*-attempt-*.log"):
        numbers.add(int(ATTEMPT_LOG.search(log.name).group(1)))
    return numbers


# ---------------------------------------------------------------- systemd


def journal_runs() -> list[dict]:
    out = sh(
        [
            "journalctl",
            "--user",
            "-u",
            f"{UNIT}.service",
            "-o",
            "json",
            "-n",
            "3000",
            "--no-pager",
        ]
    )
    runs: list[dict] = []
    cur = None
    for line in out.splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        msg = entry.get("MESSAGE")
        if not isinstance(msg, str):
            continue
        at = int(entry["__REALTIME_TIMESTAMP"]) / 1e6
        if entry.get("SYSLOG_IDENTIFIER") == "systemd":
            if msg.startswith("Starting"):
                cur = {
                    "started": iso(at),
                    "finished": None,
                    "result": "running",
                    "lines": [],
                }
                runs.append(cur)
            elif cur and msg.startswith("Finished"):
                cur["finished"], cur["result"], cur = iso(at), "done", None
            elif cur and msg.startswith("Failed"):
                cur["finished"], cur["result"], cur = iso(at), "failed", None
        elif cur is not None:
            cur["lines"].append(msg)
    return runs[-100:]


def dispatcher() -> dict:
    timer = {}
    raw = sh(
        ["systemctl", "--user", "list-timers", f"{UNIT}.timer", "--output=json"]
    )
    if raw:
        rows = json.loads(raw)
        if rows:
            timer = {
                "next": iso(rows[0]["next"] / 1e6) if rows[0].get("next") else None,
                "last": iso(rows[0]["last"] / 1e6) if rows[0].get("last") else None,
            }
    timer["active"] = (
        sh(["systemctl", "--user", "is-active", f"{UNIT}.timer"]).strip()
        == "active"
    )
    service_active = (
        sh(["systemctl", "--user", "is-active", f"{UNIT}.service"]).strip()
        == "active"
    )
    return {"timer": timer, "service_active": service_active, "runs": journal_runs()}


def triage_llm_online() -> bool:
    base = LLM_URL.rsplit("/chat/completions", 1)[0]
    try:
        with urllib.request.urlopen(f"{base}/models", timeout=1.5) as resp:
            return resp.status == 200
    except (urllib.error.URLError, OSError, ValueError):
        return False


# ---------------------------------------------------------------- tickets


def stage_of(labels: set[str], state: str, pr: dict | None, lock: bool) -> str:
    if pr and pr["merged_at"]:
        return "merged"
    if lock:
        return "in-flight"
    if "ready-for-human" in labels:
        return "escalated"
    if pr and pr["state"] == "OPEN":
        return "pr-open"
    if state != "OPEN":
        return "closed"
    if "wontfix" in labels:
        return "wontfix"
    if "needs-info" in labels:
        return "needs-info"
    if "ready-for-agent" in labels:
        return "queued"
    if "needs-triage" in labels:
        return "triage"
    return "other"


def phase_of(disk: dict) -> dict | None:
    """Newest local artifact tells where an in-flight pipeline is."""
    candidates = []
    if disk["attempts"]:
        last = disk["attempts"][-1]
        candidates.append((last["mtime"], "worker", last["attempt"]))
    for name in ("gate", "pr_body", "review"):
        if disk.get(name):
            candidates.append((disk[name]["mtime"], name, None))
    if not candidates:
        return None
    at, artifact, attempt = max(candidates)
    return {"at": at, "artifact": artifact, "attempt": attempt}


def build_ticket(issue: dict, pr: dict | None, disk: dict) -> dict:
    labels = {lab["name"] for lab in issue.get("labels", {}).get("nodes") or []}
    events = issue_events(issue)
    if pr:
        events.append(
            {"at": pr["created_at"], "kind": "pr-opened", "detail": f"#{pr['number']}"}
        )
        for v in pr["verdicts"]:
            events.append(
                {
                    "at": v["at"],
                    "kind": "verdict",
                    "detail": v["verdict"],
                    "body": v["body"],
                }
            )
        if pr["merged_at"]:
            events.append(
                {"at": pr["merged_at"], "kind": "merged", "detail": f"#{pr['number']}"}
            )
        elif pr["closed_at"]:
            events.append(
                {
                    "at": pr["closed_at"],
                    "kind": "pr-closed",
                    "detail": f"#{pr['number']}",
                }
            )
    for a in disk["attempts"]:
        events.append(
            {
                "at": a["mtime"],
                "kind": "attempt",
                "detail": f"attempt {a['attempt']} log",
                "path": a["path"],
            }
        )
    if disk["gate"]:
        failed = [k for k, v in disk["gate"]["checks"].items() if v == "FAIL"]
        events.append(
            {
                "at": disk["gate"]["mtime"],
                "kind": "gate",
                "detail": "FAIL: " + ", ".join(failed) if failed else "PASS",
                "path": disk["gate"]["path"],
            }
        )
    events.sort(key=lambda e: e["at"])
    lock = disk["lock_held"]
    return {
        "number": issue["number"],
        "title": issue["title"],
        "state": issue["state"],
        "url": issue["url"],
        "body": issue.get("body") or "",
        "labels": sorted(labels),
        "assignees": [
            a["login"] for a in issue.get("assignees", {}).get("nodes") or []
        ],
        "created_at": issue["createdAt"],
        "updated_at": issue["updatedAt"],
        "closed_at": issue["closedAt"],
        "worker": "droid" if "chore" in labels else "omp",
        "stage": stage_of(labels, issue["state"], pr, lock),
        "phase": phase_of(disk) if lock else None,
        "pr": pr,
        "events": events,
        **disk,
    }


def snapshot() -> dict:
    errors = []
    issues: list[dict] = []
    prs: dict[int, dict] = {}
    try:
        repo = github()["repository"]
        issues = repo["issues"]["nodes"]
        for pr in repo["pullRequests"]["nodes"]:
            m = AGENT_BRANCH.fullmatch(pr["headRefName"])
            if not m:
                continue
            n = int(m.group(1))
            rec = pr_record(pr)
            # Prefer the merged PR, else the newest, when a branch had several.
            if n not in prs or (rec["merged_at"] and not prs[n]["merged_at"]):
                prs[n] = rec
    except (RuntimeError, ValueError, KeyError) as exc:
        errors.append(f"github: {exc}")

    on_disk = disk_ticket_numbers()
    tickets = []
    for issue in issues:
        n = issue["number"]
        labels = {lab["name"] for lab in issue.get("labels", {}).get("nodes") or []}
        if not (labels & FACTORY_LABELS or n in prs or n in on_disk):
            continue
        tickets.append(build_ticket(issue, prs.get(n), disk_state(n)))
    tickets.sort(key=lambda t: t["number"], reverse=True)

    return {
        "generated_at": iso(time.time()),
        "repo": REPO,
        "root": str(ROOT),
        "errors": errors,
        "config": {
            "max_active": MAX_ACTIVE,
            "max_attempts": MAX_ATTEMPTS,
            "budget_min": 90,
            "timer_interval": "10min",
            "gate_checks": GATE_CHECKS,
            "workers": {"default": "omp -p", "chore": "droid exec --auto medium"},
            "reviewer": "codex exec",
            "triage": {
                "url": LLM_URL,
                "model": LLM_MODEL,
                "online": triage_llm_online(),
            },
            "state_dir": str(FACTORY),
        },
        "active": sum(1 for t in tickets if t["lock_held"]),
        "dispatcher": dispatcher(),
        "tickets": tickets,
    }


# ---------------------------------------------------------------- server

_cache: dict = {"at": 0.0, "data": None}
_cache_lock = threading.Lock()


def cached_snapshot(fresh: bool) -> dict:
    with _cache_lock:
        if fresh or _cache["data"] is None or time.time() - _cache["at"] > SNAPSHOT_TTL:
            _cache["data"] = snapshot()
            _cache["at"] = time.time()
        return _cache["data"]


# ---------------------------------------------------------------- actions


def step(steps: list[dict], cmd: list[str], cwd: Path | None = None) -> bool:
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    steps.append(
        {
            "cmd": " ".join(cmd[:4]) + (" …" if len(cmd) > 4 else ""),
            "ok": proc.returncode == 0,
            "output": (proc.stdout + proc.stderr).strip()[-4000:],
        }
    )
    return proc.returncode == 0


def labels_arg(req: dict, key: str) -> list[str]:
    labels = req.get(key) or []
    if not isinstance(labels, list) or not set(labels) <= ACT_LABELS:
        raise ValueError(f"{key}: labels must be a subset of {sorted(ACT_LABELS)}")
    return labels


def act(req: dict) -> dict:
    """Apply one human answer. Every mutation is a `gh`/dispatcher call the
    dispatcher itself performs; nothing here bypasses the factory's rules."""
    op = req.get("op")
    number = req.get("number")
    if not isinstance(number, int) or number <= 0:
        raise ValueError("number: positive int required")
    comment = req.get("comment") or ""
    if not isinstance(comment, str) or len(comment) > 20000:
        raise ValueError("comment: string ≤ 20000 chars")
    steps: list[dict] = []
    gh = ["gh"]

    if op == "issue":
        add, remove = labels_arg(req, "add"), labels_arg(req, "remove")
        close = req.get("close")
        if close is not None and close not in CLOSE_REASONS:
            raise ValueError(f"close: one of {sorted(CLOSE_REASONS)}")
        n = str(number)
        if comment.strip():
            step(steps, [*gh, "issue", "comment", n, "--repo", REPO, "--body", comment])
        edit = []
        for label in add:
            edit += ["--add-label", label]
        for label in remove:
            edit += ["--remove-label", label]
        if req.get("unassign"):
            edit += ["--remove-assignee", "@me"]
        if req.get("assign"):
            edit += ["--add-assignee", "@me"]
        if edit:
            step(steps, [*gh, "issue", "edit", n, "--repo", REPO, *edit])
        if close:
            step(steps, [*gh, "issue", "close", n, "--repo", REPO, "--reason", close])
    elif op == "pr":
        add, remove = labels_arg(req, "add"), labels_arg(req, "remove")
        edit = []
        for label in add:
            edit += ["--add-label", label]
        for label in remove:
            edit += ["--remove-label", label]
        if comment.strip():
            step(
                steps,
                [*gh, "pr", "comment", str(number), "--repo", REPO, "--body", comment],
            )
        if edit:
            step(steps, [*gh, "pr", "edit", str(number), "--repo", REPO, *edit])
    elif op == "triage":
        step(steps, [sys.executable, str(TRIAGE), "--issue", str(number)], cwd=ROOT)
    elif op == "cleanup":
        if lock_held(ticket_lock(number)):
            raise ValueError(f"#{number} is in flight; not removing its worktree")
        wt = FACTORY / f"wt-{number}"
        if wt.is_dir():
            step(steps, ["git", "worktree", "remove", "--force", str(wt)], cwd=ROOT)
        step(steps, ["git", "branch", "-D", f"agent/{number}"], cwd=ROOT)
        ticket_lock(number).unlink(missing_ok=True)
    else:
        raise ValueError(f"op: unknown {op!r}")

    with _cache_lock:
        _cache["at"] = 0.0  # next snapshot re-reads GitHub
    return {"ok": all(s["ok"] for s in steps), "steps": steps}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        url = urlparse(self.path)
        query = parse_qs(url.query)
        if url.path == "/":
            self._send(200, "text/html; charset=utf-8", HTML.read_bytes())
        elif url.path == "/atlas":
            self._send(200, "text/html; charset=utf-8", ATLAS.read_bytes())
        elif url.path == "/api/snapshot":
            data = cached_snapshot("fresh" in query)
            self._send(200, "application/json", json.dumps(data).encode())
        elif url.path == "/api/file":
            self._file(query.get("path", [""])[0])
        else:
            self.send_error(404)

    def do_POST(self) -> None:
        if urlparse(self.path).path != "/api/act":
            self.send_error(404)
            return
        # Custom header forces a CORS preflight that this server never answers,
        # so a stray web page cannot drive actions through the loopback port.
        if self.headers.get("X-Factory-Act") != "1":
            self.send_error(403)
            return
        length = int(self.headers.get("Content-Length") or 0)
        try:
            req = json.loads(self.rfile.read(length) or b"{}")
            result = act(req)
        except ValueError as exc:
            result = {"ok": False, "steps": [], "error": str(exc)}
        self._send(200, "application/json", json.dumps(result).encode())

    def _file(self, rel: str) -> None:
        root = FACTORY.resolve()
        path = (root / rel).resolve()
        if not rel or not path.is_relative_to(root) or not path.is_file():
            self.send_error(404)
            return
        data = path.read_bytes()
        if len(data) > FILE_CAP:
            data = b"[... truncated ...]\n" + data[-FILE_CAP:]
        self._send(200, "text/plain; charset=utf-8", data)

    def _send(self, status: int, ctype: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args: object) -> None:
        pass


def main() -> int:
    parser = argparse.ArgumentParser(description="Local AI-factory dashboard")
    parser.add_argument("--port", type=int, default=8766)
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="bind address; 0.0.0.0 exposes the dashboard AND /api/act "
        "(which mutates GitHub with your gh credentials) to the whole network",
    )
    parser.add_argument("--no-open", action="store_true", help="do not open a browser")
    parser.add_argument(
        "--json", action="store_true", help="print one snapshot and exit"
    )
    args = parser.parse_args()

    if args.json:
        print(json.dumps(snapshot(), indent=2))
        return 0

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    url = f"http://127.0.0.1:{args.port}/"
    print(
        f"factory dashboard: listening on {args.host}:{args.port}  "
        f"(repo {REPO}, state {FACTORY})",
        flush=True,
    )
    if not args.no_open:
        webbrowser.open(url)
    with contextlib.suppress(KeyboardInterrupt):
        server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

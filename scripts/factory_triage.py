#!/usr/bin/env python3

"""Triage needs-triage issues using the local model endpoint.

Stateless worker: reads issues via gh, asks the local model for a decision,
applies labels/comments via gh. wontfix is only ever proposed, never applied.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = subprocess.run(
    ["git", "-C", str(ROOT), "remote", "get-url", "origin"],
    capture_output=True, text=True, check=True,
).stdout.strip().rsplit("github.com", 1)[-1].strip(":/").removesuffix(".git")
LLM_URL = os.environ.get("FACTORY_LLM_URL", "http://127.0.0.1:11435/v1/chat/completions")
LLM_MODEL = os.environ.get("FACTORY_LLM_MODEL", "ornith-ai/Ornith-1.5-35B-A3B-GGUF:Q4_K_M")
LABELS_DOC = Path(__file__).resolve().parent.parent / "docs" / "agents" / "triage-labels.md"

DECISIONS = ("ready-for-agent", "needs-info", "ready-for-human", "wontfix-proposal")


def gh(*args: str) -> str:
    result = subprocess.run(
        ["gh", *args, "--repo", REPO],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(f"gh {' '.join(args)} failed: {result.stderr.strip()}", file=sys.stderr)
        raise SystemExit(1)
    return result.stdout


def system_prompt() -> str:
    try:
        label_table = LABELS_DOC.read_text()
    except OSError as exc:
        print(f"cannot read {LABELS_DOC}: {exc}", file=sys.stderr)
        raise SystemExit(1)
    return f"""You are the triage bot for the rocm-cli fork issue tracker.

Label reference:

{label_table}

Choose exactly one decision for the issue:
- "ready-for-agent": the issue is fully specified — it has a problem statement AND acceptance criteria or an observable done-condition.
- "needs-info": information is missing; state the specific missing information as a question.
- "ready-for-human": needs design judgment, touches release/signing/upstream policy, or has blast radius beyond the fork.
- "wontfix-proposal": the issue should not be actioned; explain why.

Respond with strict JSON only, no markdown, no prose outside the JSON:
{{"decision": "<one of ready-for-agent|needs-info|ready-for-human|wontfix-proposal>", "rationale": "<one short paragraph>", "question": "<the question for the reporter, or empty string if decision is not needs-info>"}}"""


def call_llm(messages: list[dict]) -> str:
    payload = json.dumps({
        "model": LLM_MODEL,
        "messages": messages,
        "temperature": 0.1,
    }).encode()
    req = urllib.request.Request(
        LLM_URL, data=payload,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            body = json.load(resp)
    except (urllib.error.URLError, OSError, TimeoutError) as exc:
        print(
            f"cannot reach local model at {LLM_URL}: {exc}\n"
            "Is rocmd serving? Set FACTORY_LLM_URL if the endpoint differs.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    return body["choices"][0]["message"]["content"]


def parse_decision(text: str) -> dict | None:
    # ponytail: naive fence strip; structured-output API if the model misbehaves
    text = text.strip()
    if text.startswith("```"):
        text = text.strip("`").removeprefix("json").strip()
    try:
        obj = json.loads(text)
    except json.JSONDecodeError:
        return None
    if not isinstance(obj, dict) or obj.get("decision") not in DECISIONS:
        return None
    obj.setdefault("rationale", "")
    obj.setdefault("question", "")
    return obj


def fetch_issue(number: int) -> dict:
    raw = gh("issue", "view", str(number), "--json", "number,title,body,comments,labels,state")
    return json.loads(raw)


def triage_issue(issue: dict) -> dict | None:
    """Ask the model for a decision. One retry on bad JSON, then None."""
    comments = "\n\n".join(
        f"Comment by {c.get('author', {}).get('login', '?')}:\n{c.get('body', '')}"
        for c in issue.get("comments", [])
    )
    user_msg = (
        f"Issue #{issue['number']}: {issue['title']}\n\n"
        f"Body:\n{issue.get('body') or '(empty)'}\n\n"
        f"Comments:\n{comments or '(none)'}"
    )
    messages = [
        {"role": "system", "content": system_prompt()},
        {"role": "user", "content": user_msg},
    ]
    for attempt in range(2):
        reply = call_llm(messages)
        decision = parse_decision(reply)
        if decision is not None:
            return decision
        messages.append({"role": "assistant", "content": reply})
        messages.append({
            "role": "user",
            "content": "That was not valid JSON matching the required schema. "
                       "Reply with ONLY the JSON object.",
        })
    return None


def apply_decision(number: int, decision: dict, dry_run: bool) -> None:
    label = decision["decision"]
    rationale = decision["rationale"]
    if label == "needs-info" and decision["question"]:
        comment = f"Triage: {rationale}\n\nQuestion: {decision['question']}"
    elif label == "wontfix-proposal":
        comment = f"Triage proposal: wontfix — {rationale}"
    else:
        comment = f"Triage: {rationale}"

    if dry_run:
        print(f"#{number} [dry-run] decision: {json.dumps(decision)}")
        print(f"#{number} [dry-run] comment: {comment}")
        return

    gh("issue", "comment", str(number), "--body", comment)
    if label == "wontfix-proposal":
        return  # never apply wontfix; leave needs-triage for a human
    gh("issue", "edit", str(number), "--remove-label", "needs-triage", "--add-label", label)


def list_needs_triage() -> list[int]:
    raw = gh("issue", "list", "--label", "needs-triage", "--state", "open", "--json", "number")
    return [item["number"] for item in json.loads(raw)]


def main() -> int:
    parser = argparse.ArgumentParser(description="Triage needs-triage issues with the local model.")
    parser.add_argument("--issue", type=int, help="triage a single issue")
    parser.add_argument("--replay", help="comma-separated issue numbers: print decisions, no writes")
    parser.add_argument("--dry-run", action="store_true", help="print decisions instead of applying")
    args = parser.parse_args()

    replay = bool(args.replay)
    if replay:
        numbers = [int(n) for n in args.replay.split(",")]
    elif args.issue:
        numbers = [args.issue]
    else:
        numbers = list_needs_triage()

    if not numbers:
        print("no needs-triage issues")
        return 0

    exit_code = 0
    for number in numbers:
        issue = fetch_issue(number)
        decision = triage_issue(issue)
        if decision is None:
            print(f"#{number}: model returned unparseable JSON twice; skipping", file=sys.stderr)
            exit_code = 1
            continue
        if replay:
            print(f"#{number}: {json.dumps(decision)}")
        else:
            apply_decision(number, decision, args.dry_run)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

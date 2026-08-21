#!/usr/bin/env python3
"""Generate THIRD_PARTY_NOTICES.txt from the locked Cargo dependency graph.

Covers the crates actually linked into the shipped x86_64 Linux binary
(normal dependencies for that target). Fails when a dependency's declared
license cannot be satisfied by the accepted permissive policy so release
automation cannot ship an unreviewed license.
"""

import json
import re
import subprocess
import sys

TARGET = "x86_64-unknown-linux-gnu"

# Licenses gruflo may redistribute under. An OR expression is acceptable when
# any alternative is accepted; an AND expression needs every part accepted.
ACCEPTED_ATOMS = {
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "ISC",
    "MIT-0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
    "0BSD",
}


def acceptable(expression: str) -> bool:
    # Legacy Cargo metadata uses `/` as OR.
    normalized = expression.replace("/", " OR ").strip()
    for alternative in re.split(r"\bOR\b", normalized):
        parts = [p.strip().strip("()") for p in re.split(r"\bAND\b", alternative)]
        if parts and all(p in ACCEPTED_ATOMS for p in parts if p):
            return True
    return False


def main() -> int:
    shipped = set()
    tree = subprocess.check_output(
        [
            "cargo", "tree", "--locked", "--target", TARGET,
            "-e", "normal", "--prefix", "none", "--format", "{p}",
        ],
        text=True,
    )
    for line in tree.splitlines():
        line = line.replace(" (*)", "").strip()
        match = re.match(r"^(\S+) v(\S+)", line)
        if match and match.group(1) != "gruflo":
            shipped.add((match.group(1), match.group(2)))

    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"]
        )
    )
    by_key = {(p["name"], p["version"]): p for p in metadata["packages"]}

    lines = [
        "Third-party notices for gruflo",
        "",
        "gruflo is distributed under the MIT license (see LICENSE).",
        f"The {TARGET} binary statically links the following Rust",
        "crates, listed with their declared SPDX license expressions and",
        "upstream sources. Full license texts are available from each listed",
        "source. No code was copied or closely translated from other",
        "projects; the crates below are ordinary Cargo dependencies.",
        "",
    ]
    rejected = []
    for name, version in sorted(shipped):
        package = by_key.get((name, version))
        if package is None:
            rejected.append(f"{name} {version}: not present in cargo metadata")
            continue
        license_expr = package.get("license") or "UNKNOWN"
        if not acceptable(license_expr):
            rejected.append(f"{name} {version}: {license_expr}")
        source = package.get("repository") or package.get("homepage") or ""
        lines.append(f"- {name} {version} — {license_expr}")
        if source:
            lines.append(f"  {source}")
    if rejected:
        print("licenses outside the accepted policy:", file=sys.stderr)
        for entry in rejected:
            print(f"  {entry}", file=sys.stderr)
        return 1
    with open("THIRD_PARTY_NOTICES.txt", "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
    print(f"wrote THIRD_PARTY_NOTICES.txt covering {len(shipped)} crates")
    return 0


if __name__ == "__main__":
    sys.exit(main())

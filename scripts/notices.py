#!/usr/bin/env python3
"""Generate complete locked-dependency notices for the shipped Linux target."""

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

TARGET = "x86_64-unknown-linux-gnu"
ACCEPTED_LICENSES = {
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MIT-0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
}
NOTICE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE")


class SpdxParser:
    """Small evaluator for Cargo SPDX expressions with AND/OR/parentheses/WITH."""

    def __init__(self, expression: str):
        expression = expression.replace("/", " OR ")
        self.tokens = re.findall(r"\(|\)|\bAND\b|\bOR\b|\bWITH\b|[^\s()]+", expression)
        self.index = 0

    def parse(self) -> bool:
        result = self.parse_or()
        if self.index != len(self.tokens):
            raise ValueError("trailing SPDX tokens")
        return result

    def parse_or(self) -> bool:
        result = self.parse_and()
        while self.peek() == "OR":
            self.index += 1
            right = self.parse_and()
            result = result or right
        return result

    def parse_and(self) -> bool:
        result = self.parse_primary()
        while self.peek() == "AND":
            self.index += 1
            right = self.parse_primary()
            result = result and right
        return result

    def parse_primary(self) -> bool:
        token = self.take()
        if token == "(":
            result = self.parse_or()
            if self.take() != ")":
                raise ValueError("unclosed SPDX parenthesis")
            return result
        if token in {"AND", "OR", "WITH", ")"}:
            raise ValueError(f"unexpected SPDX token {token}")
        license_id = token
        if self.peek() == "WITH":
            self.index += 1
            license_id += " WITH " + self.take()
        return license_id in ACCEPTED_LICENSES

    def peek(self):
        return self.tokens[self.index] if self.index < len(self.tokens) else None

    def take(self):
        token = self.peek()
        if token is None:
            raise ValueError("unexpected end of SPDX expression")
        self.index += 1
        return token


def acceptable(expression: str) -> bool:
    try:
        return SpdxParser(expression).parse()
    except ValueError:
        return False


def shipped_packages():
    tree = subprocess.check_output(
        [
            "cargo", "tree", "--locked", "--target", TARGET,
            "-e", "normal", "--prefix", "none", "--format", "{p}",
        ],
        text=True,
    )
    shipped = set()
    for line in tree.splitlines():
        line = line.replace(" (*)", "").strip()
        match = re.match(r"^(\S+) v(\S+)", line)
        if match and match.group(1) != "gruflo":
            shipped.add((match.group(1), match.group(2)))
    return shipped


def notice_files(package):
    directory = Path(package["manifest_path"]).parent
    files = []
    for path in sorted(directory.iterdir()):
        if path.is_file() and path.name.upper().startswith(NOTICE_PREFIXES):
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                text = path.read_text(encoding="utf-8", errors="replace")
            if text.strip():
                files.append((path.name, text.rstrip()))
    return files


def main() -> int:
    shipped = shipped_packages()
    metadata = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"])
    )
    by_key = {(p["name"], p["version"]): p for p in metadata["packages"]}
    rejected = []
    packages = []
    for key in sorted(shipped):
        package = by_key.get(key)
        if package is None:
            rejected.append(f"{key[0]} {key[1]}: absent from cargo metadata")
            continue
        expression = package.get("license") or "UNKNOWN"
        if not acceptable(expression):
            rejected.append(f"{key[0]} {key[1]}: unaccepted/invalid {expression}")
        files = notice_files(package)
        if not files:
            rejected.append(f"{key[0]} {key[1]}: no license/notice text in crate")
        packages.append((package, expression, files))
    if rejected:
        print("notice generation failed closed:", file=sys.stderr)
        for item in rejected:
            print(f"  {item}", file=sys.stderr)
        return 1

    lines = [
        "Third-party notices for gruflo",
        "",
        "gruflo is distributed under the MIT license (see LICENSE).",
        f"The {TARGET} binary uses the locked Rust crates below. Their exact",
        "redistribution notices and license texts are reproduced from each crate.",
        "",
    ]
    # Identical texts are emitted once with every package to which they apply.
    texts = {}
    for package, expression, files in packages:
        source = package.get("repository") or package.get("homepage") or "(crate source metadata absent)"
        lines.extend([
            "=" * 78,
            f"{package['name']} {package['version']}",
            f"SPDX: {expression}",
            f"Source: {source}",
        ])
        for filename, text in files:
            digest = hashlib.sha256(text.encode()).hexdigest()
            texts.setdefault(digest, {"text": text, "users": []})["users"].append(
                f"{package['name']} {package['version']} ({filename})"
            )

    lines.extend(["", "=" * 78, "Embedded license and notice texts", ""])
    for entry in texts.values():
        lines.append("Applies to: " + ", ".join(entry["users"]))
        lines.append("-" * 78)
        lines.append(entry["text"])
        lines.append("")

    Path("THIRD_PARTY_NOTICES.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote complete notices for {len(packages)} crates ({len(texts)} unique texts)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Find documents whose OKF frontmatter stale_after has passed.

OKF declares expiry; it does not compute it. This script computes it.

Usage: python3 scripts/check_stale_after.py
"""
from __future__ import annotations

import datetime as dt
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STALE_RE = re.compile(r"^stale_after:\s*(\d{4}-\d{2}-\d{2})\s*$", re.M)
STATUS_RE = re.compile(r"^status:\s*(\S+)\s*$", re.M)


def frontmatter(text: str) -> str | None:
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    return text[4:end] if end != -1 else None


def main() -> int:
    today = dt.date.today()
    expired: list[tuple[str, str]] = []
    malformed: list[tuple[str, str]] = []
    checked = 0

    for path in sorted(ROOT.glob("docs/**/*.md")):
        fm = frontmatter(path.read_text(encoding="utf-8"))
        if fm is None:
            continue
        checked += 1
        status = STATUS_RE.search(fm)
        if status and status.group(1) == "deprecated":
            continue  # a retired document is not subject to the expiry check
        m = STALE_RE.search(fm)
        if not m:
            continue
        raw = m.group(1)
        try:
            when = dt.date.fromisoformat(raw)
        except ValueError:
            # The format is YYYY-MM-DD but the date does not exist (2026-13-01 and
            # the like) — STALE_RE checks digits, not calendar validity. Rather than
            # dying with a traceback, it fails cleanly naming the document and value.
            malformed.append((str(path.relative_to(ROOT)), raw))
            continue
        if when < today:
            expired.append((str(path.relative_to(ROOT)), raw))

    if malformed:
        print("FAIL: documents with a malformed stale_after:")
        for p, raw in malformed:
            print(f"  {p} (stale_after: {raw!r})")
        print("\nFix them to a real calendar date in YYYY-MM-DD form.")
    if expired:
        print("FAIL: documents past their stale_after:")
        for p, when in expired:
            print(f"  {p} (stale_after: {when})")
        print("\nCheck the document against the current code, then extend stale_after")
        print("and add a verified[] entry, or update or retire it.")
    if malformed or expired:
        return 1

    print(f"OK: {checked} documents with frontmatter, none expired")
    return 0


if __name__ == "__main__":
    sys.exit(main())

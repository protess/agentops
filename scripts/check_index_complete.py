#!/usr/bin/env python3
"""Check that the document list table in docs/index.md matches the actual bundle.

index.md is the bundle root, but the table is maintained by hand and nothing read it.
`check_stale_after.py` globs `docs/**/*.md` directly rather than reading the table, so
expiry checking works perfectly well with the table empty and **CI stays green**.
In fact, from the adoption of OKF on 2026-07-30 (`9c94e22`) until 2026-08-04 this
table held only the spec, omitting three plans and known-limitations.md.

It checks three things:

1. every `docs/**/*.md` with frontmatter appears in the table (omissions)
2. every table row points at a real file (ghost rows — typos, moves, deletions)
3. the status and stale_after in the table match that file's frontmatter

Item 3 is the core of this script. With only 1 and 2, the table can exist and still
lie — the file promoted to stable while the table still says draft. That is worse
than having no table at all, for the same reason a stale graph is worse than none.

**What this script does not check** (CLAUDE.md, "presence is not multiplicity"):
a row existing in the table does not mean its link is useful to a person. Whether
the link text or description matches the document's content is not examined — only
the agreement of three values: path, status, and stale_after. And this check
**cannot catch** a plan left at `draft` after merging: if the file and the table
both say `draft`, they agree and it passes. That class belongs to the plan-closing procedure in CLAUDE.md.

Usage: python3 scripts/check_index_complete.py
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INDEX = ROOT / "docs" / "index.md"

STATUS_RE = re.compile(r"^status:\s*(\S+)\s*$", re.M)
STALE_RE = re.compile(r"^stale_after:\s*(\S+)\s*$", re.M)
# | [text](path) ... | type | status | stale_after |
ROW_RE = re.compile(r"^\|\s*\[[^\]]*\]\(([^)]+)\)[^|]*\|([^|]*)\|([^|]*)\|([^|]*)\|")

# How a document with no stale_after is written in the table. Documents with no
# concept of expiry (the Bundle Index, the append-only Update Log) use this.
NO_STALE = {"—", "-", "none"}


def frontmatter(text: str) -> str | None:
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    return text[4:end] if end != -1 else None


def field(fm: str, rx: re.Pattern[str]) -> str | None:
    m = rx.search(fm)
    return m.group(1) if m else None


def main() -> int:
    if not INDEX.exists():
        print(f"FAIL: {INDEX.relative_to(ROOT)} is missing — the bundle root is gone.")
        return 1

    index_text = INDEX.read_text(encoding="utf-8")

    # The actual bundle: every docs/**/*.md with frontmatter
    actual: dict[Path, tuple[str | None, str | None]] = {}
    for path in sorted(ROOT.glob("docs/**/*.md")):
        fm = frontmatter(path.read_text(encoding="utf-8"))
        if fm is None:
            continue
        actual[path] = (field(fm, STATUS_RE), field(fm, STALE_RE))

    # The table: normalize paths relative to index.md into repository-root paths
    listed: dict[Path, tuple[str, str]] = {}
    duplicates: list[str] = []
    for line in index_text.splitlines():
        m = ROW_RE.match(line.strip())
        if not m:
            continue
        target = (INDEX.parent / m.group(1)).resolve()
        if target in listed:
            duplicates.append(str(target.relative_to(ROOT)))
            continue
        listed[target] = (m.group(3).strip(), m.group(4).strip())

    missing = sorted(p for p in actual if p not in listed)
    ghosts = sorted(p for p in listed if p not in actual)

    mismatched: list[str] = []
    for path in sorted(set(actual) & set(listed)):
        fm_status, fm_stale = actual[path]
        row_status, row_stale = listed[path]
        rel = path.relative_to(ROOT)
        if fm_status != row_status:
            mismatched.append(f"  {rel}: status — file {fm_status!r} vs table {row_status!r}")
        norm_stale = None if row_stale in NO_STALE else row_stale
        if fm_stale != norm_stale:
            shown = fm_stale if fm_stale is not None else "none"
            mismatched.append(f"  {rel}: stale_after — file {shown!r} vs table {row_stale!r}")

    failed = False
    if missing:
        failed = True
        print("FAIL: documents with frontmatter absent from the list in docs/index.md:")
        for p in missing:
            print(f"  {p.relative_to(ROOT)}")
        print("\nAdd a row to the table. The bundle root must know the bundle's contents.")
    if ghosts:
        failed = True
        print("FAIL: documents in the table that do not exist (or lack frontmatter):")
        for p in ghosts:
            print(f"  {p.relative_to(ROOT) if ROOT in p.parents else p}")
        print("\nEither a path typo, a moved document, or one created without frontmatter.")
        print("Deleting a document violates the conventions — mark it status: deprecated instead.")
    if duplicates:
        failed = True
        print("FAIL: the same document appears more than once in the table:")
        for d in duplicates:
            print(f"  {d}")
    if mismatched:
        failed = True
        print("FAIL: table values disagree with the document frontmatter:")
        print("\n".join(mismatched))
        print("\nMatch the table to the files. The file is the truth; the table is an index.")

    if failed:
        return 1

    print(f"OK: all {len(actual)} bundle documents match the docs/index.md table")
    return 0


if __name__ == "__main__":
    sys.exit(main())

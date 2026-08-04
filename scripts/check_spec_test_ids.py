#!/usr/bin/env python3
"""Check that the spec's [INV-N] and [TEST-N] IDs exist in test function names.

A mechanism that turns documentation-to-code drift into a build failure. Deleting an
invariant from the spec exposes an orphan test; deleting a test is blocked by this check.

An ID whose item carries `(plan 2)` or `(plan 3)` is not yet in implementation scope and
is skipped. "The same item" is a logical block, not a physical line —
`logical_blocks()` reassembles an item split by line breaks, indentation, or nested
bullets. An ID itself may carry one optional uppercase letter after `-\\d+`
(the `TEST-9B` form) — accepting digits only would make such an ID invisible to this
check entirely and pass silently regardless of whether it exists.

A known limitation: this check looks only at presence — never at the number of test
functions per ID (multiplicity). `test_ids()` returns a `set[str]`, so however many
functions there are, the same ID folds into one entry. Three IDs in plan 1's
traceability table actually map to two test functions each — `TEST-9`,
(`test_9_instructions_are_ordered_by_position_then_title`,
`test_9_ties_across_phases_are_broken_by_id`), `INV-4`
(`inv_4_mark_running_is_conditional`,
`inv_4_append_step_touches_investigation_updated_at`), `TEST-14`
(`test_14_complete_commits_all_three_writes`,
`test_14_partial_failure_rolls_back_everything`). If either member of any of those
pairs is deleted or renamed while the other still matches, this check passes.
"the ID exists in a test name" and "every test that fully verifies what that ID claims
still survives" are different claims, and this script confirms only the former.
No count-enforcing mechanism was built, deliberately — declaring expected counts in
the spec table would make that table a new thing to keep in sync with the code,
creating one more instance of exactly the drift this task exists to prevent.

Usage: python3 scripts/check_spec_test_ids.py
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC_GLOB = "docs/superpowers/specs/*.md"

# ID tokens: TEST-4, INV-4, TEST-9B (one uppercase letter after the digits is allowed).
ID_RE = re.compile(r"\[((?:INV|TEST)-\d+[A-Z]?)\]")
# The deferral marker. With the spec in English the `(plan 2)` form became canonical,
# and `(계획 2)` is kept for compatibility with earlier revisions. Recognizing only one
# would mistake a deferred item for in-scope and fail CI wrongly — at the time of the
# translation zero items were deferred, so this mismatch would have passed silently.
DEFERRED_RE = re.compile(r"\((?:계획|plan)\s*[23]\)", re.I)

# Markdown patterns that begin an item:
#   "1. **Title**"        — the numbered list of Section 12.1
#   "   - **9B. Title**"  — a nested bullet sub-item (TEST-9B under TEST-9, say)
#   "**1. Title**"        — a Section 6.1 invariant (starts bold, no list marker)
ITEM_START_RE = re.compile(r"^\s*(?:-\s|\d+\.\s|\*\*\d+\.)")


def logical_blocks(text: str) -> list[str]:
    """Reassemble an item into one block regardless of physical line breaks.

    A Section 6.1 invariant has a bold title line followed by unindented prose, and a
    Section 12.1 item is a numbered list that sometimes has a nested `-` sub-item
    (`TEST-9B` under `TEST-9`). A blank line always breaks a block. A line inside a
    block that does not begin a new item is appended to the previous one — that is what
    keeps an `[ID]` token and its `(plan N)` marker in the same block even when they are
    reformatted onto different physical lines.
    """
    blocks: list[str] = []
    for line in text.splitlines():
        if not line.strip():
            blocks.append("")  # a paragraph boundary — the next line starts a new block
        elif ITEM_START_RE.match(line) or not blocks or not blocks[-1]:
            blocks.append(line)
        else:
            blocks[-1] += " " + line.strip()
    return [b for b in blocks if b.strip()]


def spec_ids() -> dict[str, bool]:
    """ID to whether it is in implementation scope now."""
    found: dict[str, bool] = {}
    for path in sorted(ROOT.glob(SPEC_GLOB)):
        for block in logical_blocks(path.read_text(encoding="utf-8")):
            deferred = bool(DEFERRED_RE.search(block))
            for m in ID_RE.finditer(block):
                # When an ID appears in several blocks, one in-scope occurrence makes it in-scope
                found[m.group(1)] = found.get(m.group(1), False) or not deferred
    return found


def test_ids() -> set[str]:
    """Extract IDs from test function names. inv_4_foo gives INV-4, test_9b_foo gives TEST-9B.

    It scans all of crates/**/*.rs — not only tests/ but the #[cfg(test)] mod tests in
    src/ as well. TEST-20 lives inside src/steps.rs because it has to call a
    pub(crate) function in mid-transaction.
    """
    ids: set[str] = set()
    name_re = re.compile(r"\bfn\s+((?:inv|test)_\d+[a-z]?)_")
    for path in ROOT.glob("crates/**/*.rs"):
        for m in name_re.finditer(path.read_text(encoding="utf-8")):
            prefix, num = m.group(1).split("_")
            ids.add(f"{prefix.upper()}-{num.upper()}")
    return ids


def id_sort_key(s: str) -> tuple[str, int, str]:
    """A sort key. It orders `TEST-9` and `TEST-9B` by the numeric part first, then by the letter suffix."""
    prefix, num = s.split("-", 1)
    m = re.match(r"(\d+)([A-Z]*)", num)
    assert m is not None
    return (prefix, int(m.group(1)), m.group(2))


def main() -> int:
    spec = spec_ids()
    tests = test_ids()

    in_scope = {i for i, active in spec.items() if active}
    missing = sorted(in_scope - tests, key=id_sort_key)
    orphaned = sorted(tests - set(spec), key=id_sort_key)

    if missing:
        print("FAIL: IDs present in the spec but absent from the tests:", ", ".join(missing))
    if orphaned:
        print("FAIL: IDs present in the tests but absent from the spec:", ", ".join(orphaned))
    if missing or orphaned:
        print(f"\n{len(spec)} spec IDs ({len(in_scope)} in scope), {len(tests)} test IDs")
        return 1

    print(f"OK: all {len(in_scope)} in-scope IDs exist in the tests")
    return 0


if __name__ == "__main__":
    sys.exit(main())

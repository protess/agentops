# agentops — working conventions for agents

Every agent session doing development in this repository **must** follow the
conventions below. They are not optional.

This project manages knowledge (spec, decisions, code) in two layers:

- **OKF v0.2** — the *format* of knowledge. Lifecycle, provenance, and
  verification are declared inside the file. This is the source of truth.
- **graphify** — the *query layer* over that knowledge. `graphify-out/` is a
  regenerable cache and is not truth.

Bundle root and full conventions: [`docs/index.md`](docs/index.md) ·
Change history: [`docs/log.md`](docs/log.md)

> **Commit hashes in this document do not resolve.** The repository's history
> was squashed to a single commit on 2026-08-05, deleting the 169 commits these
> hashes named. They are kept because each still identifies *which* change carried
> a piece of evidence, and removing them would delete that distinction too — but
> `git show <hash>` will fail, and no copy of those commits exists.

---

## MUST — before starting work

**1. Ask the graph first.** Questions about the codebase, architecture, or
relationships between documents are answered from the graph before opening
files.

```bash
graphify query "<question>"
```

If `graphify-out/graph.json` exists, query it — do not rebuild. It is faster
than reading files one at a time, and its community structure surfaces
connections you would not have thought to look for.

**Do not filter the graph by `confidence_score`.** It has two producers on two
different scales: the AST extractor emits `calls` / `indirect_call` at the fixed
constants 0.8 and 0.5, while the LLM extractor obeys a five-value rubric
(`{0.95, 0.85, 0.75, 0.65, 0.55}`). So `confidence_score >= 0.85` — the obvious
way to ask for only the confident answers — **drops every AST-derived `calls`
edge**, which is most of what makes this a call graph. Compare `confidence`
(`EXTRACTED` / `INFERRED` / `AMBIGUOUS`) freely; compare `confidence_score`
only *within* one producer. Measurements and the audit command:
[`known-limitations.md`, section 8](docs/superpowers/known-limitations.md).

**An `AMBIGUOUS` edge is worth tracing, but it does not survive re-extraction.**
It marks a real uncertainty the extractor could not resolve — L17 was found
that way. It is a snapshot of one run, not durable state, so **write what you
find into a document**; the next `--update` will not keep it.

**2. Check the frontmatter of any document you rely on.** Never build an
argument on a document whose `status` is `deprecated`. If a document is past
its `stale_after`, say so to the user and do not treat its contents as fact
until you have checked them against the current code.

---

## MUST — while working

**3. Give every new document OKF frontmatter.** Minimum required fields:

```yaml
---
type: <Design Spec | ADR | Runbook | Reference | ...>
title: <title>
description: <one sentence>
status: draft | stable | deprecated
tags: [<...>]
generated:
  by: <model or person>
  at: <YYYY-MM-DD>
---
```

Specs and design documents **additionally require** `stale_after`. If the
content derives from external material, fill in `sources[]`.

**4. Never delete a file when retiring a document.**

1. Change `status` to `deprecated`
2. Add this document's path to the replacement's `supersedes`
3. Append a line to `docs/log.md`

OKF preserves retired concepts for the sake of their links and history.
Deleting the file destroys the answer to "why did it end up this way".

**5. Record independent reviews in `verified[]`.** `kind` is `human` or
`machine`. The trust level is derived from this: human-reviewed if there is
at least one human entry, machine-confirmed if only machines.

---

## MUST — after finishing work

**6. Append to `docs/log.md`.** It is append-only. Past entries are never
edited; even a correction is added as a new entry.

```
- <verb> — <target> — <evidence / commit>
```

Verbs: `added` / `revised` / `verified` / `deprecated` / `superseded`

**7. Update the graph whenever you add or change documents or code.**

```bash
graphify . --update      # re-extract only new or changed files
```

Use `--update`, not a full rebuild. Cached files are not re-extracted.

**8. When a plan's or spec's implementation merges, close its frontmatter.**

1. `status: draft` → `stable`
2. Record every review received in `verified[]` (one entry each for the final
   review and any re-review)
3. Update the document table in `docs/index.md` — CI checks this one
   (`scripts/check_index_complete.py`)

**Do not strip the reason this item exists.** Plans 1 and 2 got this
treatment in `f58be95`, but that fix was closed as an *instance* and never
promoted to a rule. Plan 3 was written sixteen hours later, was born `draft`,
and stayed `draft` through implementation, review, and merge — because the
audit had already gone by. This is exactly the shape the "a fix cannot see
multiplicity either" section below warns about, and it recurred *after* that
section was written.

**CI cannot catch items 1 and 2.** `check_index_complete.py` only checks that
the table and the files *agree* — if both say `draft`, it passes. No script
can know whether a branch merged. That part is held by human procedure alone.

---

## Do not

- **Never put lifecycle information in `graphify-out/`.** It is a regenerable
  cache. `status`, `stale_after`, and `verified[]` exist only in document
  frontmatter.
- **Never commit `graphify-out/`.** It is in `.gitignore`. Committed, it goes
  stale silently, and a stale graph is worse than no graph.
- **Never edit past entries in `docs/log.md`.**
- **Never cite the graphify graph as evidence that a document is accurate.**
  graphify turns the current files into a graph. If a document lies about the
  code, graphify faithfully puts that lie in the graph.

---

## What is still missing — honestly

**Neither OKF nor graphify detects drift between code and documentation.**
OKF *declares* expiry through `stale_after`; it does not compute it.

The only thing that catches drift is **making it a build failure**. Task 12
introduced the four guards below:

| Guard | What it catches | Status |
|---|---|---|
| `cargo test --doc` | Code examples in documentation that no longer compile | **In place** — the `test` job in `.github/workflows/ci.yml` |
| Integration tests against a migrated DB | SQL and schema drift | **In place** — `cargo test --workspace --all-targets` in the same `test` job (34 `#[sqlx::test]` cases from Tasks 7–11) |
| Spec-to-test ID traceability | IDs are assigned to the design spec's invariants (Section 6.1) and test items (Section 12.1), and CI checks that every spec ID exists in the test suite | **In place** — `scripts/check_spec_test_ids.py`, `drift-guards` job |
| `stale_after` expiry scan | Documents past their revalidation date | **In place** — `scripts/check_stale_after.py`, `drift-guards` job |

**Three of the four run in CI** (`.github/workflows/ci.yml`). The
`drift-guards` job covers spec ID traceability and `stale_after` expiry; the
`test` job covers doc tests. Schema drift is caught by **integration tests
against a migrated database**, not by `cargo sqlx prepare --check` — this
project does not use the `query!` macro, so `--check` verifies nothing (design
spec, Section 7).

The remaining limitation: these guards guarantee that **what the spec claims
and what the tests verify correspond**, not that the tests are correct. An
invariant's ID can appear in a test name while that test verifies the
invariant wrongly. That is what code review is for.

> The first version of this table listed `cargo sqlx prepare --check` in the
> second row. This project does not use the `query!` macro, so that command
> checks nothing — measured: `no queries found`, an empty `.sqlx`, and
> `--check` passing unconditionally. Integration tests catch schema drift
> more broadly; the reasoning is in the design spec, Section 7. **graphify's
> extraction found this contradiction** — it surfaced as a
> `semantically_similar_to` edge showing that CLAUDE.md and the spec's
> Section 7 reached opposite conclusions about the same question.

**An ID-existence check does not prove that a test can fail.** This is the
most important fact in this section. `check_spec_test_ids.py` only checks
whether an ID string is *present* in a test function name — never whether
that test verifies anything. In this plan (Tasks 7–11), `TEST-9` and
`TEST-20` each had a name, passed green, and **verified nothing**:

- `TEST-9` — the original test `test_9_repeated_reads_are_byte_identical`
  passed but did not verify the determinism claim about instruction ordering.
  It was replaced by
  `test_9_instructions_are_ordered_by_position_then_title` and
  `test_9_ties_across_phases_are_broken_by_id`, rewritten so that breaking
  the `position` → `title` → `id` ordering actually fails. Evidence:
  `67cbbb7`, `d660877`
- `TEST-20` — the original test
  `test_20_boot_cleanup_only_touches_locked_ids` passed, and kept passing
  after the `WHERE id = ANY($1)` clause was deleted — because the fixture's
  second investigation never became `running` at any point, so the remaining
  `WHERE status = 'running'` condition already excluded it. It was replaced
  by `test_20_race_interleaving_leaves_newly_running_investigation_untouched`,
  which constructs a real lock/UPDATE interleaving. Evidence: `7f30f43`,
  `d384da7`

In both cases the defect was caught by **mutation testing** — deleting the
guard or removing the condition and confirming the test fails.
`scripts/check_spec_test_ids.py` cannot catch this; ID presence and detection
power are different properties. Mutation testing is **not in CI**. It has only
ever been done by hand during review, and it remains a known limitation.

**Presence is not multiplicity.** This is one level more specific than the
limitation above. `test_ids()` in `check_spec_test_ids.py` returns a
`set[str]`, so when one ID maps to several test functions, the check passes as
long as *any* of them survives — it never counts how many. Three IDs in plan
1's traceability table map to two test functions each: `TEST-9`, `INV-4`, and
`TEST-14`. If either member of any of those pairs is deleted or renamed while
the other remains, the check passes silently. No count-enforcing mechanism was
built, deliberately: declaring expected counts in the spec table would make
that table a new thing to keep in sync with the code, creating one more
instance of exactly the drift this task exists to prevent. This is recorded in
`scripts/check_spec_test_ids.py`'s docstring, along with the measurement that
confirmed deleting one of a pair still passes.

**This is not a property of that script — it is a convention.**
The same shape appeared five more times in plan 2, in ordinary test
assertions, and **two of those hid real defects**. Both were different paths
into the same root bug (two writers writing the terminal row), and both passed
because `steps.iter().any(...)` cannot tell one row from two. Evidence:
`9d27771`, `230c78f`.

> **When asserting on a durable row, count — do not probe for presence.**
> `assert_eq!(steps.iter().filter(...).count(), 1)`, not
> `assert!(steps.iter().any(...))`.

This cannot be enforced by a script — which `any()` should have been a count
is a question of meaning, and `any()` is correct for scanning message contents
or for negative assertions. It functions as a review checklist item.
Audit command: `grep -rn "\.any(\|find_map" crates/*/tests/`

**Some invariants cannot be protected by tests — they are protected by a
structural audit.** Plan 3 confirmed this by measurement: **one writer per
terminal row (F1)** is not catchable by a race test. The implementer ran it
15 times and the reviewer 20 times, independently, and **every run passed
falsely**. The cause is not luck but a structural difference in speed — a
split write takes 2 round trips with no `BEGIN` while an honest transaction
takes 4, so on a local database with near-zero latency the split writer
**always** finishes first and both assertions become trivially true even with
the bug present. A hook would have to already exist at *precisely the point a
future regression would be introduced*, which is circular, and a
query-delaying proxy is flaky by design.

> **Only the transaction that flips the status writes the terminal row.**

Audit command:
`grep -rn "SET status = 'failed'\|SET status = 'completed'" crates/*/src/`

As of 2026-08-04 there are exactly three matches, all inside a transaction —
`complete_investigation` and `fail_investigation` in `artifacts.rs`, and
`terminate_orphans` in `steps.rs` (whose only caller wraps it in
`tx.begin()` / `commit()`). **A new match is a review signal.** Plan 2 found
all three instances of this defect **by review**, not by testing.

**The trap one level above: a fix cannot see multiplicity either.**
When the first of those two defects was fixed, it was **recorded as an
instance and never promoted to a rule**, so the same bug survived on a sibling
path and made it into a second one. The third could not be found by `grep` at
all — the runner never *read* that field; it **independently recomputed** the
same condition.
→ When closing a defect, ask two things together:
  **who reads this field**, and **who recomputes this condition**.
  (There is a third: **what happens when a field nobody reads gets set?**
  That was `PhaseOutcome::terminated`, and it left investigations marked
  `completed` after doing nothing.)

---

## Current state

- Code: a 4-crate workspace — `agentops-core` (domain types and traits, **no
  I/O dependencies**), `agentops-store` (Postgres persistence),
  `agentops-agent` (Anthropic streaming, MCP, the phase loop, the
  investigation runner), `agentops-server` (Axum, SSE, HTMX, `JobManager`,
  watchdog)
- Tests: 263 passing
- CI: `.github/workflows/ci.yml` — four jobs, `fmt` / `clippy` / `test` /
  `drift-guards`
- Spec: `docs/superpowers/specs/2026-07-30-agentops-design.md` — `stable`,
  two independent reviews, `stale_after: 2026-10-30`
- **Plans 1, 2, and 3 are all implemented and merged.** The v0.1 vertical
  slice works.
- **Known limitations and carry-overs live in
  [`docs/superpowers/known-limitations.md`](docs/superpowers/known-limitations.md)**
  — what is open and why, with the reasoning. Read it before follow-up work.

**Run tests with `--test-threads=4`.** `#[sqlx::test]` creates a connection
pool per test, and 19 test binaries running in parallel exhaust Postgres's
`max_connections=100`. At the default parallelism this fails roughly one run
in four, and the error appears as
`Protocol("unexpected response from SSLRequest: 0x00")` **in an unrelated
test**, which makes it look like a code defect.

---

## Writing style for documents in this repository

- **English only.** Every document in this repository is written in English.
- **Never use the `§` symbol.** Write "Section 6.1", or name the section.
- Prefer Markdown with YAML frontmatter. Give every code fence a language tag.
- Keep headings frequent enough that an agent can navigate by them and cite
  them.

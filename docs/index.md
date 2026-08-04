---
type: Bundle Index
okf_version: "0.2"
title: agentops knowledge bundle
description: Root of the OKF bundle that holds agentops's spec, decisions, and code knowledge
status: stable
tags: [okf, index]
generated:
  by: claude-opus-5
  at: 2026-07-30
---

# agentops knowledge bundle

This directory is an [OKF v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
bundle. Every `.md` file declares its own provenance, verification, and
lifecycle in YAML frontmatter.

## Why OKF

We needed a format that treats the **lifecycle** of knowledge as a
first-class concern. This project expects more than ten specs to arrive in
sequence, some of which will supersede or retire others (design spec,
Section 16). "What is currently true" cannot live in memory outside the
files.

Why OKF specifically:

- **No dependencies.** Markdown plus YAML frontmatter. Readable with `cat`,
  distributable with `git clone`.
- **Lifecycle is first-class.** `status: draft|stable|deprecated`,
  `stale_after`.
- **Provenance and verification are first-class.** `sources[]`, `generated`,
  `verified[]` — the trust level is derived from whether a reviewer was human
  or machine.
- **The file is the source.** The committed file is the truth, not a
  generated artifact.

## Document list

| Document | Type | Status | stale_after |
|---|---|---|---|
| [superpowers/specs/2026-07-30-agentops-design.md](superpowers/specs/2026-07-30-agentops-design.md) | Design Spec | stable | 2026-10-30 |
| [superpowers/plans/2026-07-30-foundation.md](superpowers/plans/2026-07-30-foundation.md) | Implementation Plan | stable | 2026-10-30 |
| [superpowers/plans/2026-07-30-agent.md](superpowers/plans/2026-07-30-agent.md) | Implementation Plan | stable | 2026-10-30 |
| [superpowers/plans/2026-08-03-web.md](superpowers/plans/2026-08-03-web.md) | Implementation Plan | stable | 2026-11-03 |
| [superpowers/known-limitations.md](superpowers/known-limitations.md) | Reference | stable | 2026-11-04 |
| [frontend-assets.md](frontend-assets.md) | Runbook | stable | 2027-02-04 |
| [log.md](log.md) | Update Log | stable | — |
| [index.md](index.md) (this document) | Bundle Index | stable | — |

This table is maintained by hand, and `scripts/check_index_complete.py`
verifies it against the filesystem in CI. Note what the older guards do
*not* do: `check_stale_after.py` globs `docs/**/*.md` directly, so expiry
checking works perfectly well with this table empty — **a stale table keeps
CI green.** It did exactly that from the day OKF was adopted (2026-07-30)
until 2026-08-04, listing only the spec while three plans and
`known-limitations.md` were missing. When you add a document, add a row here.

## Conventions

### Required frontmatter

OKF itself requires only `type`. This project **additionally requires**:

| Field | Why |
|---|---|
| `type` | OKF requirement |
| `title` / `description` | Needed for listing and search |
| `status` | The basis for any lifecycle judgment |
| `generated: {by, at}` | Who made it, and when |
| `tags` | Cross-cutting classification |

`stale_after` is required for specs and design documents, optional for
reference material.

**Comments inside frontmatter must occupy a whole line.** An inline comment
after a value (`status: draft  # ...`) breaks the guards' regexes.
`^status:\s*(\S+)\s*$` fails to match, the value becomes `None`, and when
`check_stale_after.py` cannot read `stale_after` it **skips that document
silently and reports OK** — expiry checking is switched off entirely while CI
stays green. This was measured, not assumed. The only guard that catches it
is `check_index_complete.py`, and it catches it incidentally, because the
parsed value disagrees with the table. A check that parses one source cannot
notice that its own parse failed; only a check that compares two sources
survives.

### Writing a new plan document

A plan is born `draft` and must be closed when it merges — but **the person
who has to remember that is not the person who wrote the plan.** It is a
different session, weeks later, opening a file that says `status: draft` and
nothing else. Plan 3 was missed exactly this way: it was written sixteen
hours after the audit that closed plans 1 and 2 (`f58be95`), and it stayed
`draft` through implementation, review, and merge.

So the procedure lives **inside the file**, not in anyone's memory. A new
plan's frontmatter starts like this:

```yaml
---
type: Implementation Plan
title: <title>
description: <one sentence>
# On merge: promote to stable and record every review received in verified[].
# Update the index table in docs/index.md too — CI checks that one.
# Full procedure: CLAUDE.md, "MUST — after finishing work", item 8.
status: draft
tags: [plan, ...]
stale_after: <YYYY-MM-DD>
sources:
  - resource: <path to the spec this plan implements>
    author: <...>
    last_modified: <YYYY-MM-DD>
generated:
  by: <model or person>
  at: <YYYY-MM-DD>
supersedes: []
---
```

The three comment lines go **directly above** `status: draft`, never after
the value — see the inline-comment warning above.

This only prevents "nobody knew it had to be closed." It does not prevent
knowing and skipping it, and no script can tell that a branch merged. That
part is held by CLAUDE.md convention 8, as a human procedure.

### Retiring a document

**Never delete the file.** OKF preserves retired concepts for the sake of
their links and history.

1. Change `status` to `deprecated`
2. If there is a replacement, add this document's path to that document's
   `supersedes`
3. Append a line to `log.md`

### Recording verification

When a document receives an independent review, add an entry to `verified[]`.
`kind` is either `human` or `machine`. OKF derives the trust level from this:
human-reviewed if there is at least one human entry, machine-confirmed if
only machines, unverified if the list is empty.

### When `stale_after` has passed

Re-read the document and check it against the current code. If it still
holds, extend `stale_after` and add a `verified[]` entry. If it does not,
update it or retire it.

## What is still missing — honestly

**Neither OKF nor graphify detects drift between code and documentation.**
OKF *declares* expiry through `stale_after`; it does not compute anything.
graphify turns the current files into a graph — if a document lies about the
code, graphify faithfully puts that lie in the graph.

The only thing that actually catches drift is **making it a build failure**.
Four guards now run in CI:

| Guard | What it catches | Where |
|---|---|---|
| `cargo test --doc` | Code examples in documentation that no longer compile | `test` job |
| Integration tests against a migrated DB | SQL and schema drift | `test` job |
| `scripts/check_spec_test_ids.py` | Spec invariant and test IDs that no longer exist in the test suite | `drift-guards` job |
| `scripts/check_stale_after.py` | Documents past their revalidation date | `drift-guards` job |
| `scripts/check_index_complete.py` | This index disagreeing with the filesystem | `drift-guards` job |

Schema drift is caught by the integration tests, **not** by
`cargo sqlx prepare --check`. This project does not use the `query!` macro,
so that command finds no queries, writes an empty `.sqlx`, and passes
unconditionally — it checks nothing. The reasoning is in the design spec,
Section 7.

What none of these guarantee is that a test which carries an invariant's ID
actually verifies that invariant. ID presence and detection power are
different properties. That is what code review is for.

## graphify

Knowledge-graph queries go through
[graphify](https://github.com/Graphify-Labs/graphify). Its output lives in
`graphify-out/` and is a **regenerable cache** — never a source of truth, so
no lifecycle information belongs there.

```bash
graphify . --update        # re-extract only new or changed files
graphify query "question"  # query the graph
```

---
type: Reference
title: agentops v0.1 — known limitations and carry-overs
description: Items deliberately deferred or left unclosed while executing plans 1 through 3, each with its reasoning
status: stable
tags: [limitations, triage, followup, v0.1]
stale_after: 2026-11-04
sources:
  - resource: .superpowers/sdd/2026-08-03-web/final-review.md
    author: claude-opus-5
    last_modified: 2026-08-04
    note: Plan 3's final whole-branch review. That directory is gitignored scratch, so this document preserves its content.
generated:
  by: claude-opus-5
  at: 2026-08-04
verified:
  - kind: machine
    by: claude-opus-5 (plan 3 final whole-branch review plus two fix rounds)
    at: 2026-08-04
    scope: 30 commits, 6,900 lines. Found 1 Critical and 4 Important and fixed all of them; the re-review found N-1, which was also fixed.
    result: Ready to merge. The items below are **deliberately left open**, each for a stated reason.
supersedes: []
---

# Known limitations and carry-overs

These are the items that were **deliberately deferred or could not be closed**
while executing plans 1 through 3. Each one records why it was left open —
nothing here is open by accident.

The review artifacts under `.superpowers/sdd/` are gitignored scratch, so
**this document is the durable copy** of what they said.

---

## 1. What the tests do not protect

The code in this section is **correct today**. What is missing is a mechanism
that would catch a regression.

| # | What | Why it is still open |
|---|---|---|
| L1 | Reverting `chart.rs`'s SQL to an instant-based window leaves every test passing | The defensive drop in `daily_bars` makes the rendered output identical, and no test captures tracing output from the real HTTP path. Capturing logs across worker threads risks being more flaky than it is worth. |
| L2 | Reverting the 404/500 split in `investigation_detail` leaves every test passing | Provoking a non-`NotFound` error over HTTP at this call site would require making `AppState` and `router` generic over `Store`. Too high a price for a one-line mapping. |
| L3 | **F1 (one writer per terminal row) cannot be protected by a race test** | 15 runs by the implementer and 20 by the reviewer all passed falsely. A split write takes 2 round trips with no `BEGIN`; an honest transaction takes 4. On a local database with near-zero latency the split writer **always** finishes first, which makes both assertions trivially true. **Replaced by a structural audit** (section 4 below). |
| L4 | No test pins `CAPACITY = 256` in `bus.rs` | `TEST-3` (Lagged) in Task 3 depends on the real value indirectly, so the practical risk is low. |
| L5 | The no-CDN test only scans the `/incidents` HTML body | Adding `@import url(...)` to `app.css` still passes — measured. **Scanning static assets for host strings would itself create a new drift surface**, so this stays a review checklist item rather than a CI gate. |
| L6 | `serve_shutdown.rs` does not distinguish C-1's two mechanisms (`select!` cancellation vs `abort_all`) | Removing only the `select!` still passes, because stage 5's `abort_all` kills the producer anyway (5.28s versus 0.21s when healthy). **The production shutdown deadline is 30 seconds**, so if someone deletes the `select!` as "redundant", **every shutdown takes 30 seconds whenever a single tab is open — and the test stays green.** To strengthen it: give the shutdown a generous deadline and set the test timeout below it. |
| L7 | No test protects the `ToolCall` branch of `step_html` (`tool_use_id`, `tool`) | The other five `StepKind` variants are covered. Those two values are an Anthropic-issued ID and a tool name, so the exposed surface is narrow. |

## 2. Structurally open

| # | What | Impact today |
|---|---|---|
| L8 | The agreement between `now_utc()` (app clock) and `now()::date` (DB clock) is **accidental** | `SHOW timezone` returns `UTC`, but nothing in the code or the connection setup enforces it. A change to the pooler or the connection string could shift the chart window by **a full day**. |
| L9 | If a client disconnects from an in-flight but idle investigation, the producer task parks until the next step or `Terminal` | **Bounded and self-resolving** through the INV-4 watchdog or normal completion. |
| L10 | `DEFAULT_SESSION` is a hardcoded constant (`Uuid::from_u128(1)`) | Session creation is out of scope for v0.1. A drift-guard test ties the HTML literal to the Rust constant. **Whoever introduces session creation must also build the reclamation path for the chat channel map.** |
| L11 | The health-check DB query has no timeout | It relies solely on the pool's default `acquire_timeout` of 30 seconds. |
| L12 | The deadline check in `jobs.rs::wait_idle` only exists on the `None` branch | While `try_join_next` keeps returning `Some`, the function will not return even past the deadline. Bounded in practice because the task count is finite. |
| L18 | **The server does not apply migrations at startup** | `main.rs` connects the pool and goes straight to `recover_on_boot`, which queries `investigations`; against an empty database that fails with `relation "investigations" does not exist`. **No test could have caught it**: `#[sqlx::test]` creates and migrates a throwaway database per test, so the suite never meets a hand-made one — the only path that does is a human following the README. Measured 2026-08-05 against a fresh database. The schema must be applied by hand (`psql -f migrations/0001_initial.sql`, or `sqlx migrate run`); the README's Migrations section documents both routes. Whether the server should migrate itself is **an open design question, not a decided omission** — doing it automatically on a shared database is a footgun, and the alternative is to fail with a clear message naming the fix rather than a raw relation error. |
| L13 | `shutdown_with_hook` and `ShutdownHook` are `pub` without a test-only marker in the name | `stream.rs::collect_stream_for_test` is obvious at the call site. The doc comments state the intent. |
| L17 | **M2 — `append_step` still succeeds after an investigation has terminated** | The store has no status guard: `append_step` after `fail_investigation` returns `Ok`, and the row is durable. **No user-visible harm today**, and that has been checked rather than assumed (see below). Plan 3 was instructed to pin the behavior and not change it, because a status guard is a change to plan 1's contract and touches the ordering of three writers — the watchdog, shutdown, and the runner. Pinned by `m2_appending_to_a_terminal_investigation` (`crates/agentops-server/tests/watchdog.rs:318`); **if that assertion flips, the behavior changed.** |

### L17 in detail — why it is harmless today, and where a fix belongs

This is the one carry-over from plan 2's final review that was measured and
then **left open on purpose**, so the reasoning has to survive here.

**Why no harm today.** `Store::steps_after` has no status filter —
`WHERE investigation_id = $1 AND seq > $2 ORDER BY seq`
(`crates/agentops-store/src/steps.rs:176`) — so a post-termination row is
returned like any other. It has exactly two readers:

| Reader | What it does with a post-termination row |
|---|---|
| `investigation_stream` (`src/stream.rs:199`, resync at `:286`) | Replays it, **then** closes: `close_if_investigation_is_terminal` runs *after* replay (`:226`), not before. A row that appears after this connection has replayed is never delivered — but this connection is already closing, and the next one replays it. |
| `routes::pages::investigation_detail` (`src/routes/pages.rs:104`) | Renders it, and takes `after` from **the rendered list** (`:108`, `steps.last()`), not from `max_step_seq`. |

Because the page derives `after` from what it actually rendered, the two views
cannot disagree about where the stream should resume. That — not the absence
of the row — is what keeps this benign.

The watchdog is also unaffected: `stale_running_ids` filters
`WHERE status = 'running'` (`agentops-store/src/investigations.rs:171`), so the
`updated_at` these appends keep advancing is never read for a terminated row.

**Where a fix belongs, if one is ever made.** In `append_step`, not in
`steps_after`. Filtering on read would hide legitimate pre-termination rows
from whichever reader got the filter, and would leave the cause — that the
write is permitted — untouched. Applying section 5's questions: **two readers,
zero recomputations, one write path.**

## 3. Required by the spec, not implemented in v0.1

| # | Spec | Why |
|---|---|---|
| L14 | Section 10.2 — **search** on the investigation list | `ListFilter` has no search field. Faking it client-side interacts with pagination and produces wrong results — **the store contract has to change first**. |
| L15 | Section 10.2 — **LLM and MCP status** in `/api/health` | Only the database is checked today. |
| L16 | Section 6.1 item 6 — **`Last-Event-ID`** taking priority over `after` | The spec itself defines this as a best-effort enhancement. The `after`-based replay is the primary path and works correctly. |

## 4. F1 is protected by a structural audit

**A terminal row is owned by the transaction that flips the status.** Plan 2
met this defect three separate times in three different places, and plan 3
confirmed that a race test cannot catch it (L3).

The audit command:

```bash
grep -rn "SET status = 'failed'\|SET status = 'completed'" crates/*/src/
```

As of 2026-08-04 there are exactly three matches and all three are inside a
transaction: `artifacts.rs` (`complete_investigation`), `artifacts.rs`
(`fail_investigation`), and `steps.rs` (`terminate_orphans`, whose only caller
wraps it in `tx.begin()` / `commit()`).

**A new match is a review signal.**

## 5. Three questions to ask when closing a defect

These came out of the shape plans 2 and 3 kept losing to. Each question
caught a real defect.

1. **Who reads this field?**
2. **Who recomputes this condition?** — tracing the field's data flow will not
   reveal an independent re-derivation.
3. **What happens when a field nobody reads gets set?** — this was
   `PhaseOutcome::terminated`, and it left investigations marked `completed`
   after doing nothing.

There is a question one level above those: **how many other instances of this
defect are there?** Recording a fix as an instance without promoting it to a
rule means meeting it again on a sibling path — in plan 3, **the very commit
that documented the trap** missed one of the three call sites (N-1).

## 6. "Reporting success while doing nothing"

This repository has met this shape **four** times. The first three are
commands: all three **exit 0**, and all three are only visible **if you inspect
the output**.

| What | Succeeds while |
|---|---|
| `cargo sqlx prepare --check` | checking nothing at all (the `query!` macro is not used) |
| `check_spec_test_ids.py` | reporting OK over an empty scope (during the window when plan 2 and 3 markers were attached) |
| The Tailwind `-c` flag | being silently ignored — it does not exist in v4 — producing CSS with no utility classes in it |

The response to those three: pin the version, and inspect the artifact
immediately after the build (`grep -q '\.text-xl{'`). **Pinning alone is not
enough** — it will fail silently again the day someone raises the pin.

**The fourth is not a command — it is a handoff, and that is what makes it
worth recording separately.** M2 (now L17) was deferred three times in a row,
and each deferral reported success:

| Where | What it says |
|---|---|
| `docs/superpowers/plans/2026-08-03-web.md:2406` | "**Do not decide in this task whether to change the behavior.** … Pin the current behavior with a test and hand it to the final review's triage." |
| `docs/log.md:939` | "the behavior was not changed; the current behavior was pinned … and **triage handed to the final review**" |
| `crates/agentops-server/tests/watchdog.rs:318` | "The behavior is not changed. The current behavior is pinned by this test and **handed to the final review's triage**." |

All three are accurate about what they did. **None of them was wrong.** But the
receiving end — this document, whose stated job is "what is open and why" — had
no M2 entry until 2026-08-04, when it was found by tracing an `AMBIGUOUS` edge
in the graph rather than by any procedure. Its sibling carry-overs make the
omission legible: F1 arrived (L3 plus section 4), and G1, G2, G4, I5, and M1 are
absent **because plan 3 closed them**. M2 was absent while still open.

Compare the plan-3 `draft` frontmatter incident recorded in CLAUDE.md: an item
created after the audit had already passed by. It is the same failure, one layer
up — **a correct handoff with no arrival check.** No script can catch it:
`check_index_complete.py` verifies that the table and the files agree, and they
did; nothing knows that a document is missing a row it never had.

> **When a task defers a decision, name the document the decision lands in —
> and check that it arrived.** "Handed to triage" is a claim about a
> destination, so it is only true if the destination can be pointed at.

## 7. Mutation testing is not in CI

This has been a limitation since plan 1 and it remains one. It is done by
hand, during review, and nowhere else.

In plan 3, **seven tests that the plan document specified verbatim had no
detection power, and all seven were caught by mutation testing.** The
structural cause is that when a plan is written there is no code yet, so no
test in it has ever been run. **Mutation testing is the only defense that has
actually worked in this project.**

Execution order matters here: **run the existing test first, unchanged**, and
observe that the mutation breaks nothing — then build the replacement. In the
other order you learn only that the new test catches it, and never learn
**that the old one did not.**

## 8. The graph's confidence scores come from two producers with different scales

This is a property of **graphify**, not of this repository's code. It is
recorded here because CLAUDE.md's first convention is *ask the graph before
opening files*, and because filtering that graph by `confidence_score` — the
obvious way to ask it for only its confident answers — silently returns the
wrong set.

`graphify-out/graph.json` is built by two independent producers, and they do
not share a scale:

| Producer | Relations it emits | Scores it uses |
|---|---|---|
| The AST extractor (deterministic, no LLM) | `calls`, `indirect_call` | Two fixed constants: **0.8** and **0.5** |
| The semantic extractor (LLM) | `conceptually_related_to`, `semantically_similar_to`, `implements`, `references`, `rationale_for`, `shares_data_with` | The five-value rubric `{0.95, 0.85, 0.75, 0.65, 0.55}` |

Measured on the 2026-08-04 build (2,121 nodes, 4,355 edges): 4,174 `EXTRACTED`
at 1.0, 180 `INFERRED`, 1 `AMBIGUOUS`. Of the 180 `INFERRED`, **102 are outside
the rubric** — 74 `calls` at 0.8, and 28 `indirect_call` at 0.5 (27 of those
from the vendored `htmx.min.js`). The split is exact: every off-rubric edge is
`calls` or `indirect_call`, and every rubric-conforming score belongs to a
semantic relation.

The extraction prompt forbids 0.5 explicitly, and states the reason — a
bimodal collapse of the rubric to a binary. **That prohibition binds the LLM
extractor, which the prompt reaches; it does not bind the AST extractor, which
never reads it.** So the graph is not disobeying its own rule so much as
carrying two rules.

**Consequences when querying:**

- `confidence_score >= 0.85` excludes **every AST-derived `calls` edge**, which
  is most of what makes the graph a call graph at all.
- 0.5 does not mean "half sure". On an `indirect_call` it is that extractor's
  only value for that relation — it carries no gradation.
- Compare `confidence` (`EXTRACTED` / `INFERRED` / `AMBIGUOUS`) across
  producers, and `confidence_score` only **within** one producer.

Audit command:
`python3 -c "import json,collections;E=json.load(open('graphify-out/graph.json'))['links'];print(collections.Counter((e.get('confidence_score'),e.get('relation')) for e in E if e.get('confidence')=='INFERRED'))"`

**How this was found is the part worth keeping.** The last extraction agent
reported that "every `confidence_score` obeys the tiering", and that report was
true — of its own chunk. The verification that followed checked the same chunk.
Both were honest, and **neither looked at the 102 AST edges**, because the
scope of the check was narrower than the scope of the claim and nothing said
so. That is section 6's shape again, at the tooling layer: the same window in
which `check_spec_test_ids.py` reported OK over an empty scope.

> **A verification is only as broad as what it enumerated.** State the
> denominator, or the pass means nothing.

### A related property: `AMBIGUOUS` edges are not stable across extractions

That 1 `AMBIGUOUS` edge (`M2 --conceptually_related_to--> Store::steps_after`,
0.3) is what led to L17. A second one existed in the 2026-08-04 morning build,
linking L8 to the schema's `CHECK` constraints; re-extracting
`known-limitations.md` that afternoon replaced it with
`L8 --conceptually_related_to--> Chart window derived from a single constant`
at `INFERRED` 0.85. The new judgment is better — L8 is about clock skew and has
nothing to do with `CHECK` constraints — but nothing recorded that the old edge
had ever existed.

`AMBIGUOUS` marks a genuine uncertainty and is worth tracing, but it is **a
snapshot of one extraction run, not durable state**. Anything found by tracing
one has to be written into a document, as L17 was; the graph will not keep it.

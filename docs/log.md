---
type: Update Log
title: agentops knowledge bundle change history
description: History of additions, revisions, verifications, and retirements of documents in the bundle (append-only)
status: stable
tags: [okf, log]
generated:
  by: claude-opus-5
  at: 2026-07-30
---

# Change history

**Append-only.** Past entries are never edited. Even a correction is added as a
new entry.

Each entry: `verb — target — evidence/commit`

Verbs: `added` / `revised` / `verified` / `deprecated` / `superseded`

> **On the 2026-08-04 translation.** Every entry below was originally written
> in Korean and was translated to English in one pass, together with the rest
> of the bundle. That rewrote the bytes of past entries, which append-only
> normally forbids; it was done on explicit instruction and is recorded as its
> own entry at the end. Technical content — commit hashes, measurements, counts
> — was carried across unchanged. The section headings are also new: this file
> previously had a single heading for 86 entries, which made it unnavigable.

> **Commit hashes in this document do not resolve.** The repository's history
> was squashed to a single commit on 2026-08-05, deleting the 169 commits these
> hashes named. They are kept because each still identifies *which* change carried
> a piece of evidence, and removing them would delete that distinction too — but
> `git show <hash>` will fail, and no copy of those commits exists.

---

## Spec and knowledge bundle

- `added` — `superpowers/specs/2026-07-30-agentops-design.md` — first version of
  the agentops v0.1 design document. Established that this is an agent-first
  product rather than a monitoring dashboard, revising seven conclusions from
  the original brief. `cd12e35`
- `verified` — `superpowers/specs/2026-07-30-agentops-design.md` — first
  independent review (fable). API facts, architectural invariants, schema,
  scope. Twenty findings accepted including two Critical (`Last-Event-ID` is not
  sent; the subscribe-replay race); one rejected (the `rmcp` version — refuted
  against the crates.io API). `6d14739`
- `verified` — `superpowers/specs/2026-07-30-agentops-design.md` — second
  independent review (codex). Rust feasibility, concurrency, protocol round
  trips. Reached the same seven defects as the first review independently. Six
  new P1s accepted (missing `tool_use_id`, no loop limits at all, when
  `stop_reason` arrives, terminal transactionality, the shutdown race, a false
  idempotency claim); one rejected (the composition-root pattern). `69985aa`
- `added` — `index.md`, `log.md` — adopted the OKF v0.2 bundle. The alternative,
  graphify, is a tool rather than a format and has no concept of lifecycle,
  making it unsuitable as the foundation for knowledge management. graphify is
  adopted alongside, as the query layer.
- `revised` — `superpowers/specs/2026-07-30-agentops-design.md` — added OKF
  frontmatter. The verification history in Section 19 was promoted into the
  `verified[]` field.
- `added` — `CLAUDE.md`, `.gitignore` — codified the agent development
  conventions. Before work (query the graph, check frontmatter), during
  (attach frontmatter, retirement procedure, record verification), and after
  (append to the log, `graphify --update`) are all MUST items. `graphify-out/`
  is a regenerable cache, so it is gitignored.
- `verified` — the whole bundle — built the initial graphify graph. 92 nodes /
  204 edges / 8 communities. Edge confidence: 194 EXTRACTED, 12 INFERRED, 0
  AMBIGUOUS. Graph health: 2 undirected merged edges (warning).

## Plan 1 — foundation

- `verified` — `superpowers/specs/2026-07-30-agentops-design.md` — third review
  (codex, targeting plan 1). Plan defects flowed back as spec defects: one P0
  (two paths allocate `seq`, so the terminal step is silently lost), three P1s,
  two P2s. Section 6.1.1 was rewritten entirely — the database is the sole `seq`
  authority. Two items reversed direction, from "the plan is wrong" to "the spec
  is wrong".
- `revised` — `superpowers/plans/2026-07-30-foundation.md` — applied the fourth
  review (fable, actual compilation, plus codex, design). One blocker (Task 7
  could not compile at its own verification step — the Self-Review's claim of
  "resolved" was false) and five majors. Moved `seq` allocation to the database,
  moved `fail_orphaned_running` to Task 8, fixed a `TerminalReason` newtype
  variant serialization panic, rewrote TEST-13 and TEST-14 with real concurrency
  and error injection, added TEST-19 and TEST-20, and removed a vacuous
  sqlx-offline CI job. My own claim that `serde_json` does not implement `Eq`
  was refuted.
- `verified` — the whole bundle — incremental graphify update (`--update`).
  92→262 nodes, 204→514 edges, 8→10 communities. 213 new nodes, 43 removed. The
  extraction found the contradiction between CLAUDE.md and the spec's Section 7
  (`cargo sqlx prepare --check`).
- `revised` — `CLAUDE.md` — corrected the schema-drift row of the drift-guard
  table from `cargo sqlx prepare --check` to integration tests. Found by
  graphify.
- `verified` — `superpowers/plans/2026-07-30-foundation.md` — fifth review
  (fable, confirming the fixes compile). Three majors: `StepKind` imports
  missing in two places (dropped when the trait signature changed), and
  `rust-version` 1.88→1.94 (required by sqlx 0.9.0 itself). Test counts
  corrected. Final state: 20 core plus 34 store tests passing, all
  clippy/fmt/doc/drift gates passing, concurrency 6/6.
- `added` — `.github/workflows/ci.yml`, `scripts/check_spec_test_ids.py`,
  `scripts/check_stale_after.py` — introduced four drift guards in CI. Because
  it checks that the spec's `[INV-N]` and `[TEST-N]` IDs exist in test function
  names, a mismatch between documentation and code becomes a build failure.
- `revised` — `superpowers/specs/2026-07-30-agentops-design.md` — assigned
  traceability IDs to the Section 6.1 invariants and the Section 12.1 test
  items.
- `revised` — `CLAUDE.md` — updated the "what is still missing" section to
  reflect the guards now being in place, and stated, on the evidence of `TEST-9`
  and `TEST-20` passing under their names while verifying nothing, that "an
  ID-existence check does not prove that a test can fail." Mutation testing is
  left recorded as a limitation not present in CI. Updated "current state" from
  no code to the actual state (2 crates, 58 tests).
- `revised` — `superpowers/specs/2026-07-30-agentops-design.md` — corrected item
  9 of Section 12.1 (one review finding). `TEST-9` claimed coverage that was
  never verified — byte-level determinism of prompt and `tools[]` assembly,
  which lives in the agent layer, plan 2. What Task 9 actually implemented was
  only its precondition, the determinism of instruction ordering. The item was
  split in two: `TEST-9` (instruction ordering determinism, plan 1 scope) and
  `TEST-21` (prompt and `tools[]` assembly determinism, marked plan 2 and
  excluded from the check). `TEST-9B` was tried first as an ID suffix, but
  mutation testing confirmed that the ID regex in `check_spec_test_ids.py`
  accepted only `\d+` and therefore silently failed to parse any ID with a
  letter suffix, so a purely numeric ID (`TEST-21`) was used instead.
- `revised` — `scripts/check_spec_test_ids.py` — two review findings applied.
  (1) The ID regex accepted digits only, so an ID with a letter suffix such as
  `TEST-9B` was silently unparseable no matter where it was placed — widened to
  `\[((?:INV|TEST)-\d+[A-Z]?)\]`, with the test-function-name regex and the sort
  key fixed to match. (2) The check operated on single physical lines —
  `logical_blocks()` was added to reassemble an item into one logical block
  regardless of indentation, nested bullets, or line breaks, so an `[ID]` token
  and its `(plan N)` marker still count as the same item after reformatting.
  Verified by temporarily inserting `TEST-9B` into the real spec in three
  shapes: with a marker at column 0, without a marker but indented and
  bulleted, and without a marker — the third being precisely the case that used
  to pass silently.
- `revised` — `superpowers/specs/2026-07-30-agentops-design.md` — since the
  previous correction means letter-suffixed IDs are no longer invisible to the
  scanner, `TEST-21` was reverted to `TEST-9B`. That name reflects the original
  structure (a nested sub-item under `TEST-9`), and the reason for choosing
  `TEST-21` (invisibility to the scanner) no longer applies. The test
  requirement is unchanged — it is still marked as deferred and excluded from
  the check.
- `revised` — `scripts/check_spec_test_ids.py` — added a known limitation to the
  docstring: this check looks only at ID presence. When one ID like `TEST-9`
  maps to two test functions
  (`test_9_instructions_are_ordered_by_position_then_title` and
  `test_9_ties_across_phases_are_broken_by_id`), the check verifies only set
  membership, so deleting or renaming one of them passes as long as the other
  survives. Confirmed by actually removing the `test_9_` prefix from
  `test_9_ties_across_phases_are_broken_by_id` and running it —
  `check_spec_test_ids.py` still printed `OK`. This is not a false negative; it
  is an honest record of something this check does not do (verifying the
  multiplicity of tests bound to one ID).
- `revised` — `scripts/check_spec_test_ids.py`, `CLAUDE.md` — review applied.
  Expanded the multiplicity note to name all three IDs in plan 1's traceability
  table that actually map to two test functions: `TEST-9`, `INV-4`
  (`inv_4_mark_running_is_conditional` /
  `inv_4_append_step_touches_investigation_updated_at`), and `TEST-14`
  (`test_14_complete_commits_all_three_writes` /
  `test_14_partial_failure_rolls_back_everything`). Added the same limitation to
  CLAUDE.md's "what is still missing" section as a more specific paragraph
  beside "an ID-existence check does not prove that a test can fail." No
  count-enforcing mechanism was built, deliberately — declaring expected counts
  in the spec table would make that table a new drift target requiring
  synchronization with the code.
- `revised` — `scripts/check_stale_after.py` — when a `stale_after` value passed
  the `YYYY-MM-DD` digit regex but was not a real calendar date (such as
  `2026-13-01`), `dt.date.fromisoformat` raised an unhandled `ValueError` and
  died with a traceback. CI still failed (the build was blocked), but the cause
  looked like "the tool is broken." Wrapped in `try/except ValueError` and
  turned into a `FAIL:` that names the offending document and value, keeping
  exit code 1. All three cases — invalid date, expired date, and a healthy tree
  — confirmed by mutation testing.

## Plan 2 — agent layer

- `added` — `superpowers/plans/2026-07-30-agent.md` — wrote plan 2 of 3 (the
  agent layer). Twelve tasks: LLM protocol types, the SSE parser, the stream
  state machine, stop_reason branching, the Anthropic provider, store extensions
  (MCP and policy), the policy gate, the MCP registry, prompt assembly, loop
  limits, the phase loop, and the investigation runner. Covers Sections 5, 8,
  and 9 of the spec; Section 10 (HTTP), INV-1 through INV-3, and `JobManager`
  are left to plan 3. Placed the two items plan 1 deferred (mixed clocks in
  `stale_running_ids`, error flattening in `backend()`) into Task 6. Because the
  rmcp 3.0.1 API could not be verified, the surface touching rmcp was isolated
  into a single `RmcpConnection` type and its contract pinned in a table —
  guessed signatures do not go into a plan. The Self-Review found and fixed two
  placeholders (a stub attribute that does not exist, and an `unimplemented!()`
  skeleton with a requirements table) and three type mismatches (the argument
  count of `assemble_system`, the access path of `INVESTIGATION_ORDER`, and
  `PhaseCtx` holding `&mut Budget` while being taken as `&PhaseCtx`, which
  cannot compile).
- `verified` — `superpowers/plans/2026-07-30-agent.md` — first cross-check
  (fable, actual compilation). Copied the workspace outside the repository,
  transcribed the code of all twelve tasks, and ran `cargo check --workspace
  --all-targets` plus the tests that need no database (rustc 1.94.0). Two
  blockers: Task 11 does not import `MAX_TOKENS` (E0425), and Task 5's test
  calls `expect_err` on a `BoxStream` (E0277 — no Debug). Zero design-level type
  errors — the `bytes_stream().flat_map(move ...)` capture change the controller
  most suspected is fine, and even the live-socket test passes. Four minors
  fixed (2 orphan imports, 1 unused import, `clippy::await_holding_lock` in two
  places). Verified `rmcp` 3.0.1, `reqwest 0.12` features, and the
  `futures-util 0.3` API against crates.io. Corrected one error in the
  controller's brief (core tests are 20 at HEAD, not the 24 stated).
- `verified` — `superpowers/plans/2026-07-30-agent.md` — second cross-check
  (codex, design and protocol). **One P0: the seventh limit in the spec's
  Section 5.4, the 120-second stream idle timeout, existed in `Limits` only as a
  field, a default, and a value assertion — nothing anywhere fired it.** The
  evidence was that plan 1 created `LlmError::IdleTimeout` for exactly this and
  it was never used. On a stalled connection an investigation hangs
  indefinitely past the 30-minute wall clock — `check_wall_clock()` is called
  only at the start of a turn. Two P1s: `ToolCall` steps are appended
  immediately during streaming, so they remain in the database unpaired on the
  tool-budget-exhausted, `StreamError`, idle-timeout, and missing-`stop_reason`
  paths (added `reject_pending` plus a DB-layer invariant test); and `PauseTurn`
  does not inspect `pending`, so it sends a request with an unpaired `tool_use`.
  Three P2s fixed. As a side effect, `summary` being built only from
  `TextBlock`s — which passed on an evidence-free summary for a phase cut short
  by a tool call — was also fixed.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — applied the verification
  results. **The Self-Review had overstated its coverage of "the seven loop
  limits in Section 5.4"** — only six actually fired, and the assertion test for
  them merely confirmed a constant existed, giving it no detection power. This
  is the same shape as a defect that has now appeared six times in this project,
  and it is evidence that self-review alone does not catch it. One controller
  misjudgment is also recorded: the `charge_tool_calls` failure path was judged
  to "produce no orphans", which was true only of `messages` (the protocol) and
  ignored `agent_steps` (the durable record) — in a project whose Section 6.1.3
  separates the two, the review looked at only one side.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — corrected Task 1's
  `StopReason` code (evidence: `ef26af4`). `derive(Serialize)` with
  `rename_all = "snake_case"` serializes the newtype variant `Unknown(String)`
  with external tagging, as `{"unknown":"..."}`, while the hand-written
  `Deserialize` accepts only a bare string — **so the type could not
  deserialize what it had serialized.** Made symmetric with a hand-written
  `Serialize` plus `as_wire()`, replaced the test with a bidirectional one that
  asserts on the serialized bytes, and added a mutation-check step. **Two rounds
  of plan verification did not catch this** — the compilation check (fable)
  because the code compiled fine and the tests of the day passed, and the design
  check (codex) because its scope was the loop and the limits. The task review
  caught it because the review prompt explicitly asked "Serialize is derived and
  Deserialize is hand-written — do the two directions agree on every variant?"
  Methodology note: a compilation check answers "does this compile", a design
  check answers "is the structure right", and **neither asks "can this type read
  back what it produced."**
- `revised` — `superpowers/plans/2026-07-30-agent.md` — two corrections to
  Task 3's state machine code (evidence: `19479fa`). (1) `content_block_stop`
  silently returned `Ok(vec![])` for an unknown block, so **an entire completed
  semantic unit vanished** — had that block been a `tool_use`, the tool call
  would disappear without a trace and the agent would carry on as though the
  model never requested it. The brief gave deltas and `stop` the same lenient
  handling but wrote the justification for deltas only, and that justification
  (mid-stream reconnection) does not hold for `stop`, because this design has no
  mid-stream resumption (Task 5 issues a fresh POST per turn). The leniency for
  deltas stays; only `stop` is escalated to `MalformedEvent` — making the
  asymmetry deliberate and commented. (2) `phase` was written in five places and
  read in exactly one (`is_done()`), so the four-state diagram was not enforced.
  The practical consequence is that a second `message_delta` re-emits `Stopped`,
  so a consumer taking the last one gets **the wrong termination reason
  winning** — a mutation run confirmed a second `Refusal` overwriting a first
  `EndTurn`. Blocked with a two-line guard, with a file comment stating that only
  `is_done()` and duplicate detection are enforced and the rest is
  documentation. **This is the fourth plan defect found by task review** — along
  with Task 1's two, all of them passed both the fable and codex verifications.
  The layers differ: a compilation check asks "does this code hold together", a
  design check asks "does the structure match the spec", and a task review asks
  **"what happens when this code is wrong."**
- `revised` — `superpowers/plans/2026-07-30-agent.md` — fifth plan defect:
  **the refusal category the spec's Section 8.4 requires never arrived anywhere
  in the pipeline.** `LlmEvent::Stopped` had no field for it, `stream.rs` never
  read `stop_details`, and Task 11 hardcoded `classify(&stop, None)` — the
  `Some(category)` branch of `classify` was dead code end to end and
  `TerminalReason::Refusal { category }` was always `None`. That nullifies the
  very reason the enum was created (to avoid cramming `stop_details.category`
  into a string). The spec's Section 8.4 states that Opus 5's safeguards can
  produce false positives on SRE and security-adjacent work, which is this
  product's primary use, so without the category an operator cannot tell why a
  refusal happened. Fixed in three places: Task 1 (the type), Task 3
  (extraction), Task 11 (propagation). The controller found it first and the
  Task 4 review confirmed it independently — attribution is Task 3, not a Task 4
  defect.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — applied three Minors
  from the Task 4 review. (1) The brief claimed the entire branch table was
  "copied verbatim from the spec's Section 8.4", but `EndTurn` (which comes from
  Section 5.3) and `StopSequence` (inferred from the absence of
  `stop_sequences` in Section 8.3's request schema) are not in Section 8.4's
  prose — provenance stated honestly. (2) Relaxed the assertion that pinned
  `MaxTokens`'s `detail` byte for byte — the spec requires only "a clear
  message", so an edit that polishes wording must not read as a regression.
  Measured: changing the wording passes, while `detail: None` fails. Brittleness
  removed, detection power retained. (3) The docstring of
  `test_5_every_stop_reason_maps_to_an_outcome` claimed to cover "all eight
  branches" when there are seven — `Refusal` is handled by the next test.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — Task 11's `Continue` arm
  left no signal when `pending` was empty. The spec's Section 8.6 states that
  "v0.1 uses no server-side tools, so `pause_turn` should not occur", yet the
  fact that it did occur would vanish without a trace. Fixed to emit a
  `tracing::warn!` regardless of whether `pending` is populated.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — ninth plan defect, an
  **ordering defect**: Task 8's test imported Task 10's `limits.rs`, so Task 8
  could not compile on its own. While fixing cross-check P2 (the 60-second
  constant existing in both `mcp::TOOL_CALL_TIMEOUT` and
  `Limits::default().tool_call_timeout`), the drift-check test was put in the
  wrong task. **When an earlier task imports a later task's module, that task
  cannot compile standalone** — the correct dependency direction is for the
  later task to check agreement with the earlier constant. Moved the test to
  Task 10 along with its mutation list. The implementer flagged a fallback of
  creating a stub `limits.rs`, but that would make Task 10 extend a file instead
  of creating it from a red test, and would leave a partial `Limits` missing six
  fields in the tree for someone to use first — deleting the test costs nothing
  (drift cannot occur before Task 10 creates the second constant).
- `revised` — `superpowers/plans/2026-07-30-agent.md` — eleventh plan defect:
  Task 9's `cargo tree -i indexmap` check **treats mere existence as a failure
  signal** and gives false positives. After Task 8 added rmcp, `indexmap`
  entered the graph through `rmcp → process-wrap` and through `sqlx-core`, with
  no relation to `serde_json`. The precise signal is **whether `serde_json`
  itself depends on `indexmap` directly** (only then is `preserve_order` on,
  making `Map` an `IndexMap` instead of a `BTreeMap` and leaking
  `input_schema` key order — insertion order — into the prompt prefix), which is
  checked with `cargo tree -p serde_json -e normal`. Measured: serde_json
  depends only on itoa, memchr, serde_core, and zmij — safe. The implementer
  followed the brief's fallback procedure, judged it independently, and reported
  it as a plan defect.
- `revised` — `superpowers/plans/2026-07-30-agent.md` — twelfth plan defect,
  **the same class as the P0**: Task 10's
  `phase_reset_clears_per_phase_counters_but_not_the_clock` asserted
  `check_wall_clock() == Ok(())` under a `wall_clock: 3600s` ceiling, and that
  assertion is **true whether or not `reset_phase` resets `started`**, so it
  cannot distinguish the two cases. The mutation (adding
  `self.started = Instant::now()` to `reset_phase`) turned out not to fire. The
  spec's Section 5.4 makes the wall clock **the only limit that fails an entire
  investigation**, so if `reset_phase` resets it an investigation can run
  forever by crossing phase boundaries — exactly the runaway Section 5.4 exists
  to prevent. Replaced with a fixture that starts with `started` already past
  the limit (`Instant::now() - 200ms` against `wall_clock: 50ms`) and asserts
  `Err` both before and after the reset. It is deterministic because it never
  actually waits. The implementer read the non-firing mutation correctly as "the
  test cannot distinguish" rather than "the mutation was not applied."
- `added` — `crates/agentops-agent/src/runner.rs`,
  `crates/agentops-agent/tests/runner.rs` — implemented plan 2 Task 12, the
  investigation runner (evidence: `92ee27e`, `5c9967a`). It always runs the
  three phases of `Phase::INVESTIGATION_ORDER`, renders the phase summaries into
  a single artifact, and backs off when a terminal transition returns
  `Conflict`. Because `complete_investigation` already leaves the
  `ArtifactWritten` step inside its transaction, the runner leaves no separate
  step (the brief asked this be confirmed; verified at `artifacts.rs:51-98`).
  One brief defect fixed: the test code used
  `LlmEvent::Stopped { reason, usage }`, but since the fifth plan defect was
  fixed the actual definition has three fields including `refusal_category`.
  **Two mutations not in the brief exposed two gaps in detection power.**
  (1) Deleting `budget.reset_phase()` left all six existing tests passing —
  neither the status (exceeding the turn limit does not fail an investigation,
  Section 5.4) nor the termination reason (all three limits return the same
  `TurnLimitExceeded`) can distinguish it, and the only observable that can is
  the provider call count (1 versus 3). Closed with
  `the_turn_budget_resets_at_each_phase_boundary`. `tests/limits.rs` verified
  what `reset_phase` does, but **nobody checked that the runner calls it** — a
  unit-test-to-call-site gap. (2) **Unresolved**: deleting the `Terminated` step
  the runner leaves on wall-clock termination still passes the tests —
  `fail_investigation` leaves the same step inside its transaction, so normal
  operation produces two identical rows, and because the runner's row is outside
  the terminal transaction, an intervening shutdown leaves a `WallClockExceeded`
  record behind even after backing off. The code was explicitly specified by the
  brief, so it was reported with a recommendation to delete rather than changed.
- `revised` — `docs/superpowers/plans/2026-07-30-agent.md`, Task 12 — marked the
  `append_step(Terminated { WallClockExceeded })` in the wall-clock branch of the
  Step 3 code block as retired (the original is kept for history with a
  ⚠️ SUPERSEDED comment and a retirement section appended). `fail_investigation`
  already writes the same row as `Phase::All` inside the terminal transaction,
  so the normal path produces two byte-identical rows (measured: `seq: 0` /
  `seq: 1`), and because the runner's write is outside the transaction, an
  intervening shutdown makes `fail_investigation` back off with `Conflict` while
  that row remains, contradicting another party's termination reason. The plan
  already stated the principle in the very next paragraph ("adding a step while
  breaking the single-transaction guarantee is worse"), but applied it only to
  `ArtifactWritten` and not to the `Terminated` in its own code block. **The four
  mutations specified in Step 6 did not catch this** — deleting that
  `append_step` left them all passing, because the tests used `any()` to check
  presence and cannot tell one row from two. Closed by replacing with count
  assertions (the reinsertion mutation now fails with `left: 2, right: 1`). This
  is the same shape as this repository's known gap — presence assertions cannot
  see multiplicity — appearing outside plan 1's `check_spec_test_ids.py`.
  Evidence: `9d27771`
- `revised` — `crates/agentops-agent/src/phase.rs`,
  `crates/agentops-agent/tests/phase.rs`,
  `crates/agentops-agent/tests/runner.rs` — Task 12 review C1 (Critical):
  `9d27771` applied the judgment "the terminal row is owned by the terminal
  transaction" to the wall-clock path without generalizing it to sibling paths,
  so the same defect remained. When the `TurnOutcome::FailInvestigation` arm
  writes `Terminated { reason }` with the real phase tag, outside a transaction,
  and sets `failed`, the runner then calls `fail_investigation` and writes a
  second row with the same reason as `Phase::All` — measured: `seq 0 @ Triage` /
  `seq 1 @ All`. The racing path is worse: if shutdown wins first,
  `fail_investigation` backs off with `Conflict`, but this row was already
  written unconditionally, leaving a `Refusal` termination record on an
  investigation that ended by shutdown. The general rule is now stated in a code
  comment — **a `Terminated` row is owned by the writer that flips the status
  with it.** `reject_pending` was left alone (it is another session's C1 fix and
  is independently correct). **Other arms were not touched**: this is the only
  arm that sets `failed = Some`, and `Terminate` plus the four `TaskPanicked`
  paths set `terminated`, which the runner does not read, so they have no second
  writer and deleting them would erase the fact of termination. However, **that
  arm's terminal row had no test** — deleting `append_terminated` left all 20
  passing (measured). To stop the next person from applying the same cleanup to
  that arm,
  `max_tokens_records_its_terminal_step_here_tagged_with_the_actual_phase` was
  added, turning the asymmetry into a tested contract (the `Phase::Triage` tag
  is what distinguishes the two paths).
  `refusal_in_the_first_phase_fails_the_investigation` was not looking at
  `agent_steps` at all, so a count assertion was added (the reinsertion mutation
  fails with `left: 2, right: 1`). Minor M1 was closed too — the `find_map` in
  `completion_writes_an_artifact_and_a_step` became a count. **The reason the
  same defect appeared twice on two paths is that the first fix was handled as
  an instance and never promoted to a rule.** Evidence: `230c78f`
- `revised` — `crates/agentops-agent/tests/phase.rs` — Task 11 re-review
  finding: the `Error` and `Terminated { TaskPanicked }` assertions in
  `stream_error_terminates_the_phase_and_is_recorded` used `any()` and could not
  tell one row from two. Changed to counts, and `phase == Phase::Triage` is now
  asserted too (this path sets `terminated`, which the runner does not read, so
  there is no second writer, and the actual phase tag is what distinguishes it
  from the runner path's `Phase::All`). The mutation (a second
  `append_terminated` on the same path) fails with `left: 2, right: 1`, as
  confirmed. **Counting where the presence-not-count shape appeared in this plan
  gives six**: (1) the `set[str]` in plan 1's `check_spec_test_ids.py` (recorded
  in CLAUDE.md), (2) the `any()` in the runner's wall-clock path — hid a
  duplicate (`9d27771`), (3)
  `refusal_in_the_first_phase_fails_the_investigation` not looking at
  `agent_steps` at all — hid a duplicate (`230c78f`), (4) the `find_map` in
  `completion_writes_an_artifact_and_a_step` (`230c78f`), and (5) and (6) the
  two assertions in this test. Three phase-step presence assertions remain
  unchanged — `ToolResult{is_error}`, `Terminated{TurnLimitExceeded}`, and
  `Error{idle}` in `tests/phase.rs`. None carries a duplication risk today, so
  they stay in Task 11's scope. **This plan revealed that the limitation
  CLAUDE.md recorded about `check_spec_test_ids.py` is not confined to that
  script but runs through this repository's test assertions generally.**
  Evidence: `6aa5149`
- `revised` — `crates/agentops-agent/src/phase.rs`,
  `crates/agentops-agent/tests/phase.rs`,
  `crates/agentops-agent/tests/runner.rs` — the **third instance of the terminal
  row ownership defect, and the only one actually reachable** (re-review C1). The
  idle-timeout branch writes `Terminated { WallClockExceeded }`, and the runner
  does not read `out.terminated` but instead **independently re-derives** the
  same condition through `budget.check_wall_clock()` — so when a stalled stream
  fires late and the accumulated time also exceeds the investigation limit, both
  writers hold simultaneously and two rows remain, `seq 0 @ Triage` and
  `seq 1 @ All` (reproduced at the runner level). The criterion was made
  precise: **`WallClockExceeded` is the only reason the runner re-derives
  independently.** Other reasons are reached only through `out.failed`
  (`FailInvestigation`) or are not reached at all (`Terminate`, the four
  `TaskPanicked` paths), so `phase.rs` is the sole writer there and must keep
  writing. **"The runner does not read this field" is not enough to see this
  collision — a re-derivation does not go through the field.** The early-firing
  case is worse than a duplicate: writing a row while the investigation limit is
  nowhere near is a false record, and because the spec's Section 5.4 (the table
  at line 297) pins the idle timeout's reason to `WallClockExceeded`, there is
  no true reason available to choose, so nothing is written (the fact does
  survive in the `Error` step's `LlmError::IdleTimeout` message). Two re-review
  Importants were also closed — the terminal rows in `charge_tool_calls` and
  `charge_pause_resume` had no verification whatsoever. **Actually asking the
  previous entry's question — how many other instances of this defect are there
  — produced one more, and it was the only one reachable in production.**
  Evidence: `f7b76db`
- `revised` — `docs/superpowers/specs/2026-07-30-agentops-design.md`,
  Section 12.1 — plan 2 final whole-branch review I4 (Important, a problem
  actually live in CI): `check_spec_test_ids.py` excludes an item from its scope
  when it sees a deferral marker, and seven already-implemented items —
  `TEST-5`, `TEST-6`, `TEST-9B`, `TEST-12`, `TEST-15`, `TEST-16`, `TEST-17` —
  still carried that marker. `drift-guards` printed "9 in-scope IDs all exist in
  tests", but all nine were plan 1's — while this branch added 90 tests, the
  traceability scope grew by zero, and the 18 test functions corresponding to
  those seven IDs could all have been deleted or renamed with CI staying green.
  Removing the markers raised the scope from 9 to 16 (measured). Whether the
  check has real detection power was confirmed by mutation — temporarily
  renaming `test_6_unknown_events_and_pings_do_not_stop_the_parser` in
  `tests/sse.rs` without its prefix produced `FAIL: ID present in spec but not
  in tests: TEST-6`, and reverting restored the pass. **A guard that reports
  success over an empty scope is worse than no guard.** Evidence: plan 2 final
  review
- `revised` — `crates/agentops-agent/src/{phase,runner,mcp,mcp_client}.rs` and
  their tests — final whole-branch review C1 (Critical), I1, I2, I3, M5. **C1 was
  found by a question the previous three rounds never asked: "what happens when
  a field nobody reads gets set?"** Six paths set `PhaseOutcome::terminated` and
  there was no consumer, and that very fact was being used as an argument that
  "there is no second writer, so it is safe." The signal that field carries is
  "this phase could not do its job", so when it goes unread an investigation
  ends `completed` even when all three phases hung on a stalled stream
  (measured: `status=Completed terminated_rows=0`, an artifact with three empty
  sections). Fixed by having the runner **evaluate `check_wall_clock()` once** to
  decide the terminal row writer — the review's suggestion of a conditional
  write on the `phase.rs` side leaves a race where the limit passes between the
  two checks and both write. If no phase terminated cleanly and there is nothing
  to carry forward, the investigation fails (both conditions are required
  because the text of a phase cut short by `MaxTokens` is still valid —
  Section 5.4). C1b: swallowing a `tools/list` failure and running all three
  phases without tools was fixed — `McpToolRegistry` already swallows per-server
  failures individually, so this `Err` is effectively only a policy store error,
  and proceeding with an empty list under a deny-by-default policy is a policy
  bypass. I1: `TerminalReason::ToolTimeout` was never constructed in production
  (the spec's Section 5.4 table exempts only the parallel-call limit from
  termination). I2/M5: `ToolOutput::is_error` was hardcoded `false`, so a
  server-reported `permission denied` was recorded as a success; `truncated` had
  no consumer, so the marker required by Section 9 was never attached; and
  non-text content blocks were discarded entirely. I3: `pause_turn` resumption
  sent an unpaired `tool_use` — the durable record said "not executed" while the
  wire said "in progress". Eight tests added; all eight mutations confirmed
  detected. Evidence: `c052c4d`, `8c6de83`
- `verified` — the test harness — running `cargo test --workspace` at default
  parallelism fails **roughly one run in four** with
  `failed to connect test pool: Protocol("unexpected response from SSLRequest:
  0x00")`. This is not a logic defect but exhaustion of Postgres's
  `max_connections=100` (the docker-compose default), triggered by the
  combination of `#[sqlx::test]` creating a pool per test and cargo running 19
  test binaries concurrently. The trap is that the failure point is a pool
  connection, which makes the cause look like code. With `-- --test-threads=4`,
  five consecutive runs all passed (177 tests). **CI running at default
  parallelism will go red at the same rate** — the response (raising the
  container's `max_connections`, pinning `--test-threads` in CI, or shrinking
  the test pool size) is outside this task's scope, so only the record is left.
- `revised` — `crates/agentops-agent/src/runner.rs`,
  `crates/agentops-agent/tests/runner.rs` — final review N1: the completion
  guard also required "no phase terminated cleanly", so **a single phase that
  ended on `end_turn` with no content defeated it** (`terminated` is `None`, so
  it counts as a clean termination). `ThinkingBlock` does not contribute to
  `summary`, so a turn that only thinks produces that state with no machine
  failure at all, and the result was the same as C1a: `status=Completed` with an
  artifact of three empty sections. Fixed to judge on the output alone —
  verified directly: `summary` grows in only two places, text blocks
  (`phase.rs:271`) and tool evidence (`:554`), so all three being empty means
  there was neither text nor a tool call and there is nothing to carry forward.
  Even stating the conclusion "there is nothing to do" produces a text block, so
  that case never reaches this branch, and a phase truncated by `MaxTokens` has
  text and is not caught (spec Section 5.4). `a_phase_ended_cleanly` was deleted
  because its reader was gone — a variable that is only set and never read is the
  exact shape of this branch's C1 defect. **N2 (adding `detail` to
  `PhaseOutcome`) was not done**: measured, `terminated` is set in 11 places
  while the runner reads only one of them, the idle path, so adding the field
  would create **ten set-but-never-read values**, resurrecting at greater scale
  the defect class that four rounds had just eliminated. Four of those places
  already compute the same string as an argument to `append_terminated`, which
  would create a second source of truth as well. The correct shape is for the
  `terminated` value itself to carry the detail (as
  `Option<(TerminalReason, Option<String>)>` or similar), and that ripples into
  16 places that assert on `out.terminated`, so it belongs to plan 3. Evidence:
  `afc2a9c`
- `revised` — CLAUDE.md — updated "current state" to plan 2 complete (3 crates,
  178 tests) and stated the need for `--test-threads=4` — final review M3 and
  G3.
- `revised` — CLAUDE.md — promoted presence-not-count from a property of
  `check_spec_test_ids.py` to a **convention**, and added the two questions "who
  recomputes this?" and "what happens when a field nobody reads gets set?" —
  evidence `9d27771`, `230c78f`, `c052c4d`.
- `revised` — `.github/workflows/ci.yml` — added `--test-threads=4` to
  `cargo test --workspace --all-targets`, bringing CI in line with CLAUDE.md's
  rule. The rule had been declared while CI did not honor it — **this mismatch
  was found by the graphify extraction** (an AMBIGUOUS edge between the
  `--test-threads=4` convention and the `CI job: test` node).
- `revised` — `docs/superpowers/plans/2026-07-30-agent.md` — `status: draft` →
  `stable`. It had remained draft even after all twelve tasks were implemented,
  reviewed, and merged (a violation of CLAUDE.md MUST 3).
- `verified` — `docs/superpowers/plans/2026-07-30-agent.md` — recorded the final
  whole-branch review (machine) in `verified[]`. First pass Needs work
  (Critical 1, Important 5) → CLEARED after all were fixed.
- `verified` — `docs/superpowers/plans/2026-07-30-foundation.md` — recorded the
  final whole-branch review (machine) in `verified[]`. It had no `verified[]`
  entries at all (a violation of CLAUDE.md MUST 5). Ready to finish —
  Critical 0, Important 0, Minor 2.

## Plan 3 — web layer

- `added` — `docs/superpowers/plans/2026-08-03-web.md` — wrote plan 3 of 3, the
  web layer, in twelve tasks. Covers Sections 4.2, 6.1, 10, 11, and 13 of the
  spec and closes INV-1, INV-2, INV-3, TEST-1, TEST-2, TEST-3, TEST-8, TEST-10,
  and TEST-18. Names the owning task for each of plan 2's carry-overs G1, G2,
  G4, I5, M1, M2, and F1.
- `added` — `crates/agentops-server` — implemented plan 3 Task 1 (server crate
  skeleton, `Config::from_env`, `/api/health`). Two brief defects fixed: (1) the
  `Cargo.toml` template's `version.workspace = true` referenced a key absent
  from the workspace and failed to compile — replaced with the literal
  `version = "0.1.0"` pattern used by the three existing crates; (2) two tests
  in `tests/health.rs` touched process-global environment variables
  concurrently without a lock and failed flakily in 3 of 5 measured runs —
  serialized with a `static Mutex` guard, then 8/8 stable.
- `revised` — `crates/agentops-server/src/routes/health.rs` — Task 1 review,
  Important: the health handler swallowed database errors silently with
  `.is_ok()` (transcribed straight from the brief's Step 6 code — the plan
  violated its own global constraint, "never swallow database errors silently",
  two sections after writing it). Changed to a `match` that emits
  `tracing::error!` and continues with `{"db": false}` — this is the only
  database-error handling example in this crate, so the fragment handlers of
  Tasks 5 through 12 will copy its shape. Added the regression test
  `db_failure_is_logged_not_swallowed`: a pool created with `connect_lazy`
  against a loopback port with no listener produces a real query failure, and a
  `tracing-subscriber` buffer writer asserts on the log content directly
  (content inspection rather than presence probing, so it genuinely reacts to
  reverting to `.is_ok()` — confirmed by mutation: `left: ""`, the log is
  empty).
- `added` — `README.md` — the spec's Section 13 requirement of a
  public-exposure warning at the top of the README was assigned to no task in
  plan 3, as the Task 1 review pointed out. The repository had no README.md at
  all, so one was created, leading with the v0.1 no-authentication warning, the
  `127.0.0.1:3000` default bind, and the prohibition on public exposure.
- `added` — `crates/agentops-server/src/bus.rs` — implemented plan 3 Task 2
  (`StepBus`). Isolates a `broadcast::Sender` per investigation in a
  `HashMap<Uuid, _>` so one busy investigation cannot push another's slow
  subscriber into `Lagged`, and ignores `send`'s `Err` (zero subscribers) as a
  normal state so the runner keeps going on an investigation nobody is
  watching. Implements `DeltaSink` to emit deltas as `BusEvent::Delta`,
  separate from `Step` — a delta emitted as a `Step` has no `seq` and would
  break the replay and deduplication of later plan 3 tasks (spec
  Section 6.1.3). No brief defects. Two mutations confirmed: (1) as specified —
  replacing `sender()` with a single global channel failed only
  `subscribers_only_see_their_own_investigation`, as predicted; (2) chosen
  independently — changing `let _ = ...send(ev)` in `emit()` to `.unwrap()`
  failed both tests that create a zero-subscriber state (the panic site is
  `b`'s first `publish_step`, so
  `subscribers_only_see_their_own_investigation` is caught too) — confirming
  both load-bearing decisions have real detection power. Both mutations
  verified applied with `grep` before running, and restored with `grep` plus
  `git status --short` (the brief warned in advance that `git diff` is
  meaningless for a new file). Full workspace 186 tests (baseline 181 plus 5
  new) passing, clippy clean. Details in
  `.superpowers/sdd/2026-08-03-web/task-2-report.md`.
- `revised` — `crates/agentops-server/src/stream.rs`,
  `crates/agentops-server/tests/stream.rs` — implemented plan 3 Task 3 (the SSE
  stream: subscribe → replay → deduplicate → live → `Lagged` recovery).
  **The Step 6 mutation check found a defect in the brief's own test harness**:
  with `hook` placed where the brief said (immediately after subscribing,
  immediately before replay), of the three mutations — A (invert the
  subscribe-replay order), B (delete deduplication), C (ignore `Lagged`) — B and
  C broke no test at all, and A broke the unrelated
  `terminal_event_closes_the_stream` rather than `test_1`. The cause: `hook` was
  a single block completed with `.await`, so its database write always committed
  before the replay query (Postgres autocommit), and `collect_stream_for_test`
  stopped as soon as `want` was satisfied, returning before ever reaching the
  live loop — which is where deduplication and `Lagged` recovery actually live.
  The two invariants this task exists to protect (INV-2 and INV-3) were not
  being verified at all, while showing 6/6 green — plan 3's instance of
  CLAUDE.md's "a guard that is named and green with no detection power". After
  reporting to the team lead, the harness was redesigned: `hook` moved to after
  replay and before the live loop, and test_1, test_2, and test_3 were
  restructured so `want` can only be satisfied from the live channel (replay
  output < want). On re-check all four mutations broke at least one test — A
  failed test_1, test_2, test_3, and terminal together (structurally expected,
  since INV-2 is a shared precondition of the live path; the narrow guard is
  test_1), B failed test_2 alone, C failed test_3 alone, and D failed test_18
  alone (unrelated to the redesign, as the brief predicted). Removing the
  deferral markers from `TEST-1`, `TEST-2`, `TEST-3`, `TEST-18`, `INV-2`, and
  `INV-3` in Sections 12.1 and 6.1 raised `check_spec_test_ids.py`'s scope from
  16 to 22, but `INV-2` and `INV-3` still failed — the guard requires an
  `inv_N_` prefixed function name, while `test_1` and `test_3` satisfy only
  `TEST-1` and `TEST-3` (the same 1:N pattern as `INV-4`). Added
  `inv_2_subscribe_happens_before_replay_or_the_live_step_is_lost` and
  `inv_3_lagged_must_resync_or_steps_vanish_silently` in minimal form (with
  mutations A and C used to confirm real detection power, so they would not
  become empty name-matching guards), turning `check_spec_test_ids.py` green.
  Full workspace 194 tests (baseline 186 plus 8 new) passing, clippy clean.
  Details in `.superpowers/sdd/2026-08-03-web/task-3-report.md`.
- `revised` — `crates/agentops-server/src/stream.rs` — Task 3 independent
  review, Important. After independently reproducing all four mutations and
  confirming they matched the report exactly (re-verifying that the harness
  redesign gained real detection power rather than just new names), the review
  found a real defect in code Task 3 itself wrote, which was not on the brief's
  interface list: `investigation_stream` and `items` never checked the
  investigation's status at connection time, so connecting anew to an
  investigation that had already ended `completed` or `failed` (a reconnect, a
  refresh, an old link) meant `sub.recv().await` blocked forever after replay —
  and because the producer task waits for a next event it never notices the
  client disconnecting, leaking the task and its `broadcast::Receiver` until the
  process exits. If Task 10 renders `sse-connect` on the detail page of
  completed investigations, this is triggered by the ordinary act of opening a
  past investigation. Fixed by checking the status with `Store::get_investigation`
  immediately after replay (rather than inferring from the last step kind —
  `complete_investigation` and `fail_investigation` commit the status transition
  and the terminal step in one transaction, so `status` is the more direct
  authority) and sending `Terminal` and returning at once if it has already
  ended. Race-window analysis: there is no remaining window if the investigation
  ends between the check and entering the live loop — the subscription (INV-2)
  always precedes this check, so a `publish_terminal` in between is already
  buffered in the active subscription channel and is received normally on
  entering the live loop. Added the regression test
  `stream_opened_on_a_completed_investigation_closes_instead_of_hanging`
  (verified through a real `store.complete_investigation` transaction) —
  confirmed with a mutation deleting the check block that it fails exactly as
  the review reproduced (two replayed items, then a 20-second timeout), then
  restored. Because the fix is adjacent to the live-loop entry, the full
  four-mutation matrix (A/B/C/D) was re-run — identical results, no regression.
  `check_spec_test_ids.py` holds at 22, full workspace 195 tests (194 plus 1
  new) passing, clippy clean. Details in the fix-round section of
  `.superpowers/sdd/2026-08-03-web/task-3-report.md`.
- `revised` — `crates/agentops-server/src/stream.rs` — closed the one
  non-blocking Minor left by the Task 3 re-review (**ADDRESSED/CLEARED**): the
  `Lagged` handler resynced only steps from the database and never rechecked the
  terminal status, so if `Terminal` itself was evicted from the channel by
  `Lagged` (enough events arriving during the database round trips of
  subscription setup), the same permanent hang-and-leak that round 1 fixed
  remained on this one path. The team lead disputed the re-review's "unrealistic"
  assessment with evidence — **deltas share this channel.** Verified by
  comparing `agentops-agent/src/phase.rs:261` (`ctx.sink.text(id, phase,
  &text)`) with `bus.rs:96` (`StepBus::text`): a delta goes out on every
  `LlmEvent::TextDelta` with no database round trip and rides the same
  `broadcast::Sender` as `Step` and `Terminal`, so accumulating 256 deltas (the
  channel capacity) during the two or three database round trips of the replay
  query and status check is routine rather than exceptional — the team lead's
  judgment was correct. Fixed by extracting the post-replay check into a
  `close_if_investigation_is_terminal` helper and reusing it after a successful
  `Lagged` step resync. Added the regression test
  `lagged_recovery_rechecks_terminal_status_when_the_terminal_event_itself_is_evicted`,
  whose hook performs a real `complete_investigation` transaction, then
  `publish_terminal`, then a flood of 300 deltas, in that order, evicting
  `Terminal` deterministically. The first attempt's assertion was wrong
  (expecting `seqs.is_empty()`) — the `ArtifactWritten` step that
  `complete_investigation` commits (caught only by the `Lagged` resync, since
  the not-yet-existing JobManager does not broadcast it) legitimately arrived as
  `[0]`, which is the resync doing its job rather than a bug, so the assertion
  was corrected. A mutation deleting only the recheck call in the `Lagged`
  branch reproduced exactly the failure the review predicted (one resynced step
  received, then waiting forever), then was restored; the other nine tests do
  not react to that mutation. The four-mutation matrix was re-run — A now breaks
  seven tests including the new one (all depend on the hook plus the live
  channel) and C breaks three including the new one (all depend on the whole
  `Lagged` branch), but individual detection power is unchanged and there is no
  regression. `check_spec_test_ids.py` holds at 22, full workspace 196 tests
  (195 plus 1 new) passing, clippy clean. Details in the re-review fix round 2
  section of `.superpowers/sdd/2026-08-03-web/task-3-report.md`.
- `revised` — `crates/agentops-agent/src/phase.rs`,
  `crates/agentops-agent/src/runner.rs`, `crates/agentops-core/src/step.rs` —
  implemented plan 3 Task 4 (plan 2 carry-overs). Changed
  `PhaseOutcome::{terminated, failed}` from `Option<TerminalReason>` to
  `Option<Termination>` (`reason` plus `detail`), closing the trap where of the
  twelve places that set `terminated` (the brief said eleven; measurement found
  a twelfth, the `check_wall_clock` failure branch at loop entry — a site the
  first `cargo check` missed) only the one the runner reads would carry a
  detail. Added `TerminalReason::DependencyUnavailable { what }` to name policy
  store and MCP server failures instead of `TaskPanicked`, and added it to the
  manual list in `every_terminal_reason_serializes` so a new variant does not
  fall outside round-trip coverage. Switched `phase.rs`'s `tools/list` failure
  path to that new variant. The idle-timeout path (`WallClockExceeded`, the only
  path that writes no terminal row) reuses the `e.to_string()` already written
  to the `Error` step for `Termination.detail` as well — no second copy. The
  runner now unwraps `.reason` and passes `detail` along on the
  `WallClockExceeded` terminal row (it was previously fixed at `None`, losing
  the cause of a late-firing idle timeout). The twelve existing assertions
  (`out.terminated == Some(TerminalReason::X)`) were not weakened to
  `is_some()`; they were wrapped in a `reason(&out.terminated)` helper to keep
  exact-reason detection. Added the `RunTools` assistant-turn test (G4).
  **The registry investigation-scope test (I5) did not use the brief's fixture
  as given** — in the original, A and B allow the same tool under the same
  policy, so the result is identical whether the `known` cache is per instance
  or globally shared; passing would prove nothing (the same shape as CLAUDE.md's
  "presence is not multiplicity"). Added `Fake::set_tools` so A calls `list()`,
  then the server's exposed set shrinks, then B calls `list()`, making the two
  caches genuinely differ — confirmed by temporarily making the `known` field a
  shared `&'static Mutex` and watching this test fail as predicted, then
  reverting (`git checkout`, empty `git diff --stat`). `#[tokio::test]` plus
  `policy_store_allowing` were also absent from this file, so rather than
  stubbing twenty-odd unrelated Store methods to satisfy `PolicyGate<S: Store>`,
  the file's existing `#[sqlx::test]` plus `PgStore` plus `seed()` convention
  was followed. Added TEST-10
  (`test_10_each_phase_gets_only_its_own_instructions`); removing the deferral
  marker in the spec's Section 12.1 raised `check_spec_test_ids.py`'s scope from
  22 to 23. The three mutations specified in the brief's Step 8 (idle-timeout
  detail to None, fetch instructions with `Phase::All` only, drop one ToolUse
  from the assistant) each failed exactly one specified test — unlike Task 3,
  the three specified mutations' harness was not itself powerless this time.
  Full workspace 200 tests (196 plus 4 new) passing, clippy clean. Details in
  `.superpowers/sdd/2026-08-03-web/task-4-report.md`.
- `added` — `crates/agentops-server/src/jobs.rs` — implemented plan 3 Task 5
  (`JobManager`). `spawn(id)` puts an independent task on a `JoinSet` and
  returns immediately (INV-1) so the HTTP handler never waits for an
  investigation. If the runner exits with a `StoreError` (M1, Minor 1 of plan
  2's final review) or panics (INV-4's second layer), it is wrapped in
  `AssertUnwindSafe(run_investigation(...)).catch_unwind()` so the task itself
  catches it while knowing its own `id`, and applies the conditional terminal
  transition through `fail_investigation` — one transaction writes the status
  and the terminal step together rather than splitting into `append_step` plus a
  separate `UPDATE`. Added the first caller of `retire()` (the gap the Task 2
  review noted: defined but used by nobody) — called after a `RETIRE_GRACE`
  delay (5 seconds, extracted as a constant) rather than immediately after
  `publish_terminal`, giving SSE subscribers time to drain `Terminal`.
  **Design deviation**: the brief's pseudo-code fixed `store: Arc<PgStore>`, but
  the M1 test needs to inject a `FailingStore` wrapper that fails only
  `append_step` into the runner, so it was made generic as
  `JobManager<S: Store + Send + Sync + 'static>` (the same pattern as the
  existing `McpToolRegistry<S>` and `PolicyGate<S>`). `Clone` is implemented by
  hand so it does not require `S: Clone` (avoiding the derive macro's
  over-conservative bound). I5 (a fresh registry per investigation, so the
  `known` cache is not shared) follows the brief's pseudo-code as written. All
  four mutations detected as predicted — mutation 1 (run synchronously) broke
  `inv_1_...` for the predicted reason (a timing assertion, 5.69s) and required
  a separate OS thread plus a new runtime to reproduce (`#[sqlx::test]` is
  measurably a current-thread runtime, so `block_in_place` panics immediately);
  incidentally two other tests also failed from an sqlx pool-crossing-runtime
  artifact, judged not to be a signal of the mutation itself and recorded as
  such. Mutation 2 (remove `fail_investigation` from the M1 branch) broke
  `m1_...` alone, and mutation 3 (remove it from the panic branch) broke
  `a_panicking_task_...` alone — different tests, as the brief required.
  Mutation 4 (remove `bus.retire(id)`) broke
  `finished_investigations_release_their_channel` alone. All four verified
  applied and reverted with `grep` plus a copy `diff` (the brief warned that
  `git diff` is meaningless for new untracked files). Removing the deferral
  marker on INV-1 in Section 6.1 of the spec raised
  `check_spec_test_ids.py`'s scope from 23 to 24. Full workspace 205 tests (200
  plus 5 new) passing, clippy clean. Details in
  `.superpowers/sdd/2026-08-03-web/task-5-report.md`.
- `revised` — `crates/agentops-server/src/jobs.rs`,
  `crates/agentops-server/tests/jobs.rs` — Task 5 review, fix round 1.
  Important 1: the M1 and panic branches logged `tracing::error!`
  unconditionally on any `Err` from `fail_investigation`, including `Conflict` —
  but `runner.rs::terminate_failed` already treats `Conflict` as normal
  operation (another party terminated it first), so once Tasks 6 and 7 begin
  racing for the same investigation, every loser would leave a false error log
  on normal operation. Changed to pass `Conflict` silently. Important 3:
  changed the M1 branch's `TerminalReason` from `TaskPanicked` to
  `DependencyUnavailable { what }` — the brief's pseudo-code specified the
  former, but Task 4 had already created a variant for exactly "an external,
  recoverable dependency failure rather than a panic" (the second case of the
  plan contradicting itself; the more specific rule wins). Since
  `fail_investigation` has no detail parameter, the cause is carried as
  `what: format!("store: {e}")`. Added a clause to the M1 test asserting the
  exact reason (`DependencyUnavailable`) with a count (`terminated.len() == 1`)
  — applying the review's point that status alone cannot distinguish before and
  after this fix. Minor 2: made the `RETIRE_GRACE` wait `select!` against
  `cancel.cancelled()` so shutdown retires immediately instead of waiting out
  the grace period (before the fix, a shutdown deadline shorter than
  `RETIRE_GRACE` always timed out on a task doing nothing). Added the
  regression test `cancellation_short_circuits_the_retire_grace` — confirmed by
  a mutation deleting the `select!` that it really fails (a 5.01s wait), then
  restored. The mutations for the three changed branches (2, 3, 4) were re-run —
  2 and 3 each still break a different test alone, and 4 now breaks both the
  existing and the new test (naturally, since both react to the same defect).
  Full workspace 205→**206 tests** (1 new) passing, clippy clean,
  `check_spec_test_ids.py` holds at 24. Details in the fix round 1 section of
  `.superpowers/sdd/2026-08-03-web/task-5-report.md`.
- `added` — `crates/agentops-server/src/jobs.rs` (`JobManager::shutdown`),
  `src/main.rs` (signal wiring) — implemented plan 3 Task 6 (graceful shutdown).
  Transcribed the spec's Section 6.1 six stages in order (close the intake →
  fire cancellation → signal SSE termination → wait until the deadline → abort
  the excess → conditionally transition still-running investigations to Failed
  with a Terminated step). When stage 6's `fail_investigation` returns
  `Conflict` it **logs nothing**, matching the judgment the Task 5 review had
  just settled for the M1 and panic branches in `jobs.rs::spawn` — the brief's
  pseudo-code said to log `tracing::debug!`, but logging normal operation where
  another party simply won the race would plant on the shutdown path the very
  defect the Task 5 review had just fixed. **Brief defect**: the `main.rs`
  snippet assumed `jm` was already assembled, but per the plan document the
  first place `JobManager::new(...)` is assembled was the Task 7 section (boot
  cleanup and watchdog) — and this task itself states that "`shutdown_deadline`
  first touches behavior here", so deferring assembly would leave its output
  unverified. A minimal `JobManager` was assembled with `AnthropicProvider` plus
  `cfg.anthropic_api_key` so the shutdown signal is wired to a real instance (no
  route spawns onto this instance yet — the `AppState.jobs` field and route
  wiring are Tasks 7 and 9 by plan). Added three tests in a new
  `tests/shutdown.rs`, correcting the brief's `id_artifact(id)` (undefined; the
  real `complete_investigation` signature takes `(id, &NewArtifact)`) and
  `never_ending_provider` (undefined; newly written with
  `futures_util::stream::pending()`) to match the real signatures.
  **Two mutations, and the measurement diverged from the prediction**:
  mutation 1 (swap the order of `close_gate()` and `cancel.cancel()`) **broke no
  test at all**, contrary to the brief's prediction that
  `shutdown_closes_the_gate_before_cancelling` would fail — both calls are
  synchronous with no `.await`, so swapping them happens nanoseconds apart while
  the test observes only 20ms later, by which time both have completed in either
  order. The real risk (a `spawn()` slipping into that nanosecond window leaves
  an investigation stuck in `queued` forever, which stage 6's
  `stale_running_ids` cannot catch since it looks only at `running`) is genuine,
  but deterministic reproduction needs a synchronization point like
  `stream.rs::Hook`, which is outside this task's scope, so only the fact is
  recorded. Mutation 2 (split the terminal row into `append_step` plus a
  separate status `UPDATE`) had to be applied to the actual implementation,
  `agentops-store/src/artifacts.rs::fail_investigation`, rather than
  `jobs.rs::shutdown` (the `Store` trait has no status-only update method) —
  as predicted, `test_13_...`'s terminal-step count assertion failed (2), and
  the sibling `agentops-store` test using the same transaction broke at the same
  time. Reverted with an empty `git diff`. Full workspace 209 tests (206 plus 3
  new) passing, clippy clean, `check_spec_test_ids.py` holds at 24 (TEST-13 was
  already in scope before Task 5). Details in
  `.superpowers/sdd/2026-08-03-web/task-6-report.md`.
- `revised` — `crates/agentops-server/src/jobs.rs`,
  `crates/agentops-server/tests/shutdown.rs` — Task 6 fix round 1, responding to
  the team lead's review. After confirming the previous report ("mutation 1 broke
  nothing") that `shutdown_closes_the_gate_before_cancelling` does not verify
  the ordering its name claims, the team lead did not approve leaving it — by
  CLAUDE.md's rule, a guard that is named and green with no detection power is
  worse than no test. Following the same pattern as `stream.rs::Hook`, a
  `ShutdownHook` type and
  `shutdown_with_hook(&self, deadline, hook: Option<ShutdownHook>)` were added so
  a test can interpose **between** shutdown stages 1 and 2 (`shutdown()` is now a
  thin wrapper calling `shutdown_with_hook(deadline, None)` — the production
  signature is unchanged). The test was rewritten to attempt `spawn()` inside
  the hook and observe the gate and cancellation state — under the correct order
  the gate is already closed and the spawn is refused, whereas with the order
  inverted cancellation has already fired while the gate is still open, so
  `spawn` succeeds and the task, holding an already-cancelled token, takes the
  already-ready cancellation branch before it even calls `mark_running` (a real
  database round trip that is necessarily pending on first poll). Re-applying
  mutation 1 (swapping just the two statements, leaving the hook call in place)
  failed as predicted (reproduced five consecutive times with
  `--test-threads=1`, deterministically failing at the same point) — the team
  lead's overrule was correct. Mutation 2 (splitting the terminal row in
  `agentops-store::fail_investigation`) was re-verified identically to the
  previous round with the same result. Both mutations reverted byte-exact (empty
  `git diff --stat`). Full workspace 209 tests passing, clippy clean,
  `check_spec_test_ids.py` holds at 24. Details in the fix round 1 section of
  `.superpowers/sdd/2026-08-03-web/task-6-report.md`.
- `added` — `crates/agentops-server/src/watchdog.rs` (new), `src/main.rs` (boot
  cleanup and watchdog startup) — implemented plan 3 Task 7 (the two-layer
  zombie-investigation defense). `sweep_once` performs one watchdog sweep as
  `stale_running_ids(idle)` (interpreted on the database clock) →
  `fail_investigation` for reclamation (F1: one transaction writes the status
  and the terminal step together) → `bus.publish_terminal`, and `spawn_watchdog`
  runs that periodically, sharing a `cancel: CancellationToken`.
  `recover_on_boot` marks orphaned `running` investigations `failed` and
  reschedules `queued` ones, wired into `main.rs` to run **before the server
  starts listening** — in the opposite order, requests arriving during cleanup
  race with its targets. Reused the existing `jm` assembled by Task 6 (a second
  `JobManager` would split the `JoinSet` and make graceful shutdown wait for
  only half). **Two brief defects**: (1) the `recover_on_boot` pseudo-code
  signature stepped into the very trap the brief itself warned about (a missing
  generic argument — `JobManager` instead of `crate::jobs::JobManager`), fixed
  to `JobManager<PgStore>`; (2) the `backdate` helper assembled a SQL string
  dynamically with `format!`, which this repository's sqlx 0.9 rejects at
  compile time through `SqlSafeStr` — changed to take `ago: Duration` instead of
  `interval: &str` and pass it as a `make_interval(secs => $1)` bind parameter,
  the same way `stale_running_ids` does. **Three mutations, one of which
  diverged completely from the prediction**: mutation 1 (splitting
  `fail_investigation` into `append_step(Terminated)` plus a separate `UPDATE`;
  brief prediction: `f1_...` fails with two terminal steps) — **measurement: all
  six tests passed unchanged.** The cause is that this test is a single
  execution path and never actually races a second writer — the defect F1
  prevents (lose the race, and the status is someone else's while the terminal
  row is yours) is observable only under concurrency, and
  `f1_reclaimed_investigation_has_exactly_one_terminal_step` constructs no such
  concurrent execution. **This test verifies only that the normal path leaves
  exactly one terminal step, not that F1 holds** — actually detecting F1 needs a
  separate `tokio::join!` construction like `jobs.rs::test_13_...`, which is
  outside this task's scope, so it is left as-is and handed to the final review.
  Mutation 2 (`stale_running_ids(idle)` → `stale_running_ids(Duration::ZERO)`)
  failed `a_fresh_running_investigation_is_left_alone` as predicted. Mutation 3
  (remove `bus.publish_terminal(id)`) had detection power but in a different
  shape — not a clean failure but a **hang**, with `rx.recv().await` blocking
  without a timeout until `cargo test` itself printed a warning after 60 seconds
  and the process had to be killed with `kill -9` — without a global timeout in
  CI this defect appears as "hangs forever" rather than "fails fast". All three
  mutations were verified applied and reverted byte-exact by `md5` plus a copy
  `diff` rather than `git diff`, since these are new untracked files. M2 (plan
  2 carry-over: can steps still be appended to a terminated investigation?) —
  measurement: yes, they can (the store has no status guard). As instructed the
  behavior was not changed; the current behavior was pinned by
  `m2_appending_to_a_terminal_investigation` and triage handed to the final
  review. Removing the deferral marker on TEST-8 in the spec's Section 12.1
  raised `check_spec_test_ids.py`'s scope from 24 to **25**. Full workspace 215
  tests (209 plus 6 new) passing, `cargo test --doc --workspace` unchanged at 0,
  clippy clean. Details in
  `.superpowers/sdd/2026-08-03-web/task-7-report.md`.
- `revised` — `crates/agentops-server/tests/watchdog.rs` — Task 7 fix round 1,
  on the team lead's instruction. (1) Added
  `f1_concurrent_reclaim_and_another_terminal_writer_leave_exactly_one_winner`,
  which launches the watchdog's reclamation (`sweep_once`) and a direct
  `fail_investigation` call imitating shutdown against the same investigation
  with `tokio::join!`, checking that exactly one wins and that the winning
  reason matches the terminal step (the `jobs.rs::test_13_...` pattern).
  **Mutation 1 (the F1 violation) was re-tested against this new test and went
  undetected 15 consecutive times** — the cause, measured with `--nocapture`, is
  not luck but a structural bias: the split write (mutation 1) takes only two
  round trips (insert plus an unconditional UPDATE, no transaction), whereas an
  honest `fail_investigation` takes four (`BEGIN` → conditional `UPDATE` →
  `INSERT` → `COMMIT`), so in a test environment with effectively zero latency
  the split write finishes the status transition entirely first every time, and
  the honest side's conditional `UPDATE` sees an already-failed row and loses
  with `Conflict` every time — "exactly one won, and the reason matches" holds
  by accident on every run even with the defect present. **Conclusion: a
  whole-call race at the `tokio::join!` level cannot detect F1** — the real
  defect occurs in the window between the split write's two statements, and
  opening that window deterministically requires a synchronization point like
  `jobs.rs::ShutdownHook` or `stream.rs::Hook` inside `sweep_once`'s structure,
  which is outside this task's scope. As the team lead predicted in advance
  ("meaning F1 is unverifiable at this level"), the test was not reshuffled into
  passing; it is recorded as it stands and handed to the final review. The test
  still pins the contract under real contention in a correct implementation
  ("exactly one wins and the reason matches") and prevents regressions — for
  instance, removing `WHERE status='running'` from the conditional `UPDATE`
  makes the round-trip difference irrelevant and this test catches it
  immediately. (2) Wrapped `rx.recv()` in
  `reclaimed_investigations_notify_their_subscribers` with
  `tokio::time::timeout(20s)` (the same pattern as
  `stream.rs::collect_stream_for_test`, panicking with a name for what failed to
  arrive) — re-testing mutation 3 (removing `bus.publish_terminal`) now ends in
  a clean failure at 20.21 seconds rather than a hang. After both re-tests,
  `src/watchdog.rs` was restored byte-exact by diff and copy comparison. Full
  workspace 216 tests (215 plus 1 new) passing, `cargo fmt --all -- --check`
  clean, clippy clean, `check_spec_test_ids.py` holds at 25. Details in the fix
  round 1 section of `.superpowers/sdd/2026-08-03-web/task-7-report.md`.
- `added` — `crates/agentops-server/templates/{base,incidents}.html`,
  `src/routes/pages.rs`, `static/{htmx.min.js,sse.js,app.css}`,
  `tailwind.{config.js,src.css}`, `tests/pages.rs` — implemented plan 3 Task 8
  (askama templates and the static asset foundation). The spec's Section 4.2
  three-pane layout (nav, chat aside, content main) is expressed as `base.html`
  plus `{% block %}`, and `/incidents` renders through an `IncidentsPage` askama
  template. `/` is `Redirect::to("/incidents")` as a 303 See Other (spec
  Section 10.1). Vendored HTMX 2.0.4 (50,917 bytes) and htmx-ext-sse 2.2.2
  (8,896 bytes) — both confirmed by measuring size and header bytes not to be
  misdetected 404 HTML. Static serving is wired in `routes/mod.rs` with
  `ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))` to avoid the
  working-directory mismatch between `cargo test` and `cargo run`.
  **Brief defect**: the Tailwind CLI syntax in Step 2 (`-c tailwind.config.js`
  plus `@tailwind base/components/utilities;`) is for v3, but the standalone
  binary actually obtained from the "latest" release is **v4.3.3** — the `-c`
  flag does not exist in the v4 CLI (absent from `--help`), is silently ignored
  when passed (exit 0 even pointing at a nonexistent file), and building with
  that syntax succeeds without error while never reading the `content` array, so
  every utility class the templates use is absent from the output CSS — the same
  shape as the defect this project keeps worrying about ("a branch exists but
  verifies/produces nothing"), reproduced here in an infrastructure script.
  Fixed by moving `tailwind.src.css` to v4 syntax (`@config
  "./tailwind.config.js"; @import "tailwindcss";`) and removing `-c` from the
  build command — `tailwind.config.js`'s `content` remains valid, and
  `text-xl`, `bg-neutral-950`, `space-y-2` (a compound selector), and
  `hover:text-white` (an escaped selector) were all confirmed present in the
  output by grep. Node was not installed (standalone binary only). Added
  `tower = "0.5"` to `[dev-dependencies]` with a pinned version (not registered
  in the workspace, following the `futures-util` pattern).
  **Mutation check (brief Step 7)**: replacing `/static/htmx.min.js` in
  `base.html` with a CDN URL — since these are new untracked files `git diff` is
  meaningless, so the substitution was confirmed applied with `grep` before
  re-running; `pages_do_not_reference_any_cdn` failed alone as predicted (the
  other two unaffected), and after a byte-exact restore verified by `diff`
  against an original copy, all three passed again — detection power confirmed.
  Full workspace 219 tests (216 plus 3 new) passing, `cargo test --doc
  --workspace` 0 failures, clippy clean, `check_spec_test_ids.py` holds at 25
  (new tests are not subject to spec ID traceability). Details in
  `.superpowers/sdd/2026-08-03-web/task-8-report.md`.
- `revised` — `README.md` — Task 8 review, Important 1. The review pointed out
  that the Tailwind download command still pointed at `releases/latest`, leaving
  it able to reproduce in the next major version exactly the failure this task
  had just documented by measurement ("latest silently gave v4 against a v3
  brief assumption, exit 0, zero utility classes") — the story was written down
  but no guard against recurrence was installed. Two fixes: (1) pinned the
  download URL from `releases/latest` to `releases/download/v4.3.3` (measured:
  `curl -o /dev/null -w "%{http_code}"` → 200); (2) embedded output verification
  in the build command block itself,
  `grep -q '\.text-xl{' static/app.css || { echo ...; exit 1; }`, against the
  possibility that a future major reintroduces the same failure through a
  different flag — the point of this round being that pinning a version does not
  guarantee *what was produced*. The pinned binary was downloaded fresh and the
  README's command block reproduced exactly: the `grep` check passed and the
  regenerated CSS was byte-identical to the already-committed `static/app.css`
  (empty `diff`, both 8,211 bytes) — confirming documentation and committed
  artifact agree, so `static/app.css` itself was not rewritten. Important 2 (a
  CDN reference inside a CSS `@import` is outside
  `pages_do_not_reference_any_cdn`'s field of view — the review demonstrated
  this by mutation, with all three tests unaffected) was judged by the team lead
  to need no CI guard and to belong in the final review ledger, so no code
  changed this round. No Rust code, test, or static asset changes (README only)
  — full workspace 219 tests unchanged, clippy clean, `check_spec_test_ids.py`
  holds at 25. Details in the fix round 1 section of
  `.superpowers/sdd/2026-08-03-web/task-8-report.md`.
- `added` — `crates/agentops-server/src/chart.rs` (new),
  `src/routes/investigations.rs` (new), `templates/investigation_list.html`
  (new), `tests/investigations.rs` (new), and modifications to `src/lib.rs`,
  `src/routes/{mod.rs,pages.rs}`, `src/main.rs` — implemented plan 3 Task 9
  (investigation launch, the list fragment, the seven-day frequency chart).
  `POST /api/investigations` creates the row, calls `AppState.jobs.spawn(id)`
  (the `JobManager<PgStore>` Task 6 assembled, reused from `main.rs`), and
  immediately sends a `303 See Other` to `/investigations/{id}` (INV-1 — it does
  not wait for the investigation). The title is cut at a character boundary with
  `prompt.chars().take(80).collect()` (a byte slice panics on multi-byte input).
  Added `pub jobs: jobs::JobManager<PgStore>` to `AppState` — the two internal
  test modules that construct `AppState` directly (`tests/pages.rs` and
  `src/routes/health.rs`) were updated with a placeholder `LlmProvider` that is
  never spawned onto. `chart::daily_bars` always emits seven bars even with no
  data and normalizes to the maximum so it never exceeds the viewBox (H=100).
  `pages::incidents` fills `chart_svg` from a seven-day `queued_at::date`
  aggregate query (a query failure falls back to an empty vector and is recorded
  with `tracing::error!` — not swallowed). **One brief defect**: Step 1's
  `the_list_fragment_escapes_user_supplied_text` expected `&lt;script&gt;` as
  the escaped form, but askama 0.16's default escaper in this repository emits
  numeric character references (`&#60;`/`&#62;`) — measured: the response body
  contains `&#60;script&#62;alert(1)&#60;/script&#62;`. The escaping itself was
  correct; only the literal was wrong, so the second assertion was corrected to
  `&#60;script&#62;` (the first assertion, that the raw markup must be absent,
  was left as-is — that is the actual security property). **Three mutations**:
  mutation 1 (`Redirect::to` → `Html(...)` 200, changing the return type to
  `Response`) failed `creating_an_investigation_redirects_with_303` alone as
  predicted (`left: 200, right: 303`). Mutation 2 (`st.jobs.spawn(id)` →
  blocking) — the brief specified `run_investigation(...).await`, but that
  requires assembling all of `RunnerDeps` (`PolicyGate`, `McpToolRegistry`,
  `DeltaSink`) inside the handler, so the same defect shape (the handler waits
  for the investigation) was reproduced with
  `tokio::time::sleep(500ms).await` instead — `inv_1_create_returns_while_...`
  failed alone as predicted (violating a 300ms deadline). Mutation 3
  (`{{ inv.title }}` → `{{ inv.title|safe }}`, testing the security assertion's
  detection power) failed `the_list_fragment_escapes_user_supplied_text` alone
  as predicted, confirming the corrected assertion retains real detection power.
  All three verified byte-exact against `/tmp` copies with `diff`. Full
  workspace 225 tests (219 plus 6 new) passing, `cargo fmt --all -- --check`
  clean, clippy clean, `check_spec_test_ids.py` holds at 25. Because
  `ListFilter` has no search field, the search the spec's Section 10.2 requires
  was not implemented and was left with a code comment for final review triage
  (as the brief explicitly directed). Details in
  `.superpowers/sdd/2026-08-03-web/task-9-report.md`.
- `revised` — `crates/agentops-server/src/chart.rs`,
  `src/routes/investigations.rs`, `tests/investigations.rs` — Task 9 review
  (Important 1, Minors 3 and 4). The review pointed out that `daily_bars` filled
  `counts` by array position rather than by actual date, so combined with
  `pages::incidents`'s `GROUP BY d` query returning no row at all for empty
  days, the values shifted wholesale (reproduced: with only two entries, six
  days ago and today, today's value was drawn in the "five days ago" slot).
  Fixed to use `OffsetDateTime::now_utc().date()` as "today" and place each
  value in the `(today - date).whole_days()` offset slot, and to discard rather
  than misplace dates outside the seven-day window while emitting a
  `tracing::warn!` (the function signature is a brief contract and was kept).
  Added `counts_are_bucketed_by_actual_date_not_array_position` — detection
  power confirmed by a mutation reverting to the old array-position code, which
  failed with output byte-identical to the review's manual reproduction
  (`[55.6, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0]`), then restored byte-exact. As a
  side fix, the fixed literal date (`2026-08-01`) in
  `bars_are_scaled_within_the_viewbox` was made relative to "today" — with a
  seven-day window now anchored on "today", a fixed date becomes a time bomb
  that silently always passes while verifying no scaling at all the moment it
  falls more than seven days behind the real clock (not a review finding; this
  fix created the problem, so it was handled here). Minor 3: `create()`'s
  `map_err(|_| ...)` discarded the `StoreError` — aligned with the
  `tracing::error!` pattern of `list_fragment` and `pages::incidents`. Minor 4:
  `let _ = st.jobs.spawn(id)` silently swallowed `GateClosed` — logging added
  (the response itself is unchanged, since that is the brief's pseudo-code
  behavior; the contract that a row created during the shutdown window waits for
  the next boot's requeue is preserved, with triage handed to the final review).
  Two corrections for the record (no code change): as the review reproduced,
  mutation 1 (`Redirect` → 200) breaks two tests, not one
  (`creating_an_investigation_redirects_with_303` plus
  `inv_1_create_returns_while_...`) — the previous round's "failed alone" was
  inaccurate. And the substitute for mutation 2 (`sleep(500ms)`) proves a weaker
  property rather than "the same defect shape", as the review noted and this
  round agrees.
- `revised` — `crates/agentops-server/src/routes/pages.rs`,
  `tests/investigations.rs`, `tests/pages.rs` — Task 9 re-review (Important,
  re-examining `task-9-review.md`). The finding: `pages::incidents`'s SQL
  (`WHERE queued_at >= now() - INTERVAL '7 days'`, a 168-hour rolling window on
  the clock) and `chart::daily_bars`'s calendar window (`days_ago in 0..=6`)
  disagree at the boundary — since `now()` is essentially never midnight, this
  query returns rows dated `today - 7 days` on almost every day, and
  `daily_bars` discards them as outside the window while emitting a
  `tracing::warn!` every time (re-review measurement: `now_utc: 2026-08-04
  04:19:45, cutoff: 2026-07-28 04:19:45, days_between: 7`) — the central point
  being that a routine warning on healthy data cries wolf and destroys the
  signal value of the log. Fixed by narrowing the SQL to a calendar basis
  (`queued_at::date >= now()::date - INTERVAL '6 days'`), aligning the two
  windows. Of the two directions the team lead offered (narrow the SQL versus
  widen `daily_bars` to `0..=7`), the former was chosen: the latter creates a new
  ambiguity with eight dates competing for seven slots, and its boundary would
  still depend on the SQL's clock basis, merely relocating the problem, whereas
  narrowing the SQL makes the query itself always return exactly seven calendar
  dates so no ambiguity can arise. `daily_bars`'s `tracing::warn!` was kept — it
  is a `pub` function that may acquire other callers, and now that the SQL
  aligns the window this warning cannot fire on healthy data (if it fires, it is
  a genuine anomaly), so the warning has found its proper place. Two new tests
  pin the boundary at two layers: (1) the `chart.rs` unit test
  `daily_bars_does_not_warn_at_the_window_boundary_but_does_just_outside`
  (replicating for this file the `BufWriter` plus
  `tracing::subscriber::with_default` pattern from `routes/health.rs`) asserts
  on log content directly — the inside edge (six days ago) lands in the first
  slot without a warning, and just outside (seven days ago) actually warns.
  Mutation (remove the warning) confirmed failing, then restored byte-exact.
  (2) The `pages.rs` integration test
  `the_chart_shows_investigations_from_the_oldest_day_in_the_window` seeds a
  six-day-old investigation into a real Postgres and calls `/incidents` for
  real, confirming it appears in the rendered SVG's first slot. Mutation
  (`INTERVAL '6 days'` → `'5 days'`) confirmed failing, then restored
  byte-exact. **Limitation stated honestly**: the layer-2 test does not catch a
  revert of the SQL to the clock basis (`now() - INTERVAL '7 days'`).
- `revised` — `crates/agentops-server/src/chart.rs`, `src/routes/pages.rs` —
  Task 9 third review (final round; re-examination of `task-9-review.md` came
  back ADDRESSED/CLEARED). The finding: what the previous round called a
  `chart.rs` unit test that "prevents the SQL reverting to the clock basis"
  actually calls only `daily_bars` and never goes through `pages.rs`'s SQL at
  all, so reverting that SQL to `now() - INTERVAL '7 days'` would trip no test —
  the team lead re-raised the limitation round 2's report had already stated
  honestly. The root cause is that the SQL's `INTERVAL '6 days'` and
  `chart::daily_bars`'s `0..=6` window were two separate literals meaning the
  same value — a shape this task had already been caught by twice. Fixed by
  opening `chart::BARS` (the bar count, previously private) to `pub(crate)` and
  having `pages::incidents` compute
  `let window_days = crate::chart::BARS as i32 - 1;` and bind it into
  `make_interval(days => $1)`, so the date range the SQL actually returns and
  `daily_bars`'s window both derive from a single constant — structurally
  removing any way for two literals to drift apart again. The `make_interval`
  pattern follows `agentops-store::investigations::stale_running_ids`, which
  already uses `secs => $1` for the same reason (this repository's sqlx blocks
  dynamic SQL string assembly at compile time) — though `secs` is
  `double precision` while `days` is `int`, which was verified before use. The
  shorter alternative of `date - integer` with no function call was considered
  but rejected: introducing a second idiom alongside the established
  `make_interval` one costs more, so the existing pattern was followed (recorded
  as an easily reversible style choice if anyone disagrees). Mutation check
  (`window_days` as `BARS-2` instead of `BARS-1`) confirmed failing — with the
  same panic message as round 2's literal mutation (`'6 days'` → `'5 days'`),
  since the value now comes from a single source and touching either side
  produces the same symptom — then restored byte-exact. No new tests were added
  (only the mutation vector of the existing tests changed, from a literal to a
  derived value). For the record (final review triage, no code change): (1) the
  timezone agreement between the app's `now_utc()` and Postgres's `now()::date`
  holds only accidentally, because the database session timezone is currently
  UTC, with no code enforcing it (third-review measurement: `SHOW timezone` →
  `UTC`) — a connection configuration change could reintroduce a full-day
  mismatch; (2) the reasoning for `make_interval` over `date - integer`.
- `added` — `crates/agentops-server/templates/investigation.html` (new),
  `tests/detail.rs` (new), modifications to `src/routes/{pages.rs,mod.rs}` —
  implemented plan 3 Task 10 (the investigation detail page: server rendering
  plus embedding `after`, INV-2). `GET /investigations/{id}` fetches every
  existing step with `Store::steps_after(id, -1)`, renders them server-side with
  `render::step_html` (the detail page and SSE share the same function), and
  **reads `seq` directly from the last element of that list** to embed in the
  stream URL's `after` — it does not call `Store::max_step_seq` separately:
  doing so would structurally admit a window where a step committed between the
  two queries is absent from the page while `after` has already passed it,
  losing the replay. With no steps it embeds `after=-1` (`steps_after` uses
  `seq > after`, so embedding `0` would hide the first step forever). A missing
  investigation maps `get_investigation`'s `Err` straight to 404. Added the
  axum 0.8 path `/investigations/{id}` to `routes/mod.rs`. The spec's `[INV-2]`
  marker (Section 6.1) was already active from Task 3 (per the brief's
  "confirm only" instruction, no document change), and
  `check_spec_test_ids.py` holds at 25. **Two of three mutations diverged from
  the brief's predictions**: mutation 1 (fix `after` at `0`) — contrary to the
  prediction that `an_investigation_with_no_steps_embeds_after_minus_one` would
  fail alone, **`inv_2_detail_page_renders_steps_and_embeds_the_last_seq` also
  failed** (it also disagrees with the expected `after=2` when steps exist —
  unmentioned by the brief). Mutation 2 (`steps_html` as an empty string) failed
  `inv_2_...` plus `the_page_and_the_stream_render_steps_identically`, as
  predicted. Mutation 3 (output fields directly in the template instead of
  `render::step_html`) — **the brief's literal text (`{{ s.kind }}`) does not
  even compile**: `StepKind` implements neither `Display` nor `HtmlSafe`, and
  this askama (0.16) has no debug filter (meaning the type system happens to
  block exactly this regression, though other fields still route around it, so
  it is not a designed defense). Substituting `{{ s.seq }}` (which has Display)
  to reproduce the intent, **`inv_2_...` also failed** contrary to the
  prediction of `the_page_and_the_stream_render_steps_identically` alone (the
  substituted field carries no step text, so no `body0`-style literal remains in
  the body). All three verified applied and then restored (tracked files by
  `git diff --stat`, the new untracked template by direct comparison with
  `Read`).
- `revised` — `crates/agentops-server/src/routes/pages.rs` — Task 10 review
  (Important). `task-10-review.md` pointed out that
  `investigation_detail`'s `get_investigation` call collapsed **every**
  `StoreError` — including `Backend`, a genuine database failure — into a
  logless 404 with `.map_err(|_| StatusCode::NOT_FOUND)`, while the
  `steps_after` call four lines below already correctly logged with
  `tracing::error!` and returned 500. Two adjacent calls in the same function
  behaved differently. The operational consequence: while the database is
  temporarily unstable, an investigation that exists appears to be missing and
  there is no log to read the cause from — contradicting both the plan's
  convention ("never swallow database errors") and the line directly below.
  Changed to `StoreError::NotFound` → 404 and everything else → log then 500,
  matching `steps_after`. The review also reported that in this round it
  independently reproduced Step 5's mutations 1 and 3 and confirmed they matched
  the earlier report, established that mutation 3's `s.seq` substitution really
  does detect divergence (the incidental `inv_2_...` failure being a separate
  real effect), and self-verified both that Task 3's
  `close_if_investigation_is_terminal` applies unconditionally to every
  `investigation_stream` caller — making this page's unconditional
  `sse-connect` rendering safe — and that `stream.rs`'s existing `max_step_seq`
  plus `clamp_after` does not reintroduce the same two-query loss window this
  task avoided (it only narrows the cursor downward, so steps inside the window
  still satisfy `seq > after`). **No test was added — the reasoning is recorded
  honestly**: provoking a non-`NotFound` `StoreError` at this point requires a
  failure-injection wrapper over the `Store` trait's twenty methods, and while
  `tests/jobs.rs::FailingStore` already implements that pattern, it plugs only
  into the generic `JobManager<S: Store>`, whereas the `AppState` and
  `routes::router` this handler passes through are pinned to the concrete
  `Arc<PgStore>` and `JobManager<PgStore>` (`lib.rs:19-23`) — testing it over
  HTTP would require making `AppState` and `JobManager` generic, an
  architectural change judged too expensive for a one-line error mapping. This
  branch is not directly tested and is supported only by having the same shape
  as the `steps_after` handling four lines above. All four existing tests still
  pass. Full workspace 232 tests (unchanged, no new tests), `cargo fmt --all`
  clean, clippy clean, `check_spec_test_ids.py` holds at 25. Task 10 closed.
- `added` — `crates/agentops-server/src/routes/instructions.rs` (new),
  `templates/{instruction_list,knowledge,artifacts,artifact,settings}.html`
  (new), `tests/knowledge.rs` (new), modifications to
  `src/routes/{mod.rs,pages.rs}` — implemented plan 3 Task 11 (the Knowledge,
  Artifacts, and Settings pages, plus Instructions CRUD). `list_fragment`
  (`GET /api/instructions`) calls `instructions_for` with all five phases
  (`ALL_PHASES`) to prevent a UI-versus-prompt mismatch, and `create`/`update`
  (POST/PUT) share an internal `upsert(st, id, f)`, differing only in where the
  id comes from. `artifacts`, `artifact_detail`, and `settings` were added to
  `pages.rs` — `settings.html` is read-only with no `<form>`, per the spec's
  Section 10.1. Because `Phase` and `McpTransport` do not implement `Display`
  (verified in the code — only `InvestigationStatus` does), the templates call
  `.as_str()` directly. **Brief defect**: the Step 1 test code puts
  `phase=All` (capitalized) in the form, but `Phase: FromStr` parses lowercase
  only, so left as written every POST fails with `BAD_REQUEST` and
  `instructions_are_listed_in_deterministic_order` panics at its `unwrap()` —
  written as `phase=all`. **A detection-power gap in the brief's test was
  measured and independently reinforced**: that ordering test seeds all three
  items as `Phase::All`, so cutting `ALL_PHASES` down to just `[Phase::All]`
  still shows every item and catches nothing — measured: the first mutation run
  passed all six, contrary to the prediction of a missing-item failure. Added
  `list_fragment_enumerates_every_phase` (seeding one per phase and confirming
  all appear), after which re-running the mutation failed as predicted on the
  missing `Chat` item — that new test is now the mutation's actual detector.
  Mutation 2 (`{{ i.body }}` → `{{ i.body|safe }}`) failed
  `instruction_bodies_are_escaped_in_the_list` alone, as predicted. Both
  mutations were verified applied and reverted with `grep` (the brief warned,
  for the same reason as earlier tasks, that `git diff` is meaningless for new
  untracked files). Independently added: `an_unknown_artifact_is_not_found`
  (exercising the 404 branch) and `list_fragment_enumerates_every_phase`
  (closing the gap above) — seven new tests in total.
- `revised` — `crates/agentops-core/src/traits.rs`,
  `crates/agentops-store/src/{instructions.rs,lib.rs}`,
  `crates/agentops-store/tests/instructions.rs`,
  `crates/agentops-server/src/routes/instructions.rs`,
  `crates/agentops-server/tests/{jobs.rs,knowledge.rs}` — Task 11 review
  (Critical). `task-11-review.md` pointed out that
  `PUT /api/instructions/{id}` reused `store.upsert_instruction`, which selects
  its update target by `ON CONFLICT (phase, title)` and silently discards the
  path's `id` (plan 1's intended behavior) — so if another row already holds the
  submitted `(phase, title)`, **that other row** is silently overwritten instead
  of the one the path pointed at, and the response is a 201. The original
  comment ("because it upserts by id") assumed the opposite, and that assumption
  came straight from the brief's text. The team lead chose authority option (b):
  add `update_instruction` to `Store` (targeting with `WHERE id = $1`, returning
  `Conflict`→409 on a `(phase,title)` collision with another row via the same
  `is_unique_violation()` idiom as `steps.rs::insert_step_strict`, and
  `NotFound`→404 for a missing id), with delegating methods on `PgStore` and the
  test `FailingStore` (both `Store` implementors) — being a new trait method, the
  compiler forced both implementors to be updated. `routes/instructions.rs::update`
  now uses it, and the misleading comment was deleted. Done by TDD: the
  regression tests (three at the HTTP layer in `tests/knowledge.rs`, three at
  the store layer in `tests/instructions.rs`, six total) were written first and
  confirmed to fail exactly as predicted against the unfixed bug — the Critical
  scenario (rows A and B; a PUT carrying B's id with A's `(phase,title)`) failed
  with `left: 201, right: 409`, and a PUT to a nonexistent id failed with
  `left: 201, right: 404` — then the fix turned all six green.
  **Two mutations**: (1) reverting `update` to call `upsert_instruction` failed
  exactly the same two tests as the pre-fix reproduction, with no other effects;
  (2) removing the store's `is_unique_violation()` discrimination so a unique
  index violation collapses into `Backend(String)` instead of `Conflict` failed
  the store test `update_that_collides_with_another_rows_identity_is_a_conflict`,
  and that failure propagated up to the route layer, failing
  `put_never_rewrites_a_different_row` as well (500 versus the expected 409) —
  measuring that a store-layer defect leaks all the way to the HTTP layer. Both
  mutations were compared byte-exact against copies with `diff` before
  reverting. Because a Task 12 (chat) agent was concurrently modifying
  `bus.rs`, `render.rs`, and the new `routes/chat.rs` in this worktree, those
  were left untouched as instructed and `git add` named only the seven files
  changed here. Full workspace 245 tests (239 before plus 6 new) passing,
  `cargo fmt --all -- --check` clean, clippy clean,
  `check_spec_test_ids.py` holds at 25. Task 11 closed. Details in the fix
  round 1 section of `.superpowers/sdd/2026-08-03-web/task-11-report.md`.
- `added` — `crates/agentops-server/src/routes/chat.rs` (new),
  `templates/chat_panel.html` (new), `tests/chat.rs` (new), modifications to
  `src/{bus.rs,jobs.rs,render.rs,routes/mod.rs,templates/base.html}` —
  implemented plan 3 Task 12 (chat: sessions, sending, the token stream; spec
  Section 6.2; the last task of plan 3). Added `ChatEvent`
  (`Delta`/`Message`/`Terminal`) to `bus.rs` along with a separate map (`chat`)
  distinct from the investigation channels (`channels`) — putting both in one
  map keyed by UUID would structurally allow two streams to mix if a session ID
  and an investigation ID ever coincided. `send` stores the user message
  **first**, then spawns response generation and returns immediately (the same
  reason as INV-1). Because `run_phase_loop` requires an `investigation_id` and
  cannot be used for chat, a thin loop (`reply`) calling `LlmProvider::stream`
  directly was written, sending `tools: Vec::new()` so v0.1 chat has no tools
  (with tools there would be nowhere to record a `ToolCall` without
  `agent_steps`). **Added `spawn_task` to `JobManager`** so the response
  generation task joins the same `JoinSet` as investigation tasks — applying the
  "no detached tasks" convention to chat as well; unlike `spawn(id)` it does not
  check the gate (the user message is already stored, so refusing here would
  strand it with no response). `stream` follows the same pattern as the
  investigation SSE but **has no replay** (deltas are not persisted) —
  `tokio_stream::wrappers::BroadcastStream` was avoided because it needs the
  tokio-stream `sync` feature the workspace does not enable, replaced by the
  same spawn-plus-mpsc convention as `crate::stream::async_stream_impl`.
  **`ChatEvent::Terminal` does not close the connection** — unlike an
  investigation, a chat session is not one-shot, so it keeps waiting for the
  next exchange. **A judgment call (the brief put session-creation UI out of
  scope)**: `base.html` is a static template shared by every page and has no way
  to know a session ID, so a fixed constant (`routes::chat::DEFAULT_SESSION`)
  was hardcoded with `panel` creating the session lazily — and the drift guard
  `default_session_id_matches_the_literal_wired_into_base_html` was added so the
  literal in `base.html` and the constant cannot diverge. Explicitly flagged for
  final review triage. `routes/mod.rs` was not on the brief's file list but is
  required for route registration, so it was modified too (judged a
  documentation omission; `lib.rs` was named by the brief but needed no change
  and was left alone). **Two mutations**: (1) moving the user-message store
  inside the spawned task (after response generation begins) failed
  `the_user_message_is_persisted_before_the_reply_starts` as predicted (`0 != 1`)
  and **also failed `sending_to_a_missing_session_is_not_found`, which was not
  predicted** (moving the store into the task takes the `NotFound` error mapping
  out of the response path too, so it always returns 200) — the measurement is
  reported as observed rather than as predicted. (2) The brief's "remove the
  chat map and share channels" does not type-check in one line, because
  `Sender<BusEvent>` and `Sender<ChatEvent>` are different types, so the nearest
  behavioral equivalent was performed instead (one line making `publish_step`
  also feed the chat channel of the same id) — failing
  `chat_and_investigation_channels_are_separate` alone, as predicted. Both
  mutations confirmed byte-exact reverted with `git diff`. Beyond the brief's
  four tests, three were added independently covering escaping (spec
  Section 13, on both the `panel` and `send` echo paths) and a 404 for a
  nonexistent session (a global convention), plus one drift guard — eight new in
  total, because the brief's four tests verified the escaping requirement not at
  all. A test comment notes that `render::esc` uses named entities
  (`&lt;`/`&gt;`) unlike askama (it reuses the same functions as `step_html` and
  `delta_html`). Full workspace 253 tests (baseline 245 at HEAD `161aef2` plus 8
  new) passing — the baseline was 245 rather than the 239 the brief stated
  because another agent in this session had already landed the Task 11 review
  fix (`161aef2`). `cargo test --workspace --doc` 0 tests across all four crates
  (unchanged), `cargo fmt --all --check` clean, clippy clean,
  `check_spec_test_ids.py` holds at 25 (new tests are not subject to spec ID
  traceability), `check_stale_after.py` OK. Plan 3 closed. Details in
  `.superpowers/sdd/2026-08-03-web/task-12-report.md`.
- `revised` — `crates/agentops-server/src/{bus.rs,jobs.rs,routes/chat.rs}`,
  `tests/chat.rs` — Task 12 review (Important 1, Minor 1).
  `task-12-review.md` pointed out that `spawn_task`'s justification for skipping
  the gate ("a user message already stored would be stranded by a refusal") did
  not match the actual shutdown wiring in `main.rs` —
  `axum::serve(...).with_graceful_shutdown(shutdown)` closes the listener only
  when the `shutdown` future **completes** (that being all of `ctrl_c().await`
  followed by `jm2.shutdown(deadline).await`), so the HTTP listener keeps
  accepting new connections for the several seconds `JobManager::shutdown` runs.
  `close_gate()` (stage 1) and `cancel.cancel()` (stage 2) are nanoseconds
  apart, so a response task spawned in that window starts holding an already
  cancelled token, wastes work opening `instructions_for`, `chat_messages`, and
  `provider.stream()`, and then cancels itself the instant it reaches the
  `select!` — the review's core point being that the outcome (a message with no
  response) is the same as honoring the gate, with only the wasted work added.
  Work began after tracing `main.rs:70-77` directly to re-confirm the review's
  trace. Added `JobManager::is_accepting()` (a thin getter reading the same
  `gate_open` field `spawn()` already checks) and changed `routes::chat::send`
  to check it **before storing** and refuse immediately with `503` when closed.
  Why `503` rather than the investigation path's `303` plus row creation (an
  open Task 9 triage item): investigations are requeued by `recover_on_boot` at
  the next boot, but chat has no such retry path, so refusing after storing
  would leave a message that can never receive a response — not storing in the
  first place is the only honest option. `spawn_task`'s documentation was also
  rewritten to strip the false justification and state the real reason (the gate
  check belongs to the caller, before it causes side effects, not to this
  function) — so as not to repeat the shape CLAUDE.md warns about, a false claim
  from a brief being copied verbatim into a code comment (the Task 11 Critical).
  The new test
  `sending_after_the_gate_closes_is_rejected_before_anything_is_saved` calls the
  already-public `close_gate()` (a method `tests/jobs.rs:382` already calls
  directly) synchronously to reproduce the window deterministically without a
  shutdown hook, and asserts not only the `503` but that `chat_messages` is
  actually empty (the property that matters more than the status code) —
  confirmed by a mutation removing the gate check that this test fails alone
  (`8 passed / 1 failed`, `200 != 503`), then restored byte-exact. Minor 1:
  documented on both types that `BusEvent::Delta` (raw text, rendered later by
  `stream.rs`) and `ChatEvent::Delta` (already-rendered HTML, pre-rendered by
  `routes::chat::reply`) are opposites in *when* rendering happens — unifying
  the types was not requested by the review and was not done. Minor 2 (no GC for
  chat channels) was downgraded by the review to "structurally limited to at
  most one, since there is no session creation route yet" and recorded as "a
  precondition for whichever task adds session creation", so no code changed.
  Full workspace 254 tests (253 plus 1 new) passing, `cargo fmt --all` clean,
  clippy clean, `check_spec_test_ids.py` holds at 25, `check_stale_after.py` OK.
  Task 12 and plan 3 finally closed.
- `revised` — `crates/agentops-server/src/{main.rs,render.rs,routes/chat.rs,stream.rs}`,
  `templates/{chat_panel.html,investigation.html}`, `tests/stream.rs`
  (modified), `tests/serve_shutdown.rs` (new) — responding to plan 3's final
  review (`final-review.md`): 1 Critical, 3 Importants, 1 Minor.
  **C-1 (Critical)**: an open SSE connection blocks graceful shutdown forever —
  the producer tasks in both `routes/chat.rs::stream` and
  `stream.rs::async_stream_impl` were detached via `tokio::spawn` and could not
  see cancellation at `sub.recv().await`, so they parked forever, the SSE body
  never ended, and axum's `close_tx.closed().await` waited indefinitely (the
  review confirmed this by independently reproducing it on axum 0.8.9). Both
  files were moved onto `JobManager::spawn_task` and their `sub.recv()` bound to
  `cancel_token()` with `select!`, so cancellation sends `terminal` and returns
  (the same shape as `reply()` lines 275-278). **Incidental discovery**: after
  adding the `StreamCtx.jobs` field, because `async_stream_impl` is a
  synchronous function with no `.await`, it returned immediately after calling
  `jobs.spawn_task(fut)` and dropped `ctx.jobs` (which Rust 2021 disjoint
  capture leaves uncaptured by `fut`), taking the `tasks: Arc<Mutex<JoinSet<()>>>`
  reference count to zero so `JoinSet::drop` immediately aborted the
  just-spawned task — measured as 8 of the 11 tests in `tests/stream.rs`
  failing instantly with empty results. Fixed by cloning `jobs` once more inside
  the spawned async block and capturing it explicitly with
  `let _keep_alive = keep_alive;` (applied identically in both files) — with a
  code comment recording the lesson that production merely hid this by accident,
  because axum's `Router` holds `AppState` for the server's lifetime, and one
  must not rely on that accident. **I-1 (Important)**: added
  `sse-close="terminal"` to `investigation.html` (the browser's `EventSource`
  was interpreting body EOF as a reconnect and replaying steps endlessly) — it
  was deliberately not added to `chat_panel.html`, since a chat session is not
  one-shot (see the `ChatEvent::Terminal` documentation), with a comment noting
  the asymmetry is intentional. **I-3 (Important)**: in
  `stream.rs::close_if_investigation_is_terminal`, separated
  `StoreError::NotFound` (a permanent fact) from `Backend` (a transient
  failure) so only the former closes the stream — the place where Task 10's
  404/500 distinction in `investigation_detail` had not been generalized. This
  closed a leak where opening with a nonexistent id leaked a 256-slot channel
  and a parked task on every request. **I-2 (Important)**: added
  `#[cfg(test)] mod tests` to `render.rs` — five tests confirming `step_html`
  escapes `ToolResult.output`, `Text.text`, `Thinking.summary`, `Error.message`,
  and `Terminated.detail` (there had previously been no test anywhere on this
  branch guarding `step_html` directly). **M-1 (Minor)**: added
  `wait_for_shutdown_signal()` to `main.rs` — under `#[cfg(unix)]` it binds
  `ctrl_c()` and `SIGTERM`
  (`tokio::signal::unix::signal(SignalKind::terminate())`) with `select!` so a
  `docker-compose.yml` deployment (spec Section 15) taken down by SIGTERM still
  runs the six-stage shutdown. **Four mutation verifications** — C-1 was
  reverted to its original (buggy) form and the new `tests/serve_shutdown.rs`
  confirmed failing on a 15-second timeout exactly as predicted (reproducing the
  pre-fix state); I-3's `NotFound` branch was deleted and
  `stream_opened_on_a_nonexistent_investigation_closes_instead_of_leaking`
  failed on a 10-second timeout; I-2's `esc(output)` at `render.rs:50` was
  reverted to `output` (the very mutation the review had measured) and
  `tool_result_output_is_escaped` failed alone (the other four unaffected) — all
  three restored byte-exact by `diff` and re-run green. Full workspace 262 tests
  (baseline 254 plus 8 new: 5 render, 2 stream, 1 serve_shutdown) passing,
  `cargo fmt --all` adjusted whitespace only, clippy clean,
  `cargo test --workspace --doc` 0 tests across all four crates (no drift),
  `check_spec_test_ids.py` holds at 25, `check_stale_after.py` OK. Details in
  `.superpowers/sdd/2026-08-03-web/final-fix-report.md`; the review text is in
  `final-review.md`.
- `revised` — `crates/agentops-server/src/{jobs.rs,routes/chat.rs,stream.rs}`,
  `tests/chat.rs` (modified) — responding to re-review N-1 (Important-latent).
  The previous round applied `keep_alive` (the pattern where a spawned async
  block clones the `JobManager` once more to hold it, preventing `JoinSet::drop`
  at reference count zero from aborting the task just spawned) in two places
  (`stream.rs::async_stream_impl` and `routes/chat.rs::stream`) but missed the
  third — the `reply()` spawned by `send` at `routes/chat.rs:198`. The re-review
  reproduced it through the real handler path: build `routes::router(state)`,
  `oneshot` a `POST /api/chat/{sid}/messages`, drop the router, and poll for
  four seconds — the assistant response was never stored. Confirmed from source
  that `tower-0.5.3`'s `Oneshot::poll` drops `svc` (and therefore
  `Router` → `AppState` → `JobManager`) right after calling `Service::call`,
  long before the response completes. That production is quiet about it only
  because axum's `Router` holds `AppState` for the server's lifetime is exactly
  the lesson the previous round wrote down itself — **the very commit that
  recorded that lesson was caught by it in one place.** **The fix**: rather than
  leaving `keep_alive` as each of three callers' responsibility, it was absorbed
  into `JobManager::spawn_task` (`let keep_alive = self.clone();` once at spawn
  time, wrapping the body as `let _keep_alive = keep_alive; fut.await;` — a
  three-line net addition), eliminating the recurrence path that depends on
  caller discipline. The `keep_alive` each of the three call sites built was
  deleted, and the site that had been missed was fixed with no code change of
  its own. **New test** (which could not have existed before the fix):
  `tests/chat.rs::the_reply_task_survives_the_router_that_spawned_it_being_dropped`
  builds a real `Router`, `oneshot`s one request, and polls `chat_messages` for
  four seconds to confirm both the user and assistant messages are stored.
  **Mutation verification**: deleting the two absorbed `keep_alive` lines from
  `spawn_task` and confirming with `grep` that they were gone from the code
  (only the comment text remaining), then running — failing exactly as predicted
  with `left: 1, right: 2` (4.21 seconds, polling to the four-second deadline
  before failing), then restored byte-exact by `diff` and passing again. Full
  workspace 263 tests (262 plus 1 new) passing, `cargo fmt --all` no changes,
  clippy clean, `cargo test --workspace --doc` 0 tests across all four crates,
  `check_spec_test_ids.py` holds at 25, `check_stale_after.py` OK. N-2 and N-3
  were deferred to triage on the re-review's judgment that they are guard
  precision issues rather than merge blockers, and no code was touched for them
  this round. Details in the re-review round section of
  `.superpowers/sdd/2026-08-03-web/final-fix-report.md`.

## After the merge — documentation and guards

- `added` — `docs/superpowers/known-limitations.md` — promoted the 16
  carry-overs from plans 1 through 3, plus the process limitations, into a
  durable document. The review artifacts (`.superpowers/sdd/`) are gitignored
  scratch and vanish with the worktree. Includes the F1 structural audit
  command, the three questions to ask when closing a defect, and the three
  instances of "reporting success while doing nothing".
- `revised` — CLAUDE.md — added the convention that F1 (one writer per terminal
  row) is protected by a **structural audit**. Plan 3 measured that a race test
  cannot catch it (15 runs by the implementer and 20 by the reviewer, all false
  passes; the cause being a structural speed difference from the round-trip
  count). Records the audit command and the location of the current three
  matches.
- `revised` — CLAUDE.md — updated "current state" to plan 3 complete (4 crates,
  263 tests) and linked to known-limitations.md.
- `verified` — `docs/superpowers/plans/2026-08-03-web.md` — promoted plan 3 from
  `draft` to `stable` and recorded two machine verifications in `verified[]`:
  the final whole-branch review (first pass Needs work — Critical 1 (an open SSE
  connection blocks graceful shutdown forever, independently reproduced on axum
  0.8.9), Important 3, Minor 1; CLEARED after all were fixed, `e96e5c9`) and the
  scoped re-review (N-1 Important-latent — `keep_alive` missing at the third
  spawn site `routes::chat::send`, fixed by absorbing it into `spawn_task`,
  `94c818d`). **Why it was missed**: plans 1 and 2 received the same treatment in
  `f58be95` (2026-08-02 20:46), but that was closed as an *instance* and never
  promoted to a rule, and plan 3 was born `draft` sixteen hours later in
  `d0f951d` (2026-08-03 12:42) — after the audit had already passed. Another
  instance of the "a fix cannot see multiplicity either" shape CLAUDE.md warns
  about.
- `revised` — `docs/index.md` — filled the document list table with seven
  documents. It had not been updated since `9c94e22` (2026-07-30, the adoption
  of OKF), listing only the spec while omitting three plans and
  `known-limitations.md` — not a plan-3-specific omission but a continuous one
  since adoption. Stated under the table why CI stays green with a stale table
  (`check_stale_after.py` globs `docs/**/*.md`, not the table). No
  index-completeness guard was added — awaiting the user's decision.
- `added` — `scripts/check_index_complete.py`, `.github/workflows/ci.yml`
  (modified) — the `drift-guards` job now checks that `docs/index.md`'s document
  table agrees with the actual bundle. Three things: (1) every `docs/**/*.md`
  with frontmatter appears in the table, (2) every table row points at a real
  file (ghost rows), and (3) each row's `status` and `stale_after` match the
  file's frontmatter. **(3) is the core** — with only (1) and (2), the table can
  exist and still lie (the file `stable`, the table `draft`), and a stale table
  is worse than no table. **Three mutations**: (1) deleting plan 3's row failed
  as a missing document; (2) reverting only the table's status to `draft` (the
  file staying `stable`) failed as a mismatch; (3) a typo in a link path failed
  as both a missing document and a ghost row. All three restored from a `cp`
  backup and re-run green. **The known limitation is stated in the docstring**:
  if a merged plan sits at `draft` in both the file and the table, they agree
  and it passes — that class is unknowable to a script and belongs to CLAUDE.md
  convention 8.
- `revised` — CLAUDE.md — added item 8 to "MUST — after finishing work" (when a
  plan's or spec's implementation merges, close its frontmatter: promote to
  `stable`, record reviews in `verified[]`, update the index table). Since the
  root cause of the plan 3 omission was that the `f58be95` fix was never
  promoted to a rule, this time the instance fix ships with the rule. Also
  states that CI catches only item 3 and not items 1 and 2 — with both the table
  and the file at `draft` they agree, and it passes.
- `revised` — `docs/index.md` — added "Writing a new plan document" to the
  conventions, so a new plan's frontmatter template carries the closing
  procedure (promote to `stable`, `verified[]`, the index table) as comments
  directly above `status: draft`. Reasoning: plan 3 was missed not because the
  procedure was unknown but because **the person who has to remember it is not
  the person who wrote the plan — it is a different session weeks later.** The
  procedure moves into the file rather than into memory. Stated in the same
  section that this only prevents "nobody knew it had to be closed", and that no
  script can know whether a branch merged.
- `revised` — `docs/index.md` — added **no inline comments** to the required
  frontmatter section. Measured: adding `stale_after: 2026-11-03  # comment`
  makes `^stale_after:\s*(\d{4}-\d{2}-\d{2})\s*$` fail to match, and
  `check_stale_after.py` then **skips that document silently while printing
  `OK: no expirations` and exiting 0** — expiry checking is switched off
  entirely while CI stays green. The same mutation is caught by
  `check_index_complete.py` (the table's `2026-11-03` disagreeing with the
  file's absent value). A check that parses one source fails this way; only one
  that compares two sources survives — the fourth instance of CLAUDE.md's
  "reporting success while doing nothing". The mutation was restored byte-exact
  from a `cp` backup.

## Documentation translated to English

- `revised` — `README.md`, `CLAUDE.md`, `docs/index.md`,
  `docs/superpowers/known-limitations.md`, `docs/frontend-assets.md` (new) —
  stage 1 of translating the documentation to English. (1) Rewrote the README as
  an open-source front page — what the product is, its status, requirements, a
  quick start (docker compose plus an environment variable table), an
  architecture diagram, a crate table, development commands, contribution
  conventions, and licensing. The old README's Tailwind and htmx build
  procedures did not belong on a front page and were split into
  `docs/frontend-assets.md` (a new Runbook with OKF frontmatter). (2) While
  translating `docs/index.md`, corrected the `cargo sqlx prepare --check` claim
  in its "what is still missing" table — CLAUDE.md had already recorded by
  measurement that it checks nothing because `query!` is unused, while index.md
  alone still carried the original claim (the residue of the contradiction
  graphify found). The guard table was also brought current at five entries.
  (3) Added a "Writing style for documents" section to CLAUDE.md — English only,
  no `§` symbol (write "Section 6.1"), language tags on code fences.
  (4) `check_index_complete.py` caught the new document immediately, requiring
  an eighth row in the index table — measuring that the guard works as intended.
- `revised` — the documentation format audit — measured across nine documents:
  frontmatter on 7 of 9 (README and CLAUDE.md deliberately excluded), stable
  anchors and table usage healthy. **`docs/log.md` was the only failure** — 86
  entries under a single heading, with the longest entry 2,894 characters on one
  line (median 586, nine over 2,000), so an agent must read all 118KB and a
  `grep -n` hit cannot be excerpted with its context. Fourteen code fences
  lacked language tags. The Markdown-plus-YAML-frontmatter combination itself is
  appropriate, so the format was not changed.
- `revised` — `docs/superpowers/specs/2026-07-30-agentops-design.md` —
  translated the spec to English, replaced `§` with `Section N` notation (84
  occurrences), and **removed the record of observing the reference product's
  live screens**. Deleted the `https://<instance>.aidevops.global.app.aws` entry
  from the frontmatter `sources[]`, along with Section 1's "having examined the
  live screens" sentence and Section 2's description of the screen structure
  (the three-pane layout, the per-page feature table, the URLs). **It did not
  stop at deletion** — Section 2.3 (seven conclusions revised from the original
  brief) rested on those observations, so removing only the sentences would
  leave the conclusions without support. Section 2 was rewritten as "Product
  model" (a section defining the shape of the product being built) and the seven
  conclusions restated as **design decisions** rather than observation reports.
  The AWS product's own URLs (`/dashboard`, `/prevention`, `/changes`) and page
  inventory were removed entirely, and Section 2.3 became Section 2.2. **ID
  integrity measured**: extracted and compared the `[TEST-N]` and `[INV-N]` sets
  before and after, confirming both are exactly the same 25.
- `revised` — `scripts/check_spec_test_ids.py` — widened `DEFERRED_RE` from
  recognizing only `(계획 N)` to accepting both that and `(plan N)`. With the
  spec now in English, the canonical deferral marker became `(plan 2)` while the
  regex was still looking only for Korean. **Because zero items were deferred at
  the moment of translation, this mismatch would have passed silently** — the
  next person to defer an item would have had that ID misread as in-scope,
  failing CI incorrectly. Mutation check: marking TEST-18 with `(plan 3)`
  dropped the checked scope from 25 to 24, confirming deferral works; the old
  regex matches only `계획` and would have missed that marker. The spec was
  restored byte-exact from a `cp` backup.
- `revised` — `docs/log.md` — translated this file to English and gave it
  navigable structure. It previously carried 86 entries under a single date
  heading, so an agent had to read all 118KB to find anything, and the longest
  entry was 2,894 characters on one physical line. Entries are now grouped under
  section headings by phase (spec and bundle, plans 1 through 3, post-merge, and
  this translation), and each entry is wrapped rather than left on one line.
  **This rewrote the bytes of past entries, which append-only forbids** — it was
  done on the user's explicit instruction to put every document in English, and
  is recorded here as its own entry rather than silently. All technical content
  (commit hashes, measurements, test counts, mutation results) was carried
  across unchanged; nothing was summarized away. The append-only rule stands
  unchanged for everything after this entry.
- `revised` — `docs/superpowers/plans/{2026-07-30-foundation,2026-08-03-web,2026-07-30-agent}.md` — translated all three implementation plans' prose to English, completing the bundle. **Code fences were frozen byte-for-byte.** The plans' code blocks were transcribed into the repository, so rewriting them — comments included — would desynchronize each plan from the code it documents, and that correspondence is what makes these documents useful as a record. A splitter (`prose_split.py`) extracts prose, leaving fences as opaque segments, and reassembles afterwards; it was round-tripped **without translation first** to prove byte-exactness before being trusted. **Integrity was measured, not assumed**: a baseline of eight properties was captured before editing and compared after — spec IDs, deferral markers, task numbers, `fn` names, test function names, fence count, checkboxes, and referenced file paths. All three files matched on all eight, and their fences compare byte-identical (75, 71, and 65 respectively). **The check caught one real loss**: in the web plan, nine literal `(계획 3)` marker strings had been paraphrased into "plan-3 deferral marker", but those are instructions telling an implementer exactly which string to delete from the spec, so paraphrasing makes the step ambiguous. Restored as literal `(plan 3)`. That is the second time in this pass a mechanical check found something reading would not have — the first was the deferral-marker regex in `check_spec_test_ids.py`. Prose is now 0 Korean lines and 0 `§` in all three; 245, 212, and 268 Korean lines remain inside code fences, mirroring the repository's own comments
- `verified` — documentation translation, complete — all ten documents are now English with `§` replaced by `Section N` notation. Three occurrences remain by design and are **quotations, not usage**: CLAUDE.md's rule forbidding `§` must name the symbol, and `docs/log.md` quotes both that rule and the `(계획 N)` regex literal `check_spec_test_ids.py` still matches for backward compatibility. Changing any of the three would make the record false. **Known scope boundary**: source code is unchanged. 2,827 Korean comment lines remain in `crates/**/*.rs`, plus 99 in `scripts/*.py`, 23 in `.github/workflows/`, and 12 in `migrations/*.sql`. Code comments are not documents, and translating those inside the plans' frozen fences would have broken plan-to-code correspondence — the two decisions are the same decision. Whether to translate the source is a separate call
- `revised` — `crates/**`, `migrations/0001_initial.sql`, `.github/workflows/ci.yml`, `scripts/*.py` — translated every comment in the source tree to English, closing the scope boundary the entry above declared. 2,961 Korean comment lines became English across four crates, 19 test binaries, the migration, the CI workflow, and all three drift guards. **Code was never touched**, and that claim was mechanically verified rather than asserted: `code_identical.py` strips comments and string literals per language (Rust, Python, SQL, YAML, HTML) and compares the remainder, and it was mutation-tested first — an identical file passes, a comment-only edit passes, and changing `STEP_PAYLOAD_VERSION: i32 = 1` to `2` fails. Every "code differs" report it raised was inspected by hand and every one was a string literal that no assertion reads by content: assert messages, `bad(...)` error text (tests match `MalformedEvent(_)` by wildcard), `SpawnError::GateClosed`'s `#[error]` text (tests match the variant), the `MaxTokens` `detail` string (the Task 4 review had deliberately relaxed that assertion), and one artifact fixture body (verified by grep that nothing compares it). 263 workspace tests passed after every commit. **Five Korean characters remain in `crates/agentops-agent/tests/mcp.rs` by design** — `'한'` is the 3-byte UTF-8 fixture the character-boundary truncation test is built on, and replacing it with an ASCII character would delete the test's subject
- `revised` — `docs/superpowers/plans/{2026-07-30-foundation,2026-07-30-agent,2026-08-03-web}.md` — translated the plans' code fences, which the previous pass had frozen. Freezing them was correct **while the source was still Korean**: the fences and the source said the same thing, and rewriting one alone would have desynchronized them. Translating the source removed that reason and created its opposite — leaving the fences Korean would have been the desynchronization. So the translation was **derived from the source diff, not written fresh**: pairing each source file's pre- and post-translation versions line by line yielded a 2,960-entry Korean-to-English map, which covered 1,083 of the plans' 1,291 Korean lines verbatim. The remaining 208 (file-tree comments, quoted spec and CLAUDE.md prose, code that never reached the repository) were translated by hand. The same eight-property integrity baseline as the prose pass — spec IDs, deferral markers, task numbers, `fn` names, test function names, fence count, checkboxes, referenced file paths — was captured before and compared after: **all three files matched on all eight**. One Korean string remains, in the foundation plan's `check_spec_test_ids.py` fence: the `(?:계획|plan)` regex literal the script still matches for backward compatibility. It is a quotation of code, and changing it would make the fence disagree with the script. **A known cosmetic divergence was introduced and is not being chased**: English assert messages are longer than the Korean ones they replaced, so `cargo fmt` reflowed 16 source files (142 lines added, 36 removed) into multi-line call shapes the plans' fences still show on one line. The text is identical; only the wrapping differs. Re-transcribing the fences to match rustfmt's output would make every future formatting change a documentation change, which is a worse trade than the divergence
- `verified` — repository translation, complete — every document and every source comment is now English. What remains is exactly three quotations and one test fixture, each of which would become false or lose its subject if translated: CLAUDE.md's rule naming the `§` symbol, `docs/log.md`'s quotation of that rule and of the `(계획 N)` regex, the foundation plan's fence of that same regex, and `mcp.rs`'s `'한'` multi-byte fixture. Drift guards green (25 in-scope spec IDs present, 8 documents unexpired, 8 bundle rows matching), `cargo fmt --check` and `cargo clippy -D warnings` clean, 263 tests passing at `--test-threads=4`
- `revised` — `docs/superpowers/known-limitations.md` — added **L17 (M2 — `append_step` still succeeds after termination)**, which was open but recorded nowhere. Plan 3's Task 7 Step 4, `docs/log.md`, and the pinning test itself each state that the decision was "handed to the final review's triage"; all three statements are accurate, and none of them was ever received. Its sibling carry-overs make the gap legible: F1 arrived as L3 plus section 4, and G1/G2/G4/I5/M1 are absent because plan 3 closed them — M2 was absent while still open. It was found by tracing an `AMBIGUOUS` edge (`M2 --conceptually_related_to--> Store::steps_after`, confidence 0.3) surfaced by graphify's suggested questions, not by any procedure. The entry records what was measured rather than only the claim: `steps_after` has no status filter (`steps.rs:176`), it has exactly two readers, `close_if_investigation_is_terminal` runs **after** replay rather than before (`stream.rs:226`) so a new connection does replay the row and then closes, `investigation_detail` derives `after` from the rendered list rather than `max_step_seq` (`pages.rs:108`) which is what keeps the two views from disagreeing, and `stale_running_ids` filters `status = 'running'` (`investigations.rs:171`) so the advancing `updated_at` is never read. A fix belongs in `append_step`, not in `steps_after` — filtering on read would hide legitimate pre-termination rows and leave the cause untouched
- `revised` — `docs/superpowers/known-limitations.md` — recorded the M2 handoff loss as the **fourth instance of "reporting success while doing nothing"**, and marked it as a different kind from the first three. Those are commands that exit 0; this is a procedural handoff where every report was true and the destination did not exist. It is the same shape as the plan-3 `draft` frontmatter incident in CLAUDE.md — an item created after the audit had already gone by — one layer up. **No script can catch it**: `check_index_complete.py` checks that the table and the files agree, and they did; nothing can know a document is missing a row it never had. Promoted to a rule rather than left as an instance, per CLAUDE.md's own warning that a fix cannot see multiplicity: **when a task defers a decision, name the document the decision lands in, and check that it arrived**
- `added` — `docs/superpowers/known-limitations.md` section 8 — recorded that `graphify-out/graph.json` carries **two confidence scales, not one**. Measured on the 2026-08-04 build (2,121 nodes, 4,355 edges): 4,174 `EXTRACTED` at 1.0, 180 `INFERRED`, 1 `AMBIGUOUS`; of the 180 `INFERRED`, **102 sit outside the five-value rubric** — 74 `calls` at 0.8 and 28 `indirect_call` at 0.5, 27 of the latter from the vendored `htmx.min.js`. The split is exact: every off-rubric edge is `calls` or `indirect_call` (the AST extractor's relations, using two fixed constants) and every rubric-conforming score belongs to a semantic relation (the LLM extractor's, which reads the prompt that forbids 0.5). This is a property of graphify, not of this repository's code, but it is recorded here because CLAUDE.md's first convention is to ask the graph first, and `confidence_score >= 0.85` — the obvious way to ask for only its confident answers — **excludes every AST-derived `calls` edge**, which is most of what makes the graph a call graph. The section carries a runnable audit command, verified to reproduce these counts. **How it was found is the entry's point**: the last extraction agent reported that every score obeys the tiering, and that report was true of its own chunk; the verification that followed checked the same chunk; both were honest and neither enumerated the 102 AST edges. Promoted to a rule — **a verification is only as broad as what it enumerated; state the denominator, or the pass means nothing**
- `added` — `docs/superpowers/known-limitations.md` section 8 — also recorded that **`AMBIGUOUS` edges are not stable across extractions**. A second `AMBIGUOUS` edge existed in the morning build, linking L8 to the schema's `CHECK` constraints; re-extracting `known-limitations.md` that afternoon replaced it with `L8 --conceptually_related_to--> Chart window derived from a single constant` at `INFERRED` 0.85. The new judgment is better — L8 is about clock skew and has nothing to do with `CHECK` constraints — but nothing recorded that the old edge ever existed. The consequence for procedure: `AMBIGUOUS` is worth tracing, but it is a snapshot of one run rather than durable state, so anything found by tracing one must be written into a document (as L17 was) or it is lost on the next `--update`
- `revised` — `CLAUDE.md` convention 1 (ask the graph first) — added the two caveats that a reader hits *at the moment they query*, rather than leaving them only in `known-limitations.md` section 8 where nobody would look first. **Do not filter by `confidence_score`**: two producers on two scales, so `>= 0.85` drops every AST-derived `calls` edge; compare `confidence` freely and `confidence_score` only within one producer. **An `AMBIGUOUS` edge does not survive re-extraction**: it is worth tracing (L17 came from one) but is a snapshot of a single run, so what the trace finds must be written into a document. Both link onward to section 8 for the measurements and the audit command. This is the arrival-check rule from section 6 applied to itself — section 8 was written, but the place the reader actually stands is convention 1, so the pointer belongs there too
- `revised` — `.gitignore`, `Cargo.toml`, `docker-compose.yml` — translated 11 Korean comment lines that the translation pass missed entirely. **The earlier entry claiming "every document and every source comment is now English" was wrong**, and this is the correction rather than an edit to it. The cause is exactly the rule written into section 8 of `known-limitations.md` earlier today: the sweep enumerated `crates/ scripts/ migrations/ .github/` and reported zero, and that report was true **of those four directories**. Nothing in the repository root was ever in the denominator. The two `docker-compose.yml` lines were even seen during this session — their graph nodes were deliberately dropped for being Korean — and the file itself still went untranslated, because the observation went to the graph rather than back to the sweep. Correct audit command, which would have caught all three files: `git ls-files -z | xargs -0 grep -ln '[가-힣]'`. It enumerates what git tracks rather than a hand-listed set of directories, so the denominator cannot silently exclude anything. Four files still contain Korean, all deliberate and all previously recorded: `mcp.rs`'s `'한'` multi-byte fixture, and three quotations of the `(계획 N)` regex literal
- `revised` — `README.md` — strengthened the setup procedure, and **corrected a claim that made the documented quick start fail on a fresh database**. The old step 3 read "Run. Migrations are applied at startup." `crates/agentops-server/src/main.rs` does no such thing: it builds the pool and calls `recover_on_boot`, which queries `investigations`. Measured against a genuinely empty database — `Error: store backend error: error returned from database: relation "investigations" does not exist`. Both replacement routes were then measured end to end: applying `migrations/0001_initial.sql` through `docker compose exec psql`, after which the server started, `/api/health` returned 200 and `/` returned 303. The probe database was created and dropped inside the existing container; the development database was not touched. Also added to the README: the Postgres healthcheck wait, a verification `curl`, `RUST_LOG`, a pointer to `.env.example`, Python 3.12 and Docker in Requirements, a note that the test suite needs a reachable `DATABASE_URL` but neither a migrated database nor an API key, a note that `graphify-out/` is absent on a fresh clone with the regeneration commands, and a failure-symptom table covering all five first-run errors observed in this repository — missing schema, unset variables, port 55433 held by a worktree leftover, a too-old toolchain, and the `--test-threads=4` signature
- `added` — `docs/superpowers/known-limitations.md` — **L18: the server does not apply migrations at startup**. Recorded rather than fixed, because whether it *should* migrate itself is an open design question — automatic migration against a shared database is a footgun, and the better fix may be to fail with a message naming the remedy instead of a raw relation error. **No test could have caught this**, and that is the entry's point: `#[sqlx::test]` creates and migrates a throwaway database per test, so the entire 263-test suite never meets a hand-made database. The only path that does is a human following the README, which is exactly the path with no automated coverage. This is a third form of the denominator problem recorded today — not a check with a narrow scope, but a test harness whose convenience removes the very condition the defect lives in
- `revised` — the repository's git history — **squashed 169 commits into one on 2026-08-05, on explicit instruction, with no copy kept.** The alternative offered was preserving the old commits under a tag so the evidence citations would still resolve; full deletion was chosen instead. The cost, stated before the operation and measured: **44 commit references across five documents stop resolving** — 29 in this log, 7 in `CLAUDE.md`, and 8 across the three plans. Each of those five documents now carries a note saying so at the top. The hashes were kept rather than stripped: each still identifies *which* change carried a piece of evidence, and deleting them would remove that distinction on top of removing the evidence. **This does not remove anything sensitive** — history was searched for GitHub token patterns before the squash and had none; the leaked PATs were never in a committed diff. Note for anyone reading this later: GitHub retains unreachable objects for a period after a force-push, so the old commits may remain fetchable by full hash for some time even though nothing points at them

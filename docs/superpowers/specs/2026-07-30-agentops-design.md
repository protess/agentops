---
type: Design Spec
title: agentops v0.1 design
description: Design of the v0.1 vertical slice of agentops, an agent-first SRE assistant
status: stable
tags: [architecture, v0.1, rust, axum, postgres, mcp, llm]
generated:
  by: claude-opus-5
  at: 2026-07-30
verified:
  - by: fable-review
    at: 2026-07-30
    kind: machine
    note: API facts, architectural invariants, schema, scope (Section 19.1)
  - by: codex-review
    at: 2026-07-30
    kind: machine
    note: Rust feasibility, concurrency races, protocol round trips (Section 19.2)
sources:
  - resource: "AWS DevOps Clone — open-source project brief"
    author: user
    last_modified: 2026-07-30
    note: The original brief. Seven of its conclusions are revised in Section 2.2.
stale_after: 2026-10-30
supersedes: []
---

# agentops — design document (v0.1)

Awaiting an implementation plan. Revision history is in `docs/log.md`;
verification history is in Section 19. Licensed MIT.

> This document carries [OKF v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
> frontmatter. Once `stale_after` has passed, it needs revalidation.

## 1. Purpose

**An open-source, self-hosted alternative to hosted DevOps investigation
agents.** The goal is an agent-based SRE assistant that is tied to neither a
cloud provider nor an LLM provider.

The central design decision is that this is an **agent-first product, not a
CloudWatch-style monitoring dashboard**. A user launches an investigation in
natural language, the agent calls tools to find the root cause, and the result
is left behind as an artifact. Anomaly detection is an input that can trigger
an investigation — it is not the center of the product.

This document revises the original brief (`AWS DevOps Clone — open-source
project brief`) according to that decision.

## 2. Product model

This section defines the shape of the product. Section 2.1 fixes the
interaction model; Section 2.2 records where this design departs from the
original brief and why.

### 2.1 Layout — three panes

1. **Left: a navigation rail** — the pages the product exposes, plus chat
   history
2. **Center: a resident chat panel** — the greeting and the suggestion chips
   change with the active page, because what the agent can usefully do
   depends on where the user is
3. **Right: the workspace** — the active page's content

v0.1 simplifies the right pane to a single active page. Multi-tab workspaces
are out of scope (Section 3.2).

### 2.2 Conclusions revised from the original brief

| Original brief | Revision |
|---|---|
| "MCP extension hook (low priority)" | **MCP is the core.** For an agent to investigate anything it must call metrics, logs, topology, and repositories as tools. Without a tool layer there is no product. |
| "ML-based anomaly detection" as MVP item 1 | Anomaly detection is a **trigger** that starts an investigation (`Triggered By: Alarm`). It is peripheral and out of scope for v0.1. |
| "Monitoring dashboard: CloudWatch-like" | The dashboard is the investigation list, the frequency chart, and topology. It is not a wall of metric graphs. |
| "RDS/EC2 resource support" | AWS-specific concepts are removed under the provider-agnostic principle. Generalized to whatever resource inventory a connector provides (out of scope for v0.1). |
| "CI/CD pipeline" | Change review is **release-risk review**, not a pipeline runner. A separate follow-up spec. |
| SQLite (MVP) → Postgres (prod) | **Postgres only.** This removes a double implementation and SQL-dialect branching. |
| No Knowledge concept | **Phase-scoped Instructions, plus Skills and Memories**, is the core differentiator. v0.1 includes Instructions. |

## 3. v0.1 scope

### 3.1 Included

A single **vertical slice** in which the agent actually performs an
investigation:

1. **Chat** — a resident panel, SSE token streaming, per-page context
2. **Investigation** — launched from free text, a long-running background job,
   status (`queued`/`running`/`completed`/`failed`), three sequential phases
   (Section 5.3), progress streaming, replay on reconnect
3. **MCP tool layer** — an `rmcp`-based MCP client. Tools the agent can call
   are discovered from and executed on MCP servers.
4. **Knowledge / Instructions** — CRUD for phase-scoped
   (`all`/`chat`/`triage`/`rca`/`mitigation`) instructions, plus per-phase
   injection
5. **Artifacts** — investigation output stored and read back as markdown
   documents (versioning is v0.2)
6. **The `LlmProvider` trait plus one Anthropic API implementation**

### 3.2 Explicit non-goals (each a follow-up spec)

- **Amazon Bedrock / Claude Platform on AWS provider implementations** — the
  trait ships in v0.1, the implementations in v0.2. Reasoning in Section 8.2.

- Automatically generated topology graphs
- Improvements (weekly preventive evaluation)
- Changes (release-risk review, test generation)
- Custom agent creation
- The Skills and Memories tabs of Knowledge
- Artifact versioning
- **Multi-tab workspaces** — v0.1 has a single active page
- Anomaly detection and alarms (the path that launches an investigation with
  `Triggered By: Alarm`)
- Metric-collection connectors and time-series storage
- Authentication, authorization, multi-tenancy
- Usage metering and billing
- Kubernetes manifests

### 3.3 Honesty principle

The README and the UI must state the following accurately:

- v0.1 is an **LLM-based investigation agent**. It contains no statistical or
  ML anomaly detection. Do not describe it as "ML-powered".
- v0.1 has **no authentication.** The default bind address is `127.0.0.1`, and
  the README warns at the top that exposing it to the public internet is
  prohibited.

## 4. Architecture

### 4.1 Cargo workspace

Boundaries are split into crates so the compiler enforces them. A single crate
with modules cannot prevent the core from accidentally depending on `sqlx` or
`reqwest`.

| Crate | Responsibility | Depends on |
|---|---|---|
| `agentops-core` | Domain types (`Investigation`, `AgentStep`, `Artifact`, `Instruction`), protocol types (`LlmRequest`, `LlmEvent`, `ContentBlock`), trait definitions (`LlmProvider`, `ToolRegistry`, `Store`, `JobManager`), error types. No I/O. | `serde`, `serde_json`, `uuid`, `time`, `async-trait`, `futures-core` |
| `agentops-llm` | Anthropic Messages API client (an `LlmProvider` implementation). AWS adapters arrive in v0.2. | core, `reqwest` |
| `agentops-tools` | `ToolRegistry` — wraps the MCP client, discovers and executes tools | core, `rmcp` |
| `agentops-store` | The `Store` trait plus its Postgres implementation, and migrations | core, `sqlx` |
| `agentops-agent` | The agent loop — assembling instructions, calling the LLM, executing tools, persisting steps | core, llm, tools, store |
| `agentops-server` | Axum routes, askama templates, SSE, the `JobManager` implementation (tokio tasks), configuration, wiring | everything |

Because `agentops-core` knows nothing about I/O, domain logic is unit-testable
immediately.

### 4.2 UI layout

The three-pane layout is kept; v0.1 simplifies the right pane to a single
page.

```
┌────────────┬──────────────────┬──────────────────────────────┐
│ Navigation │  Chat panel      │  Content (single active page)│
│ rail       │  (resident, SSE) │  Incidents / Knowledge /     │
│            │                  │  Artifacts                   │
│ Incidents  │  Greeting +      │                              │
│ Knowledge  │  suggestion chips│                              │
│ Artifacts  │  (per page)      │                              │
│            │  ┌────────────┐  │                              │
│ Settings   │  │ Input      │  │                              │
└────────────┴──┴────────────┴──┴──────────────────────────────┘
```

- HTMX plus Tailwind, no JavaScript framework
- Chat and investigation progress stream over **SSE** (`htmx-ext-sse`)
- A single dark theme. A light theme is a follow-up.

## 5. Core domain model and traits

### 5.1 Investigation lifecycle

```rust
// agentops-core
pub enum InvestigationStatus { Queued, Running, Completed, Failed }

pub enum TriggeredBy { User, Alarm { source: String } }

pub struct Investigation {
    pub id: Uuid,
    pub title: String,          // summary of user input, or auto-generated
    pub prompt: String,         // the original free text
    pub status: InvestigationStatus,
    pub triggered_by: TriggeredBy,
    /// Three distinct timestamps — matching the CHECK constraints in Section 7.
    pub queued_at: OffsetDateTime,
    pub started_at: Option<OffsetDateTime>,   // set on entering Running
    pub finished_at: Option<OffsetDateTime>,  // set on entering Completed|Failed
    pub updated_at: OffsetDateTime,
}

/// One event from the agent loop. `seq` increases monotonically within an
/// investigation.
pub struct AgentStep {
    pub investigation_id: Uuid,
    pub seq: i64,
    pub phase: Phase,
    pub kind: StepKind,
    pub created_at: OffsetDateTime,
}

pub enum StepKind {
    Thinking { summary: String },
    Text { text: String },
    /// `tool_use_id` is the ID Anthropic issued. A single turn can carry
    /// several parallel tool calls, and without this there is no way to pair
    /// a call with its result.
    ToolCall { tool_use_id: String, tool: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, tool: String, output: String, is_error: bool },
    ArtifactWritten { artifact_id: Uuid },
    /// Carries a structured termination reason such as a refusal or a context
    /// overflow. With only a `message` field, values like
    /// `stop_details.category` end up crammed into a string.
    Terminated { reason: TerminalReason, detail: Option<String> },
    Error { message: String },
}

pub enum TerminalReason {
    Refusal { category: Option<String> },
    ContextWindowExceeded,
    MaxTokens,
    TurnLimitExceeded,
    WallClockExceeded,
    ToolTimeout { tool: String },
    ShutdownRequested,
    TaskPanicked,
    /// A dependency this phase needs was unavailable — something external and
    /// recoverable, such as the policy store or an MCP server. Not a panic.
    DependencyUnavailable { what: String },
    UnknownStopReason(String),
}

/// The same axis as the Knowledge/Instructions scope.
pub enum Phase { All, Chat, Triage, Rca, Mitigation }
```

### 5.2 Traits

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn model_id(&self) -> &str;
    /// Streaming only. Investigations run for a long time, so there is no
    /// non-streaming path.
    async fn stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>;
}

#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// The tool definitions that will be serialized into the LLM request's
    /// `tools[]`.
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError>;
    async fn call(&self, name: &str, input: serde_json::Value)
        -> Result<ToolOutput, ToolError>;
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError>;
    async fn append_step(&self, step: &AgentStep) -> Result<(), StoreError>;
    async fn steps_after(&self, id: Uuid, after_seq: i64)
        -> Result<Vec<AgentStep>, StoreError>;
    async fn set_status(&self, id: Uuid, status: InvestigationStatus)
        -> Result<(), StoreError>;
    /// Returns rows ordered by position, then title. Without that ordering the
    /// prompt cache breaks (Section 8.3).
    async fn instructions_for(&self, phases: &[Phase])
        -> Result<Vec<Instruction>, StoreError>;
    // ... listing, search, artifacts
}

#[async_trait]
pub trait JobManager: Send + Sync {
    async fn enqueue(&self, investigation_id: Uuid) -> Result<(), JobError>;
    /// Subscription for live fan-out. Must be called before the replay query
    /// (Section 6.1, invariant 2).
    fn subscribe(&self, investigation_id: Uuid) -> broadcast::Receiver<AgentStep>;
}
```

**Why `#[async_trait]`, stated explicitly.** Native `async fn` in traits is not
dyn-compatible, and this design holds these traits as `Arc<dyn Trait>`, so
`#[async_trait]` is the correct choice. `LlmProvider::stream` keeps object
safety because its return type is erased to `Pin<Box<dyn Stream + Send>>`.

Putting `Send + Sync` on the trait is not sufficient by itself. `LlmRequest`,
`LlmEvent`, the error types, and every value held across an `.await` boundary
must also be `Send + 'static`. Protocol types are therefore defined as **owned
types with no borrows** and live in `agentops-core`.

### 5.3 Phase progression rules

If `Phase` is only a type with no transition rules, phase-scoped instructions
become dead code. v0.1 fixes this as a **fixed sequential sub-loop**:

```
run_investigation:
  carried = []                              # summaries of previous phases
  for phase in [Triage, Rca, Mitigation]:
      instructions = Store::instructions_for([All, phase])   # this phase only
      summary = run_phase_loop(phase, instructions, carried) # separate LLM conversation
      carried.push((phase, summary))
  artifact = render(carried)
```

- **Each phase is an independent LLM conversation.** `messages[]` starts fresh
  for each phase, and only a summary carries forward from the previous one.
  This bounds context growth per phase (connects to Section 18.3).
- A phase ends when `stop_reason == end_turn` and no tool calls are pending.
- **All three phases always run.** A branch like "skip mitigation when there is
  nothing to act on" requires structured output or a separate adjudication
  mechanism, so it is not in v0.1. Instead the mitigation phase's instructions
  say "if there is nothing to do, state that."
- The alternative — letting the LLM transition itself through a `set_phase`
  tool — was not adopted. Deterministic execution and testability took
  priority. Revisited in Section 18.

`AgentStep.phase` records which sub-loop a step came from, and drives the phase
separation in the UI timeline.

### 5.4 Loop limits — all of them required

Limiting only `PauseTurn` resumption and leaving the rest open lets the agent
call tools forever. **Every limit below is a configuration value, and
exceeding one terminates that phase with `Terminated`.**

| Limit | Default | `TerminalReason` when exceeded |
|---|---|---|
| Max turns per phase (LLM round trips) | 40 | `TurnLimitExceeded` |
| Max tool calls per phase | 120 | `TurnLimitExceeded` |
| Parallel tool calls in one turn | 16 | the excess is answered with `is_error` |
| Whole-investigation wall clock | 30 minutes | `WallClockExceeded` |
| Individual tool call timeout | 60 seconds | `ToolTimeout { tool }` |
| LLM stream idle timeout | 120 seconds | `WallClockExceeded` |
| `PauseTurn` resumptions | 5 | `TurnLimitExceeded` |

Exceeding a limit is **a normal termination path, not a failure.** A phase
summary is built from whatever was collected and execution moves to the next
phase. Only exceeding the whole-investigation wall clock marks the
investigation `failed`.

All of these counters, including the `PauseTurn` resumption counter, are
**in-memory**. v0.1 does not support resuming an investigation
(Section 6.1.2), so the counters do not need to survive a process restart.

## 6. Data flow — investigations

```
POST /api/investigations  (free text)
   │
   ├─ Store::create_investigation(status=Queued)
   ├─ JobManager: tokio::spawn(run_investigation(id))
   └─ 303 → /investigations/{id}     ← returns at once; the request does not
                                        wait for the job

run_investigation(id)  [an independent tokio task — the sole writer for this
                        investigation]
   │ status=Running
   │
   │ for phase in [Triage, Rca, Mitigation]:            ← Section 5.3
   │   1. Store::instructions_for([All, phase]) → assemble the system prompt
   │   2. ToolRegistry::list() → tools[]
   │   3. sub-loop (messages[] lives only within this phase):
   │        LlmProvider::stream(req)
   │          ├─ Thinking/Text events → append_step + broadcast
   │          └─ ToolCall events → ToolRegistry::call → append_step(ToolResult)
   │                             → add tool_result to messages and call again
   │        stop_reason branching is in Section 8.4
   │   4. accumulate the phase summary into `carried`
   │ save the summary as an Artifact
   │ status=Completed | Failed
   │
   └─ every step is appended to Postgres as (investigation_id, seq), then
      broadcast

GET /api/investigations/{id}/stream?after={seq}   [SSE, observer]
   └─ follows the ordering in Section 6.1, invariant 2
```

### 6.1 Four invariants

Honoring these from the start is what avoids a rewrite later.

**1. An investigation is not tied to an HTTP request.** `[INV-1]`
The handler spawns a task and returns a `job_id` immediately. The SSE endpoint
is an **observer** only; the job continues when the connection drops. The
existence of a `Completed` status and of scheduled runs both demand this.

**2. Replay uses the `after` URL parameter, never `Last-Event-ID`.** `[INV-2]`

`Last-Event-ID` is sent **only when the browser automatically reconnects the
same `EventSource`.** A page refresh creates a new `EventSource`, so the header
is absent; the htmx SSE extension's retry path also recreates the `EventSource`
manually, so the header is lost there too. In other words, the header is
missing in exactly the two scenarios where replay is needed.

Therefore:

```
GET /investigations/{id}
  → the server renders every existing step and embeds the last seq in the
    template
  → <div hx-ext="sse" sse-connect="/api/.../stream?after={last_seq}">

GET /api/investigations/{id}/stream?after={seq}
  1. subscribe to the broadcast channel first        ← the order matters
  2. replay with Store::steps_after(id, after) and update emitted_seq
  3. drain the already-subscribed channel, discarding seq <= emitted_seq
     (deduplication)
  4. then deliver live
  5. RecvError::Lagged → resync with Store::steps_after(id, emitted_seq) and
     continue
  6. if a Last-Event-ID header does arrive, prefer it over `after`
     (best-effort enhancement)
```

**Subscribe before replaying.** In the opposite order, any step written between
the query and the subscription is lost — which happens precisely when the
investigation is most active.

**3. Broadcast lag (`Lagged`) must be handled.** `[INV-3]`
A slow SSE client receives `RecvError::Lagged` from `tokio::sync::broadcast`.
Ignoring it makes steps vanish silently from the UI. Recover by resyncing from
the database, as in step 5 above. Channel capacity is 256, and every `Lagged`
occurrence is recorded in metrics and logs.

**4. Zombie investigations are blocked in two layers.** `[INV-4]`
- **At boot**: investigations still marked `Running` are marked `Failed` with
  the reason recorded as a step. `Queued` ones are rescheduled.
- **While running**: the `JoinHandle` is watched and the investigation is
  marked `Failed` if the task panics. In addition, a watchdog periodically
  fails any `Running` investigation whose `updated_at` has not advanced past a
  threshold (15 minutes by default).

If the process stays alive while only the task panics or stalls, boot cleanup
never fires. Without this second layer, an investigation stays `Running`
forever.

Tasks are managed in a `tokio::task::JoinSet`, and cancellation propagates
through a shared `CancellationToken`. Never create a detached task — graceful
shutdown then has nothing to wait on.

**Terminal status transitions must be conditional.** If shutdown marks an
investigation `Failed` while its task is still alive, that task will later
write `Completed` over it. Every terminal transition takes the form
`UPDATE ... WHERE id = $1 AND status = 'running'`, and when zero rows are
affected the caller backs off, treating it as another party having already
terminated it.

The shutdown order is fixed:

```
1. stop accepting new investigations (close the JobManager intake)
2. fire the CancellationToken
3. send a terminal event on SSE connections and close them
4. wait on the JoinSet until the deadline (30 seconds by default)
5. abort anything past the deadline, then join
6. for investigations still running, conditionally transition Running→Failed
   and append a Terminated{ShutdownRequested} step
```

**Terminal writes must be a single transaction.** If saving the artifact,
appending the `ArtifactWritten` step, and transitioning the status commit
separately, a partial success can survive. `Store` provides
`complete_investigation(id, artifact, final_step)` and
`fail_investigation(id, reason)` to do this in one transaction.

### 6.1.1 `seq` allocation — the database is the sole authority

**The application never computes `seq`.** Postgres allocates it atomically for
both tables.

**`agent_steps`** — the `investigations.next_step_seq` column holds the next
value. Allocation and the `updated_at` refresh happen in one statement:

```sql
UPDATE investigations
   SET next_step_seq = next_step_seq + 1, updated_at = now()
 WHERE id = $1
RETURNING next_step_seq - 1
```

The row lock serializes appends per investigation. Because the `updated_at`
refresh is in the same statement, the value the INV-4 watchdog reads is
necessarily refreshed on every step append — there is no separate UPDATE to
forget.

**`chat_messages`** — the session row is taken with `SELECT ... FOR UPDATE` and
`MAX(seq)+1` is computed. This serializes per session, and twenty concurrent
requests against one row form a wait queue rather than a deadlock (there is no
cycle). Each waiter takes a fresh snapshot after acquiring the lock, so there
is no lost update either.

> The first version chose an mpsc actor per session here and rejected
> `FOR UPDATE` on the grounds that it "holds a transaction open across
> streaming". **That reasoning was wrong** — Section 6.2 already decided that a
> chat message is written once, on completion, so no transaction wraps the
> streaming. An actor adds a layer and buys nothing.

#### Why not an in-memory counter

The first version argued that "the `run_investigation` task is the sole writer,
so an in-memory counter suffices." **That is wrong.** There are in fact two
paths that compute `seq` for one investigation:

1. the live task's counter
2. **the watchdog** (INV-4), when it terminates an investigation it judges
   stalled. While the watchdog writes the terminal step, that task is still
   alive and still writing steps — which is the entire reason the watchdog
   exists

When both paths reach the same `seq`, `ON CONFLICT DO NOTHING` **silently
swallows the terminal step while the status transition commits.** The result is
a `completed` investigation with neither an `ArtifactWritten` nor a
`Terminated` step. A mechanism added for idempotency turns into a mechanism
that hides errors.

#### Terminal step insertion must be strict

Ordinary appends and terminal steps use **different conflict policies**:

| | On conflict |
|---|---|
| Ordinary step append | Cannot conflict, because the database allocates `seq` |
| Terminal step (`Terminated`, `ArtifactWritten`) | No `ON CONFLICT`. A conflict rolls the whole transaction back and returns `Conflict`. |

#### The price we accept

Database allocation **gives up idempotency for exact retries.** Retrying an
append whose commit succeeded but whose response was lost allocates a new `seq`
and duplicates the step.

This trade is deliberate. A duplicated step in the UI timeline is a cosmetic
problem; a lost terminal step leaving a `completed` investigation with no
artifact record is a correctness problem. The agent loop also never retries an
append automatically — on failure it terminates the phase (Section 8.4).

### 6.1.2 The protocol transcript and the UI projection are different things

Blurring this distinction makes API requests silently invalid. **The two
representations are kept clearly separate:**

| | Protocol transcript | `agent_steps` |
|---|---|---|
| What | The original content blocks of `messages[]`, to be sent back to the API | The UI timeline and audit log |
| Contains | Assistant block ordering, `tool_use_id`, the `signature` of thinking blocks, correctly paired `tool_result`s | Summary text, `tool_use_id`, termination reasons |
| Where | **Owned in memory by the sub-loop** | Postgres |
| Source of truth | **This is the truth** for API round trips | Truth only for what the UI displays |

- Thinking blocks must be sent back **unmodified, including `signature`.** The
  stream's `signature_delta` events must be accumulated. `agent_steps` holds
  only summaries, so `messages[]` cannot be reconstructed from it.
- Every `tool_use_id` must have **exactly one** corresponding `tool_result`. If
  even one is missing, the API rejects the follow-up request.
- The price of this decision: **resuming an investigation is impossible in
  v0.1.** Resumption would require persisting the original blocks. This is why
  Section 6.1's invariant 4 chooses to mark `Failed` rather than resume. When a
  follow-up spec takes on resumption, it will need a migration adding a column
  for the original blocks.

### 6.1.3 A persisted step is not a stream delta

**`append_step` is not called per delta.** Inserting on every `text_delta` and
`thinking_delta` would make a single investigation tens of thousands of rows,
contradicting the scale estimate in Section 7 and exploding write load.

- **Persisted steps are recorded only at semantic boundaries**: a completed
  text block, a completed tool call, a tool result, a termination reason.
- **Deltas exist only for the live SSE stream.** They flow to the browser token
  by token and are never written to the database.
- The replay unit on reconnect is therefore a **completed step**. Partial text
  from a block that was in flight is not re-streamed from the beginning; it
  arrives as a step when that block completes. Unlike chat, investigation
  blocks are short, so this is an acceptable trade-off.
- SSE event types split in two: `delta` (transient, no `id`) and `step`
  (persisted, `id = seq`). `after` replay applies only to `step`.

### 6.2 Chat

Chat uses the same LLM pipeline but injects `Phase::Chat` instructions and
creates no investigation record. Chat sessions are stored in `chat_sessions`
and `chat_messages`.

**Reconnect semantics differ from investigations.** Chat is not persisted token
by token; one row is written when a message completes. Therefore:

- Refreshing mid-stream **loses the partial response in flight.** Token-level
  replay is not possible.
- On reconnect the server **re-renders completed messages only**, showing a
  "generating response" state if one is in flight. When it finishes, the whole
  message arrives over SSE.
- This is a deliberate trade-off. Token-level persistence carries a heavy write
  load, and chat messages, unlike investigations, are short.

## 7. Storage (Postgres)

`sqlx` is used — async, with migrations built in. Tests use `#[sqlx::test]`,
which creates a temporary database per test automatically.

**Queries use runtime validation (`sqlx::query`), not the `query!` macro.** The
first version promised compile-time validation and a `.sqlx` offline cache;
that is withdrawn:

- The list query branches four ways depending on the status filter and the
  presence of a cursor, and the macro requires literal SQL, which does not fit
  that shape.
- **Integration tests catch schema drift more broadly.** Every store function
  has a test that runs against a migrated database, so a dropped column fails a
  test. `cargo sqlx prepare --check` only inspects queries the macro covers,
  which is a narrower scope.
- Using the macro would make every `cargo build` require either a live database
  or a maintained `.sqlx` cache. That is a lot of friction for a five-table
  v0.1.

Accordingly, `cargo sqlx prepare --check` does not appear in the CI list in
Section 15.

**Every enum-like TEXT column gets a CHECK constraint.** Documenting the valid
values in a comment alone lets a wrong value in silently.

```sql
-- investigations
CREATE TABLE investigations (
    id             UUID PRIMARY KEY,
    title          TEXT        NOT NULL,
    prompt         TEXT        NOT NULL,
    status         TEXT        NOT NULL
        CHECK (status IN ('queued','running','completed','failed')),
    triggered_by   TEXT        NOT NULL
        CHECK (triggered_by IN ('user','alarm')),
    trigger_source TEXT,
    -- Three distinct timestamps. The first version used started_at as the
    -- creation time, which made queued investigations look started and left
    -- nowhere to record the finish time.
    queued_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at     TIMESTAMPTZ,           -- on entering Running
    finished_at    TIMESTAMPTZ,           -- on entering Completed|Failed
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The sole authority for agent_steps.seq. The application never computes
    -- seq (Section 6.1.1).
    next_step_seq  BIGINT      NOT NULL DEFAULT 0 CHECK (next_step_seq >= 0),
    -- trigger_source exists if and only if triggered_by is 'alarm'
    CONSTRAINT trigger_source_iff_alarm CHECK (
        (triggered_by = 'alarm') = (trigger_source IS NOT NULL)
    ),
    CONSTRAINT started_when_not_queued CHECK (
        (status = 'queued') = (started_at IS NULL)
    ),
    CONSTRAINT finished_iff_terminal CHECK (
        (status IN ('completed','failed')) = (finished_at IS NOT NULL)
    )
);
-- Keyset pagination for the list screen (status filter plus recency).
-- id must be DESC as well: a reverse btree scan flips every column uniformly,
-- so a (…, id ASC) index cannot produce (queued_at DESC, id DESC) in either
-- direction and Postgres adds a sort node.
CREATE INDEX ON investigations (status, queued_at DESC, id DESC);
CREATE INDEX ON investigations (queued_at DESC, id DESC);
-- Watchdog: find running investigations that have not been updated recently
-- (Section 6.1, invariant 4)
CREATE INDEX ON investigations (updated_at) WHERE status = 'running';
-- Queue restoration at boot (Section 17, assumption 4)
CREATE INDEX ON investigations (queued_at) WHERE status = 'queued';

-- Agent steps: the durable event log of an investigation, and the basis for
-- SSE replay.
CREATE TABLE agent_steps (
    investigation_id UUID        NOT NULL REFERENCES investigations(id) ON DELETE CASCADE,
    seq              BIGINT      NOT NULL,
    phase            TEXT        NOT NULL
        CHECK (phase IN ('all','chat','triage','rca','mitigation')),
    kind             TEXT        NOT NULL
        CHECK (kind IN ('thinking','text','tool_call','tool_result',
                        'artifact','terminated','error')),
    -- payload has the shape { "v": 1, ... }. Starting without a version leaves
    -- no way to interpret existing rows once the shape changes.
    payload          JSONB       NOT NULL
        CHECK (payload ? 'v'),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (investigation_id, seq),
    CONSTRAINT seq_non_negative CHECK (seq >= 0)
);

-- Knowledge / Instructions
CREATE TABLE instructions (
    id         UUID PRIMARY KEY,
    phase      TEXT        NOT NULL
        CHECK (phase IN ('all','chat','triage','rca','mitigation')),
    -- Prompt assembly order. Without an ordering, the system prompt's bytes
    -- differ per request and the prompt cache misses 100% of the time
    -- (Section 8.3).
    position   INTEGER     NOT NULL DEFAULT 0,
    title      TEXT        NOT NULL,
    body       TEXT        NOT NULL,
    enabled    BOOLEAN     NOT NULL DEFAULT true,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX ON instructions (phase, title);
CREATE INDEX ON instructions (phase, position, title) WHERE enabled;

-- Artifacts
CREATE TABLE artifacts (
    id               UUID PRIMARY KEY,
    investigation_id UUID REFERENCES investigations(id) ON DELETE SET NULL,
    title            TEXT        NOT NULL,
    body             TEXT        NOT NULL,   -- markdown
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON artifacts (investigation_id);
CREATE INDEX ON artifacts (created_at DESC);

-- Chat
CREATE TABLE chat_sessions (
    id         UUID PRIMARY KEY,
    title      TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Needed for recency ordering in the chat history sidebar
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON chat_sessions (updated_at DESC);

CREATE TABLE chat_messages (
    session_id UUID        NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    seq        BIGINT      NOT NULL,
    role       TEXT        NOT NULL CHECK (role IN ('user','assistant')),
    content    JSONB       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, seq)
);

-- MCP server configuration (secrets are never stored — Section 9)
CREATE TABLE mcp_servers (
    id         UUID PRIMARY KEY,
    name       TEXT        NOT NULL UNIQUE,
    transport  TEXT        NOT NULL CHECK (transport IN ('stdio','http')),
    config     JSONB       NOT NULL,   -- command or URL; secrets by env var name only
    enabled    BOOLEAN     NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-tool allow policy. The read-only annotation an MCP server reports about
-- itself is an advisory hint and cannot be trusted, so our own explicit
-- decision is stored here (Section 9).
CREATE TABLE tool_policies (
    server_name TEXT NOT NULL REFERENCES mcp_servers(name) ON DELETE CASCADE,
    tool_name   TEXT NOT NULL,
    policy      TEXT NOT NULL CHECK (policy IN ('allow','deny')),
    -- Marked by the operator as a tool that changes state. Used for UI warnings.
    mutating    BOOLEAN NOT NULL DEFAULT true,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_name, tool_name)
);

-- Automatic updated_at refresh. Do not depend on the application setting it by
-- hand on every UPDATE — missing it in one place breaks the watchdog and the
-- ordering.
CREATE FUNCTION touch_updated_at() RETURNS trigger AS $$
BEGIN NEW.updated_at = now(); RETURN NEW; END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER t_investigations_touch BEFORE UPDATE ON investigations
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER t_instructions_touch BEFORE UPDATE ON instructions
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER t_artifacts_touch BEFORE UPDATE ON artifacts
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER t_chat_sessions_touch BEFORE UPDATE ON chat_sessions
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER t_tool_policies_touch BEFORE UPDATE ON tool_policies
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
```

**`investigations.updated_at` must also advance when a step is appended.** The
watchdog (Section 6.1, invariant 4) uses that value to judge whether an
investigation has stalled, so if steps accumulate while the `investigations`
row is never updated, an active investigation is misjudged as stalled.
`append_step` performs `UPDATE investigations SET updated_at = now()` in the
same transaction.

The internal shape of the `payload`, `content`, and `config` JSONB columns is
**defined only by versioned Rust types in `agentops-core`** and serialized at
write time. Never hand-build arbitrary JSON and insert it.

Rules:
- All times are UTC; the database uses `TIMESTAMPTZ`
- The `(investigation_id, seq)` primary key rejects a duplicate `seq`. **Because
  the database allocates `seq`, ordinary appends cannot conflict.** Terminal
  step insertion uses no `ON CONFLICT` and rolls back on conflict — reasoning
  in Section 6.1.1
- `agent_steps.payload` is JSONB, so new step kinds need no schema change.
  There is no plan to search on the payload's interior (reads are always by
  `investigation_id` plus a `seq` range). If a search requirement appears, the
  needed fields are promoted to generated columns then.
- `agent_steps` runs to hundreds or thousands of rows per investigation. v0.1
  does not partition (this is not a metric time series, so it grows at a
  different rate). Deleting per investigation is a sufficient retention policy.

## 8. LLM provider abstraction

### 8.1 There is no official Rust SDK

**Anthropic ships official SDKs for Python, TypeScript, Java, Go, Ruby, C#, and
PHP, but not for Rust.** So the Messages API is called directly through
`reqwest`. This is an advantage rather than a constraint for provider
abstraction: all three providers speak the same wire format (the Messages API),
so the differences are confined to three places — **endpoint, authentication,
and model ID prefix.**

### 8.2 v0.1 implements only the Anthropic API

The trait ships in v0.1, but **only one implementation, for the Anthropic API**.

| Provider | Endpoint | Auth | Model ID | Status |
|---|---|---|---|---|
| Anthropic API | `https://api.anthropic.com/v1/messages` | `x-api-key` header | `claude-opus-5` | **v0.1** |
| Claude Platform on AWS | `https://aws-external-anthropic.{region}.api.aws/v1/messages` | AWS SigV4 (service `aws-external-anthropic`) plus a workspace ID | `claude-opus-5` (no prefix) | v0.2 |
| Amazon Bedrock | `https://bedrock-mantle.{region}.api.aws/anthropic/…` | AWS SigV4 | `anthropic.claude-opus-5` (prefix required) | v0.2 |

Common headers: `anthropic-version: 2023-06-01`,
`content-type: application/json`.

```rust
pub struct AnthropicProvider { api_key: SecretString, model: String }
```

**Why defer.** The argument in Section 8.1 — that all three providers share a
wire format and differ only in URL, auth, and model ID prefix — is exactly what
makes **adding them later cheap**. Putting them in v0.1, conversely, drags
`aws-sigv4`, the AWS credential resolution chain, two adapters, and their test
matrix onto the critical path.

Both AWS providers also have **wire details that could not be settled when this
spec was written.** The v0.2 spec must confirm them against official
documentation:

1. **How Claude Platform on AWS carries the workspace ID** — whether it is a
   header name or a body field. Only the environment variable name
   (`ANTHROPIC_AWS_WORKSPACE_ID`) is known; the wire representation is not.
2. **The SigV4 signing service name for the Bedrock Mantle endpoint** — the one
   for Claude Platform on AWS (`aws-external-anthropic`) was confirmed; the
   Bedrock Mantle value was not.

Without those two values the implementation is impossible anyway. Unconfirmed
values are not guessed into a spec.

### 8.3 Request shape (fixed for v0.1)

```json
{
  "model": "claude-opus-5",
  "max_tokens": 32000,
  "stream": true,
  "system": [{ "type": "text", "text": "<assembled instructions>",
               "cache_control": { "type": "ephemeral" } }],
  "thinking": { "type": "adaptive", "display": "summarized" },
  "output_config": { "effort": "high" },
  "tools": [ /* MCP tools serialized as custom tools */ ],
  "messages": [ /* conversation history */ ]
}
```

Reasoning and cautions:

- **`stream: true` is required.** Once `max_tokens` exceeds 16K, non-streaming
  requests hit HTTP timeouts. Investigations produce long output.
- **`thinking: {type: "adaptive"}`** — thinking is on by default for Claude
  Opus 5. It is stated explicitly in order to turn on `display: "summarized"`.
  With the default `"omitted"`, thinking blocks carry empty text and the UI
  shows no progress.
- **`max_tokens` is a combined ceiling on thinking plus response text.**
  Thinking is on, so leave headroom (starting at 32K).
- **`budget_tokens`, `temperature`, `top_p`, and `top_k` return 400 on
  Opus 5.** Never send them.
- **Assistant prefill is prohibited** — 400 on Opus 5, 4.8, 4.7, and 4.6. Use
  `output_config.format` when the output shape needs constraining.
- **Prompt cache** — the assembled instructions must be **byte-identical across
  requests**. Never put a timestamp or a UUID in the system prompt (it
  invalidates the entire prefix). Dynamic context goes toward the end of
  `messages`. The minimum cache unit on Opus 5 is 512 tokens.
  - **The instruction assembly order must be fixed.** `instructions_for` must
    sort with `ORDER BY position, title`. Postgres does not guarantee row order
    for a query without `ORDER BY`, so dropping the sort makes the assembled
    prompt differ per request and **drives the cache hit rate to zero.** That
    mistake would violate this section's own rule, which is why the schema in
    Section 7 has a `position` column.
- **Never change the tool list mid-conversation.** `tools` renders at the very
  front of the prefix, so changing it invalidates the whole cache.

### 8.4 Response handling — the stop_reasons that must be handled

```rust
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    PauseTurn,
    Refusal,
    StopSequence,
    ModelContextWindowExceeded,
    /// Keeps a deployment from breaking when new variants are added.
    #[serde(other)]
    Unknown,
}
```

Without `#[serde(other)]`, deserialization fails the day Anthropic adds a new
`stop_reason`. `Unknown` marks the investigation `failed` and records the raw
value as a step.

**`stop_reason` does not arrive until the `message_delta` at the end of the
stream.** "Check stop_reason before reading content" is therefore a
non-streaming way of thinking and does not hold here. The streaming path is
implemented as an **explicit state machine**:

```
Idle
 └ message_start        → Streaming { blocks: [] }
Streaming
 ├ content_block_start  → block begins (record its type)
 ├ content_block_delta  → accumulate the delta into that block
 │                        (text/thinking/signature/input_json)
 ├ content_block_stop   → block complete → append_step (Section 6.1.3)
 ├ ping                 → ignore
 ├ error                → Terminated{Error}
 └ message_delta        → stop_reason determined → Finalizing
Finalizing
 └ message_stop         → branch on stop_reason (below)
```

Never access blocks by index (no `content[0]`-style code). Iterate the
accumulated block list. A refusal is only determined after `message_delta`
arrives, and any partial output received before that is discarded.

- **`ToolUse`** — execute the tools and reply with **every `tool_result` in a
  single user message**. Splitting them across messages suppresses parallel
  tool calls. Failed tools must also return a result, with `is_error: true`
  (omitting it makes the API reject the request). If even one `tool_use_id`
  loses its pair, the follow-up request is rejected (Section 6.1.2).
- **`Refusal`** — arrives as HTTP 200, but `content` is empty or partial.
  `stop_details` can be `null` even on a refusal, so guard before reading
  `.category`, and put the value in `TerminalReason::Refusal { category }`.
  Opus 5 has strengthened cybersecurity safeguards, so false positives are
  possible on SRE and security-adjacent work. Mark the investigation `failed`
  and record the category as a step.
  - The Anthropic API has a server-side `fallbacks` parameter (beta) that
    automatically retries a refusal on a different model, but it is **not
    supported on Bedrock or Vertex.** For provider portability (Section 8.6) it
    is not used in v0.1. This is a recorded decision; if false positives become
    a real problem, enabling it optionally inside the Anthropic provider
    adapter alone will be reconsidered.
- **`ModelContextWindowExceeded`** — the context window overflowed. This is
  precisely the signal the handling policy in Section 18.3 targets. Without
  this variant, an overflow surfaces as an unknown error. Mark the
  investigation `failed` with a clear message.
- **`MaxTokens`** — the output was truncated. **Never use a truncated turn in a
  reply** — it may contain an incomplete `tool_use` block, and replying with it
  makes the API reject the request or call a tool with wrong arguments.
  Terminate the phase with `Terminated { MaxTokens }`, build a summary from
  what exists, and **move on to the next phase**. The investigation as a whole
  still ends `completed`, with a "output truncated" warning in the UI. It is
  not marked `failed` so that the earlier phases' work is not thrown away.
  Retrying the turn with a larger `max_tokens` is v0.2.
- **`PauseTurn`** — **append** the assistant's partial response to `messages` as
  an assistant turn, then send the same request again. Resending the identical
  request without appending the partial response duplicates work. Resumption
  counts follow the limit in Section 5.4. **However, since Section 8.6 decided
  against server-side tools, this path should never occur in v0.1.** Implement
  it defensively, but treat an actual occurrence as a signal that a server-side
  tool leaked in somewhere, and log a warning.

**No retry after partial streaming.** Automatically retrying a turn whose
stream was cut by a network error can duplicate text, tool calls, and tool side
effects. v0.1 treats a broken turn as `Terminated` and ends that phase.
Idempotent retry needs attempt-ID-based deduplication, so it is a follow-up.

### 8.5 SSE parsing

Anthropic's stream events: `message_start` → `content_block_start` →
`content_block_delta` (`text_delta` / `thinking_delta` / `input_json_delta`) →
`content_block_stop` → `message_delta` (`stop_reason`, usage) → `message_stop`.

**The parser must ignore unknown event types and keep going.** Beyond the list
above, keepalive `ping` events and mid-stream `error` events arrive. A parser
that handles only the enumerated types and fails on the rest **dies at the
first keepalive.** An `error` event is recorded as a step and terminates the
phase.

These are normalized into an internal `LlmEvent`, and the server converts them
again into SSE for the browser. The design states explicitly that **there are
two SSE layers**: Anthropic to server, and server to browser. The latter
carries HTML fragments that HTMX swaps directly.

### 8.6 Decisions that preserve provider portability

**MCP is implemented client-side.** The Anthropic API has an "MCP connector"
where the server connects to MCP servers itself, but it is **not supported on
Bedrock or Vertex.** Running the MCP client ourselves and serializing the
discovered tools as ordinary custom tools (`tools[]`) behaves identically on
all three providers. This is a deliberate choice in service of the
provider-agnostic principle.

For the same reason, nothing depends on server-side tools (web search, code
execution, the Files API, Batches) — they are unavailable on Bedrock.

## 9. MCP tool layer

`rmcp` 3.x (the official Rust MCP SDK) is used. The protocol is not
reimplemented.

```rust
pub struct McpToolRegistry {
    servers: Vec<McpConnection>,   // stdio or streamable HTTP
}
```

- At boot, connect to the enabled servers in the `mcp_servers` table and
  collect tools with `tools/list`
- Tool names are namespaced as `{server}__{tool}` to avoid collisions
- Tool definitions pass MCP's JSON Schema straight through as Anthropic's
  `input_schema`
- **The `tools[]` array must be serialized in a deterministic order** — sorted
  by namespaced name. `tools` renders at the very front of the prompt prefix,
  so if the order of an MCP server's `tools/list` response or the order in
  which servers connect varies between runs, the prompt cache misses every
  time. Same reasoning as the instruction ordering in Section 8.3.
- If one server dies, the remaining tools stay available, and connection status
  is exposed in the UI. **However, a change in the available tool set
  invalidates that investigation's prompt cache** — the tool list is frozen for
  the duration of a phase.
- **Tool output size limit** — output over 100K characters is truncated with a
  "truncated" marker. **Character count is not token count** — this limit is a
  rough defense and guarantees nothing about the context budget. The real
  defense is the per-phase tool call limit in Section 5.4.

### 9.1 Tool mutation policy must be enforceable

The principle "read-only tools by default" enforces nothing on its own. **The
read-only annotation an MCP server reports about itself is an advisory hint,
and since the server may be an untrusted party, no judgment depends on it.**

So the `tool_policies` table (Section 7) stores **our own explicit decision**:

- A newly discovered tool defaults to **`deny`**. An operator must explicitly
  change it to `allow` in Settings before the agent sees it.
- `ToolRegistry::list` returns only tools with an `allow` policy.
  `ToolRegistry::call` re-checks the policy immediately before execution (the
  policy can change after `list`).
- Enabling a tool marked `mutating: true` raises a warning in the UI.
- Annotations reported by the server are **displayed in the UI as reference
  information only** and never used for policy decisions.

Secret policy: **MCP server credentials are never stored in the database.** The
`config` JSONB holds only environment variable names
(`{"env": {"GITHUB_TOKEN": "$GITHUB_TOKEN"}}`), and the real values are read
from the process environment.

## 10. HTTP surface

### 10.1 Pages (HTML)

| Route | Contents |
|---|---|
| `GET /` | Redirect to Incidents |
| `GET /incidents` | Investigation launch form, frequency chart, investigation list |
| `GET /investigations/{id}` | Investigation detail — existing steps rendered server-side, with the last `seq` embedded in the stream URL (Section 6.1, invariant 2) |
| `GET /knowledge` | Instructions list and editor |
| `GET /artifacts` | Artifact list |
| `GET /artifacts/{id}` | Artifact body |
| `GET /settings` | MCP server and LLM provider configuration (read plus status) |

### 10.2 Fragments and API

| Route | Contents |
|---|---|
| `POST /api/investigations` | Launch an investigation → **`303 See Other`** → detail page |
| `GET /api/investigations` | List fragment (search, status filter, pagination) |
| `GET /api/investigations/{id}/stream?after={seq}` | **SSE** — step stream. Replays everything after `after`, then delivers live (Section 6.1, invariant 2) |
| `POST /api/chat/{session}/messages` | Send a chat message |
| `GET /api/chat/{session}/stream` | **SSE** — chat token stream. Reconnect semantics in Section 6.2 |
| `GET/POST/PUT/DELETE /api/instructions[/{id}]` | Instructions CRUD |
| `GET /api/health` | Health check — database, LLM, and MCP status |

### 10.2.1 SSE wire contract

Do not build the framing by hand — **use the encoder in
`axum::response::sse`.** Putting multi-line HTML into a `data:` field manually
makes newlines read as event boundaries and shreds the fragment; the encoder
prefixes every line with `data:`.

| Item | Contract |
|---|---|
| Event names | `step` (persisted, `id = seq`), `delta` (transient, no `id`), `terminal` (stream-end notification) |
| `data` | A rendered HTML fragment. HTMX swaps it into the target via `sse-swap`. |
| `id` | Only on `step`. The decimal form of `AgentStep.seq`. |
| Heartbeat | `KeepAlive` every 15 seconds, to stop proxies closing idle connections |
| Termination | Send a `terminal` event, then close the stream. A client that receives it does not reconnect. |
| `after` validity | Out-of-range values (negative, or above the current maximum `seq`) are not errors — they are **clamped to 0 or to the maximum**. A client arriving with a stale value must still get a stream. |

Because `step` events carry an `id`, `Last-Event-ID` works as an enhancement
when the browser reconnects automatically (item 6 of invariant 2 in
Section 6.1).

### 10.3 Charts

**SVG is rendered on the server** and delivered as a fragment. The daily
investigation frequency chart is a seven-day bar chart, so the **askama
template emits `<rect>` elements directly** — no charting library. This brings
the JavaScript dependency to zero, fits HTMX, and makes snapshot testing
possible. The price is no hover tooltips or zoom, which v0.1 accepts. If more
complex charts become necessary, `plotters`' SVG backend is introduced then.

## 11. Error handling

- Library crates use `thiserror` for concrete error types; `anyhow` at the
  binary boundary
- **Fault isolation** — the failure of one MCP server, one investigation, or
  one SSE connection never kills the whole server
- **Errors in the agent loop are recorded as steps.** The user is not just told
  "it failed"; where and why it failed stays in the timeline
- LLM 429 and 529 are retried with exponential backoff (up to 3 times,
  honoring the `retry-after` header). 400 is never retried
- Structured logging uses `tracing`. The investigation ID goes into the span so
  logs can be traced per investigation
- `axum::serve(...).with_graceful_shutdown(...)` — on shutdown, in-flight
  investigations are marked `failed` with the reason recorded as a step

## 12. Testing strategy

| Layer | Method |
|---|---|
| `agentops-core` | Pure unit tests. Fast, because there is no I/O dependency. |
| `agentops-llm` | **Stub Anthropic SSE responses with `wiremock`.** Golden fixtures cover every stop_reason path (end_turn / tool_use / refusal / pause_turn / max_tokens). Tests that call the real API do not run in CI. |
| `agentops-tools` | An in-memory fake MCP server verifies tool discovery, execution, and failure isolation. |
| `agentops-store` | `#[sqlx::test]` for a temporary Postgres database per test, including migration verification. |
| `agentops-agent` | A fake `LlmProvider` plus a fake `ToolRegistry` verify the whole loop deterministically: tool call round trips, resumption limits, error recording. |
| `agentops-server` | The `axum` test client verifies routes and HTML fragments. SSE replay is tested explicitly. |
| E2E | With a fake provider: launch → streaming → completion → artifact, end to end. |

### 12.1 Failure modes that must be tested

These are failure modes this design created for itself. All of them surface
only in production, so explicit tests are required.

The `[TEST-N]` and `[INV-N]` IDs assigned to each item **must appear in the
test function name** (as `test_4_...`, `inv_2_...`). `scripts/check_spec_test_ids.py`
checks this in CI, so deleting an invariant from the spec leaves an orphan test,
and deleting the test is blocked by CI. Items not yet implemented are marked
`plan 2` or `plan 3` and excluded from the check.

1. **Subscribe-replay race** — inject a step between the replay query and the
   channel subscription and confirm it reaches the client exactly once.
   Reversing the order must fail. `[TEST-1]`
2. **Deduplication** — when the replay range and the channel range overlap, the
   same `seq` must not be delivered twice. `[TEST-2]`
3. **`Lagged` recovery** — in a test harness with channel capacity reduced to
   1, create a slow consumer and confirm no steps are lost after `Lagged`
   thanks to the database resync. `[TEST-3]`
4. **`after` parameter replay** — a new connection receives only what follows
   `after={seq}`. Reproduces the situation with no `Last-Event-ID` (an actual
   page refresh). `[TEST-4]`
5. **All `stop_reason` branches** — `end_turn` / `tool_use` / `refusal` (plus
   `stop_details: null`) / `pause_turn` / `max_tokens` (mid tool loop) /
   `model_context_window_exceeded` / an unknown string. `[TEST-5]`
6. **Unknown SSE events** — the parser keeps going when `ping` and unknown
   event types are mixed into the stream. `[TEST-6]`
7. **`chat_messages.seq` serialization** — appending a user message and an
   assistant message concurrently must not collide on `seq`. `[TEST-7]`
8. **Watchdog** — a `running` investigation whose `updated_at` was backdated is
   cleaned up as `failed`, and a panicking task is marked `Failed`. `[TEST-8]`
9. **Determinism of instruction ordering** — reading the same instruction set
   sorts by `position` → `title` → `id`, so the text appended to the prompt is
   byte-identical on every call. This is a precondition for `TEST-9B` below.
   `[TEST-9]`
   - **9B. Prompt cache determinism** — the system prompt assembled from
     identical input and the `tools[]` serialization are byte-identical.
     Shuffling the MCP server connection order leaves `tools[]` unchanged.
     `[TEST-9B]`
10. **Phase transitions** — the three phases run in order and each receives
    only its own instructions. `[TEST-10]`
11. **Parallel `tool_use_id` pairing** — when three tool calls arrive in one
    turn, three `tool_result`s go back in a single user message with exactly
    matching IDs. Omitting one must fail. `[TEST-11]`
12. **Stream state machine** — reproduce the ordering where `stop_reason`
    arrives in `message_delta`, and confirm no refusal decision is made before
    that, and that blocks are not accessed by index. `[TEST-12]`
13. **Shutdown race** — when shutdown writes `Failed` while the task tries to
    write `Completed`, the conditional transition must prevent the overwrite.
    **Verified with real concurrency, not sequential calls** — both termination
    attempts are launched with `tokio::join!` and exactly one must succeed.
    `[TEST-13]`
14. **Terminal transactionality** — inject a database error **after the
    artifact is saved but before the status transition** and confirm no partial
    commit survives. A test that only exercises the success path does not
    verify this item. `[TEST-14]`
19. **A terminal step always survives** — an investigation that transitioned to
    `completed` or `failed` has **exactly one** `ArtifactWritten` or
    `Terminated` step. This assertion targets the P0 in Section 6.1.1 (silent
    loss of the terminal step) directly. `[TEST-19]`
20. **Boot cleanup racing `mark_running`** — an investigation that became
    `running` during cleanup must not be failed without a `Terminated` step.
    Verifies that cleanup is confined to the ID set captured at selection time.
    `[TEST-20]`
15. **Loop limits** — with a fake provider that never terminates, the turn,
    tool call, and wall-clock limits each fire, and a non-responding tool hits
    its timeout. `[TEST-15]`
16. **Persisted steps versus deltas** — pushing 1000 deltas grows `agent_steps`
    only by the number of semantic units. `[TEST-16]`
17. **Tool policy** — a newly discovered tool defaults to `deny` and does not
    appear in `list`. Changing the policy to `deny` after `list` makes `call`
    refuse. `[TEST-17]`
18. **`after` clamping** — opening a stream with a negative or oversized value
    works without error. `[TEST-18]`

## 13. Security

- **v0.1 has no authentication.** Default bind `127.0.0.1:3000`, with a warning
  at the top of the README against public exposure
- Secrets are referenced from environment variables or files only, never stored
  in the database
- Credentials are masked so they never reach logs or step payloads
- State-changing MCP tools are disabled by default
- LLM responses and tool output are **untrusted data.** Always escape when
  rendering HTML (askama's default behavior). Markdown rendering goes through
  sanitization.
- The system prompt states explicitly that instructions found in tool output
  are not to be treated as commands (prompt-injection mitigation)

## 14. Technology stack — pinned versions

Checked against crates.io on 2026-07-30. **Every version in the original brief
was stale.**

| Crate | Version | Original brief |
|---|---|---|
| `axum` | 0.8.9 | 0.7 |
| `tokio` | 1.53.1 | 1 |
| `tower-http` | 0.7.0 | 0.5 |
| `sqlx` | 0.9.0 | — |
| `askama` | 0.16.0 | — |
| `rmcp` | 3.0.1 | — |
| `reqwest` | latest (rustls) | — |
| `serde` / `serde_json` | 1.x | 1.0 |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | same |
| `thiserror` / `anyhow` | 1.x | — |
| `aws-sigv4` | **unused in v0.1** (arrives with the AWS providers in v0.2) | — |

`rmcp 3.0.1` was published on 2026-07-29 (confirmed directly against the
crates.io API). The docs.rs cache can lag and show 2.2.0 as the latest, so
crates.io is the reference for version checks.

**The `main.rs` example in the original brief does not compile.**
`axum::Server` was removed in 0.7:

```rust
let listener = tokio::net::TcpListener::bind(addr).await?;
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?;
```

Other decisions:
- **Templates use askama** — compile-time type checking. With dozens of HTMX
  fragments, a typo becomes a build error rather than a runtime 500.
- **Tailwind does not use a CDN.** The standalone Tailwind CLI binary generates
  CSS at build time and it is served statically through `ServeDir`. No Node
  dependency.
- **HTMX and the SSE extension** are vendored at pinned versions, removing the
  CDN dependency.

## 15. Open-source hygiene

- `LICENSE` (MIT), `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`
- `docker-compose.yml` — Postgres plus the app, so a contributor gets a dev
  environment with one command
- CI: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test --all-targets`, `cargo test --doc`, and the drift-guard scripts
  (`cargo sqlx prepare --check` is excluded, for the reason in Section 7)
- **The project name needs checking**: the repository is `agentops`, but
  whether the name is taken on crates.io must be confirmed before publishing

## 16. Follow-up specs (in priority order)

0. **AWS LLM providers** — Amazon Bedrock and Claude Platform on AWS adapters.
   Confirming the two unresolved wire details from Section 8.2 (how the
   workspace ID is carried, and the Bedrock Mantle SigV4 service name) against
   official documentation is a precondition for this spec. It is small enough
   to handle early in v0.2.
1. **Topology** — automatic generation and rendering of the service dependency
   graph
2. **Connectors, anomaly detection, and alarms** — metric collection (a
   `Connector` trait, an OTel canonical model), statistical detection, and the
   `Triggered By: Alarm` path. Alarm deduplication (a state machine plus a
   dedup key plus a cooldown) is the hard problem in that spec.
3. **Improvements** — weekly scheduled evaluation and preventive
   recommendations learned from past investigations
4. **Knowledge expansion** — Skills and Memories
5. **Multi-tab workspaces**
6. **Authentication and multi-tenancy**
7. **Changes** — release-risk review and test plan generation
8. **Custom agents**
9. **Artifact versioning**
10. **Usage metering**

### 16.1 Connector design (settled for the follow-up spec)

Metric collection is out of scope for v0.1, but the design direction was
settled in this discussion, so it is recorded:

- **A `Connector` trait** abstracts the source. AWS CloudWatch, GCP Monitoring,
  Prometheus, and others plug in as adapters.
- **The canonical data model is OpenTelemetry** —
  `Resource { attributes }` plus
  `Metric { name, unit, data_points[{ timestamp, value, attributes }] }`. It can
  hold CloudWatch's `(Namespace, MetricName, Dimensions[], Statistic, Period)`
  and GCP's `(metric.type, resource.labels, MonitoredResource)` without loss,
  and multi-cloud mapping is already a proven part of that spec. The
  connector's role becomes clearly "translate vendor format to OTel format".
- **A pull model** — `collect(window: TimeWindow) -> Vec<ResourceMetrics>`. Both
  AWS and GCP expose time-range query APIs, so this fits naturally. If a push
  source (OTLP ingestion) becomes necessary, it gets its own trait rather than
  being forced into this one.
- The time-series identifier is the **SHA-256 of the sorted attribute set**
  (the OTel identity convention). The same series keeps the same ID across
  re-collection and restarts.
- The `points` table has primary key `(series_id, ts)`, making re-collection
  idempotent. Declarative partitioning on `ts` plus a BRIN index. **No
  dependency on TimescaleDB** (it raises the self-hosting barrier).
- A single connector cannot demonstrate that the abstraction holds. When that
  spec is written, **a mapping table for both the AWS and GCP connectors is
  written first** to validate the trait.

## 17. Explicit assumptions

This design rests on the following. If one is wrong, revisit the corresponding
section.

1. **v0.1 users are engineers deploying to their own infrastructure.** That is
   what makes the absence of authentication and the loopback bind acceptable in
   v0.1.
2. **MCP servers provide the data an investigation needs.** The product is
   useful when attached to an existing observability stack (Prometheus MCP,
   Grafana MCP, and so on) without collecting metrics itself.
3. **A single process is sufficient.** Investigations run as tokio tasks in the
   same process. Distributed workers can be swapped in behind `JobManager` if
   they become necessary — which is why `JobManager` is a trait boundary.
4. **Concurrent investigations number in the single digits.** `JobManager`
   limits concurrency with a `tokio::sync::Semaphore` (permits configurable,
   default 3). Excess requests stay in the database as `Queued`, and a
   scheduler task picks them up one at a time as permits free. **At boot, the
   queue is restored by reading `Queued` investigations from the database** —
   which meshes with "`Queued` ones are rescheduled" in Section 6.1,
   invariant 4.

## 18. Open questions

Each item has a v0.1 default, so none of them blocks getting started. They are
revisited after real use.

1. **Investigation titles** — v0.1 truncates the first 80 characters of the
   prompt with an ellipsis. LLM summarization costs an extra call, so it is
   deferred.
2. **The `effort` default** — starting at `high`. Opus 5 is strong at `low` and
   `medium` too, so this is swept after real use and tuned per route.
3. **Context window overflow — the least certain assumption here.** On
   receiving `stop_reason: model_context_window_exceeded`, v0.1 marks the
   investigation `failed`. Two mitigations exist: the per-phase independent
   conversations in Section 5.3 bound growth per phase, and the per-phase tool
   call limit of 120 in Section 5.4 structurally caps history size.

   A concern remains, and it is a fair one: **the 100K-character tool output
   limit guarantees nothing about the token budget.** Calling a
   large-output tool several times can reach a 1M window, and if the entire
   investigation is discarded at that point, the "long-running investigation"
   product claim collapses. That is why Section 8.4 changed `MaxTokens` from
   `failed` to "terminate that phase and move on" — the same logic can apply to
   context overflow.

   The current policy stands so as not to block v0.1, but **whether this is
   actually reached in real use is the first thing to measure.** If it is
   observed, the options are considered in order: (1) demote context overflow
   to a phase termination, (2) token accounting before each turn with
   `count_tokens`, (3) rolling summarization of history, (4) storing large tool
   output as an artifact and keeping only a reference in context.
4. **Promoting a chat to an investigation** — v0.1 prepends the chat session's
   full message history to the investigation prompt as context. If long
   sessions become a problem, it is limited to the last N turns.
5. **The phase transition mechanism** — Section 5.3 chose fixed sequencing.
   Whether letting the LLM transition itself through a `set_phase` tool works
   better in practice is re-evaluated after v0.1 is in use, when there is data
   on whether phase-scoped instructions actually help.

## 19. Verification history

### 19.1 First review (2026-07-30)

**Major defects accepted:**

| Severity | Defect | Resolution |
|---|---|---|
| Critical | `Last-Event-ID` is not sent on a page refresh (the browser creates a new `EventSource`). The htmx SSE extension's retry path also loses the header. The entire replay design depended on a header that does not arrive. | Rewrote invariant 2 in Section 6.1 — a `?after={seq}` URL parameter plus server rendering |
| Critical | Subscribing after the replay query loses any step written in between | Fixed the order: subscribe → replay → deduplicate |
| Major | `Phase` transition rules were undefined, making phase-scoped instructions dead code | Added Section 5.3 (fixed sequential sub-loop) |
| Major | A panicking or stalled task is not caught by boot cleanup | Added `JoinHandle` watching plus an `updated_at` watchdog to invariant 4 |
| Major | Unhandled `broadcast` `Lagged` silently loses steps | Added invariant 3 (recovery by database resync) |
| Major | Thinking blocks must be echoed back verbatim, so `messages[]` cannot be reconstructed from steps that store only summaries | Added Section 6.1.2 — the loop owns the originals, and resumption is stated as impossible |
| Major | `chat_messages.seq` has two writers, so `MAX(seq)+1` races | Section 6.1.1 — append is serialized per session |
| Major | Without instruction ordering the prompt cache misses 100% of the time (violating the spec's own rule) | A `position` column plus an explicit `ORDER BY` |
| Major | `model_context_window_exceeded` was missing from `stop_reason` | Added, along with `#[serde(other)]` tolerance |
| Minor | 302/303 inconsistency, a wrong cross-reference, no CHECK constraints, missing indexes, no `updated_at` trigger, `ping`/`error` SSE events unconsidered, `MaxTokens` handling mid tool loop undefined, no `stop_details` null guard, no concurrency limit mechanism, chat reconnect semantics, `cargo sqlx prepare` | Addressed in the respective sections |
| Scope | Two AWS providers widen the critical path unnecessarily | Moved to v0.2 (Sections 3.2, 8.2, and 16-0) |

**One defect rejected:**

- *"`rmcp 3.0.1` does not exist; the latest is 2.2.0"* — **not true.** Querying
  the crates.io API directly showed `3.0.1` published on 2026-07-29 and not
  yanked. The docs.rs cache the review consulted was a day behind. The version
  stays at `3.0.1`. (The verification method is recorded in Section 14.)

This one is kept as an example of why review findings are not accepted
unconditionally.

### 19.2 Second review (2026-07-30)

A second independent review was obtained. It reached **the same seven defects
as the first, independently** (the subscribe-replay race, `Lagged`, task
watching, CHECK constraints, undefined phase transitions, the chat `seq` race,
and the tests' lack of concurrency coverage). Items reached by two different
paths are treated as high-confidence.

**Additional defects accepted:**

| Severity | Defect | Resolution |
|---|---|---|
| P1 | `StepKind` has no `tool_use_id`. With two or more parallel tool calls there is no way to pair calls with results, and the spec explicitly wants parallel calls (Section 8.4). Thinking's `signature_delta` was also missing. | Added `tool_use_id`, and rewrote Section 6.1.2 as a "protocol transcript versus UI projection" table |
| P1 | The agent loop has no limits. Only `PauseTurn` is capped at 5; the tool call loop, wall clock, and tool timeouts are unbounded | Added Section 5.4 — seven limits fixed in a table |
| P1 | `stop_reason` does not arrive until `message_delta`, so "check it first" is impossible when streaming | Added an explicit stream state machine to Section 8.4 |
| P1 | Terminal writes (artifact plus step plus status transition) were not transactional, allowing partial success | `complete_investigation` / `fail_investigation` as transactional operations |
| P1 | Shutdown can write `Failed` while the task writes `Completed` over it | Conditional transitions (`WHERE status='running'`) plus the six-stage shutdown order |
| P1 | "The primary key makes appends idempotent" is inaccurate — a primary key rejects duplicates, it does not deduplicate retries | Stated `ON CONFLICT DO NOTHING` explicitly and corrected the original wording |
| P2 | The data flow diagram read as calling `append_step` per delta, contradicting the row estimate in Section 7 | Added Section 6.1.3 — persisted steps only at semantic boundaries, deltas for SSE only |
| P2 | Non-deterministic tool ordering breaks the prompt cache (`tools[]` renders at the front of the prefix) | Section 9 specifies sorting by name |
| P2 | "Read-only by default" had no enforcement mechanism; MCP's read-only annotation is an advisory hint and cannot be trusted | Added Section 9.1 plus the `tool_policies` table, defaulting to `deny` |
| P2 | `started_at` was the creation time, making queued investigations look started and leaving nowhere for the finish time | Split into `queued_at`/`started_at`/`finished_at` plus CHECK constraints |
| P2 | No index for list filtering and pagination | Added keyset indexes |
| P2 | The JSONB shape was undefined | Enforced a `v` field in `payload`, serialized only from versioned core types |
| P2 | The terminal status for `MaxTokens` was undefined | Terminate the phase and continue; the investigation ends `completed` with a warning |
| P2 | It was not stated that resending after `PauseTurn` must append the partial response | Stated in Section 8.4 |
| P2 | Retrying after partial streaming can create duplicates | Stated that automatic retry does not happen |
| P3 | The `agentops-core` dependency table omitted `serde_json`, `async-trait`, and `futures-core` | Added |
| P3 | The reasoning for `#[async_trait]` and the `Send + 'static` requirement were unstated | Stated in Section 5.2 |
| P3 | The SSE wire contract (event names, multi-line encoding, heartbeat, termination, invalid `after`) was undefined | Added Section 10.2.1 |
| — | `TerminalReason` was unstructured, forcing `stop_details.category` into a string | Added the `TerminalReason` enum |

**Defects rejected or downgraded:**

- *`agentops-agent` depending on the concrete adapter crates (llm/tools/store)
  undermines the crate-boundary argument* — **rejected.** The boundary argument
  in Section 4.1 is "keep the core from depending on sqlx/reqwest", and that
  holds unchanged. A crate responsible for composition knowing the concrete
  implementations is the normal composition-root pattern, and
  `agentops-server` already depends on everything. Tests are fine too, as long
  as fakes implement the same traits.
- *A capability layer is needed to handle feature differences between
  providers* — meaningless now that v0.1 has a single provider. Moved to the
  preliminary review items of Section 16-0 (the AWS provider spec).
- *A context and token budget is needed even in v0.1* — the technical content
  of the objection (character count is not token count) is correct, but this is
  something Section 18.3 already defers consciously. The risk assessment is
  sound, so Section 18.3 was expanded to state the mitigations and the
  measurement plan.
- *The missing `agentops-core` dependency table entries are P1* — downgraded to
  P3. It is a documentation-completeness problem solved in the first five
  minutes of implementation, not a structural defect.

### 19.3 Third review (2026-07-30) — flowing back from plan verification

An independent review of plan 1
(`docs/superpowers/plans/2026-07-30-foundation.md`) **flowed back as defects in
the spec itself.** Because the plan implemented the spec faithfully, a P0 found
in the plan was a P0 in the spec.

| Severity | Defect | Resolution |
|---|---|---|
| **P0** | Section 6.1.1's claim that "`run_investigation` is the sole writer" is **wrong.** The watchdog (INV-4) is a second writer, and terminating a stalled-but-alive task is the watchdog's entire reason for existing. When both paths reach the same `seq`, `ON CONFLICT DO NOTHING` **swallows the terminal step while the status transition commits**, leaving a `completed` investigation with no artifact record | Rewrote Section 6.1.1 entirely — `investigations.next_step_seq` makes the database the sole `seq` authority, and terminal step insertion uses no `ON CONFLICT`. The `ON CONFLICT DO NOTHING` that the second review introduced was, in this context, a device that hid errors |
| P1 | Boot cleanup took an ID set with `FOR UPDATE` and then ran an unconditional `UPDATE ... WHERE status='running'`. Under READ COMMITTED a fresh snapshot is taken per statement, so an investigation that `mark_running` moved to `running` after the `SELECT` gets failed with no `Terminated` step | Confined cleanup to the ID set captured at selection time (Section 12.1, TEST-20) |
| P1 | Section 6.1.1 chose an mpsc actor for chat `seq` and rejected `SELECT ... FOR UPDATE` on the grounds that it "holds a transaction open across streaming", but **that reasoning was wrong** — Section 6.2 already decided messages are written once, on completion, so no transaction wraps the streaming | Settled on `FOR UPDATE`, removing the actor layer |
| P1 | TEST-13 and TEST-14 in Section 12.1 were described in a form that does not verify what their names claim (sequential calls, success path only) | Revised to require real concurrency and error injection. Added TEST-19 and TEST-20 |
| P2 | The index was `(status, queued_at DESC, id)` with `id` ascending. A reverse btree scan flips every column uniformly, so it cannot produce `(queued_at DESC, id DESC)` in either direction and a sort node is added | Corrected to `id DESC` |
| P2 | Section 7 promised compile-time SQL validation and a `.sqlx` offline cache, but the plan uses runtime queries | **The spec was corrected.** Integration tests against a migrated database catch schema drift more broadly, and `cargo sqlx prepare --check` only inspects queries the macro covers. Removed from the CI list in Section 15 |

**Two items where the direction was reversed.** The chat `seq` and compile-time
SQL were reported as "the plan violates the spec", but on examination **the
spec was the side that was wrong.** Rather than bending the plan to the spec,
the spec was corrected.

**One item where the reviewer withdrew its own hypothesis.** The review was
prompted with a suspicion that chat's `SELECT ... FOR UPDATE` would cause a
deadlock or a lost update, and it did not confirm that suspicion — it refuted
it. Contenders for a single row form a wait queue, so there is no cycle, and
each waiter takes a fresh snapshot after acquiring the lock, so there is no
lost update. Its further point was also correct: the proposed alternative
(`INSERT ... SELECT COALESCE(MAX+1,0)`) is more dangerous, because it computes
the same maximum without a lock.

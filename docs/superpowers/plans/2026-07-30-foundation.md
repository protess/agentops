---
type: Implementation Plan
title: agentops foundation layer implementation plan (plan 1 of 3)
description: Build the workspace, domain types, traits, the Postgres store, and the drift-guard CI, by TDD
status: stable
tags: [plan, foundation, rust, postgres, sqlx, ci]
generated:
  by: claude-opus-5
  at: 2026-07-30
sources:
  - resource: docs/superpowers/specs/2026-07-30-agentops-design.md
    author: claude-opus-5
    last_modified: 2026-07-30
    note: The design document this plan implements
stale_after: 2026-10-30
supersedes: []
verified:
  - kind: machine
    by: claude-opus-5 (final whole-branch review)
    at: 2026-07-30
    scope: 27 commits and 59 tests after implementation — crate boundaries, the seq authority, conditional terminal transitions, concurrency. The twelve per-task reviews are separate.
    result: Ready to finish — Critical 0, Important 0, Minor 2 (deferred). The reviewer read Task 11's un-gated module in full, closing a review gap.
---

# agentops foundation layer implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Commit hashes in this document do not resolve.** The repository's history
> was squashed to a single commit on 2026-08-05, deleting the 169 commits these
> hashes named. They are kept because each still identifies *which* change carried
> a piece of evidence, and removing them would delete that distinction too — but
> `git show <hash>` will fail, and no copy of those commits exists.

**Goal:** Build the domain types, the traits, the Postgres store, and a CI that turns documentation-to-code drift into a build failure. This is the foundation the agent loop and the web layer sit on.

**Architecture:** A Cargo workspace makes the compiler enforce the crate boundaries. `agentops-core` knows nothing about I/O and holds only domain types and traits. `agentops-store` implements the `Store` trait against Postgres. **SQL uses runtime validation (`sqlx::query`)** — the `query!` macro is not used, so there is no `.sqlx` offline cache either. Schema drift is caught by integration tests running against a migrated database (reasoning in the design spec, Section 7).

**`seq` is allocated by Postgres, not by the application.** `investigations.next_step_seq` is the sole authority (spec Section 6.1.1). With an in-memory counter the watchdog becomes a second writer and terminal steps are silently lost.

**Tech Stack:** Rust >= 1.94 (required by sqlx 0.9), sqlx 0.9 (Postgres), tokio 1.53, time, serde, thiserror, uuid

## Global Constraints

Project-wide requirements taken verbatim from the design document. **They are implicitly part of every task's requirements.**

- **Rust floor 1.94.** `sqlx 0.9.0` itself declares `rust-version = 1.94`, so anything lower fails immediately with `error: rustc X is not supported by the following packages: sqlx@0.9.0 requires rustc 1.94.0`. State `rust-version = "1.94"` in `Cargo.toml`.
- **Crate version floors**: `sqlx = 0.9`, `tokio = 1.53`, `serde = 1`, `uuid = 1`, `time = 0.3`, `thiserror = 2`, `anyhow = 1`
- **Run `cargo fmt --all` before the final commit of every task.** The code pasted into this plan is not in rustfmt shape. Formatting per task is what lets Task 12's `--check` gate pass — doing it all at the end makes every earlier commit fail `cargo fmt --check`.
- **`agentops-core` never depends on an I/O crate.** Never put `sqlx`, `reqwest`, or `tokio` in `agentops-core`'s `[dependencies]`. This constraint is the only reason the workspace is split (spec Section 4.1).
- **All times are UTC.** `TIMESTAMPTZ` in the database, `time::OffsetDateTime` in Rust (spec Section 7).
- **Every enum-like TEXT column gets a CHECK constraint.** Do not document the valid values in a comment alone (spec Section 7).
- **Queries against `instructions` must use `ORDER BY position, title`.** Without the sort, the assembled system prompt differs per request and the prompt cache hit rate goes to zero (spec Section 8.3).
- **`seq` is allocated by the database.** One statement, `UPDATE investigations SET next_step_seq = next_step_seq + 1, updated_at = now() WHERE id = $1 RETURNING next_step_seq - 1`, handles both the allocation and the `updated_at` refresh the watchdog depends on. The application never computes `seq` (spec Section 6.1.1).
- **Terminal step insertion (`Terminated`, `ArtifactWritten`) uses no `ON CONFLICT`.** On conflict, roll back and return `Conflict`. Using `DO NOTHING` for a terminal step commits the status transition while losing the step (spec Section 6.1.1).
- **Terminal status transitions are conditional.** They take the form `UPDATE ... WHERE id = $1 AND status = 'running'`, and when zero rows are affected the caller backs off, treating it as another party having already terminated it (spec Section 6.1, invariant 4).
- **There is no authentication in v0.1.** The default bind is `127.0.0.1`. This plan has no HTTP layer so it does not apply here, but no task builds authentication scaffolding (spec Section 3.2).
- Commit messages follow Conventional Commits (`feat:`, `test:`, `chore:`, `ci:`, `docs:`)

---

## File Structure

```
Cargo.toml                          # workspace root, shared dependency versions
rust-toolchain.toml                 # pinned toolchain
.github/workflows/ci.yml            # fmt, clippy, test, doc test, drift guards
docker-compose.yml                  # Postgres for development
.env.example                        # a DATABASE_URL example
migrations/
  0001_initial.sql                  # every table, plus CHECKs, indexes, and triggers
crates/
  agentops-core/
    Cargo.toml
    src/lib.rs                      # module declarations only
    src/investigation.rs            # Investigation, InvestigationStatus, TriggeredBy
    src/step.rs                     # AgentStep, StepKind, TerminalReason, Phase
    src/knowledge.rs                # Instruction, Artifact
    src/chat.rs                     # ChatSession, ChatMessage, ChatRole
    src/error.rs                    # StoreError, LlmError, ToolError, JobError
    src/traits.rs                   # Store, LlmProvider, ToolRegistry, JobManager
  agentops-store/
    Cargo.toml
    src/lib.rs                      # the PgStore struct and the assembled Store impl
    src/investigations.rs           # investigation CRUD, status transitions, keyset listing
    src/steps.rs                    # append_step·steps_after
    src/instructions.rs             # instruction CRUD and ordered reads
    src/artifacts.rs                # artifacts and the transactional termination operations
    src/chat.rs                     # sessions and messages (seq serialization)
    tests/investigations.rs
    tests/steps.rs
    tests/instructions.rs
    tests/artifacts.rs
    tests/chat.rs
scripts/
  check_stale_after.py              # the stale_after expiry scan
  check_spec_test_ids.py            # spec-to-test ID traceability
```

**Why this split:** Keeping `agentops-store` in one file would mix the SQL of five tables together. Splitting by table gives each file one responsibility, and test files correspond one-to-one, making the unit of review clear.

---

### Task 1: Workspace and toolchain verification

**Purpose:** Confirm the dependency set actually compiles, before anything else. If a version assumption is wrong, it should surface now rather than at Task 5.

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `crates/agentops-core/Cargo.toml`
- Create: `crates/agentops-core/src/lib.rs`
- Create: `crates/agentops-store/Cargo.toml`
- Create: `crates/agentops-store/src/lib.rs`
- Create: `docker-compose.yml`
- Create: `.env.example`

**Interfaces:**
- Consumes: nothing (this is the first task)
- Produces: the workspace crates `agentops-core` and `agentops-store`. Every later task adds code to these two.

- [ ] **Step 1: Write the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2021"
# sqlx 0.9.0 declares rust-version = 1.94. Anything lower fails immediately with
# "sqlx@0.9.0 requires rustc 1.94.0".
rust-version = "1.94"
license = "MIT"
repository = "https://github.com/protess/agentops"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
time = { version = "0.3", features = ["serde", "macros"] }
thiserror = "2"
anyhow = "1"
async-trait = "0.1"
futures-core = "0.3"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "sync", "time"] }
sqlx = { version = "0.9", default-features = false, features = [
  "runtime-tokio", "tls-rustls", "postgres", "uuid", "time", "json", "migrate", "macros",
] }
```

- [ ] **Step 2: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

With `stable`, CI is fine. If the local toolchain is below 1.94, Step 6's build
fails, so raise it with `rustup update` — `rust-version` reports that situation
with the clear message `sqlx@0.9.0 requires rustc 1.94.0`.

- [ ] **Step 2b: Confirm the local toolchain meets the floor**

Run: `cargo --version && rustc --version`
Expected: rustc **1.94** or newer. If lower, run `rustup update stable` and check
again. If an older version is pinned by something like `asdf`, raise that first.

- [ ] **Step 3: Create the `agentops-core` crate**

`crates/agentops-core/Cargo.toml`:

```toml
[package]
name = "agentops-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
time.workspace = true
thiserror.workspace = true
async-trait.workspace = true
futures-core.workspace = true
```

`crates/agentops-core/src/lib.rs`:

```rust
//! agentops domain types and traits. Contains no I/O.
//!
//! Never add `sqlx`, `reqwest`, or `tokio` as dependencies of this crate.
//! Preventing exactly that is the purpose of the crate boundary (design spec, Section 4.1).
```

- [ ] **Step 4: Create the `agentops-store` crate**

`crates/agentops-store/Cargo.toml`:

```toml
[package]
name = "agentops-store"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
agentops-core = { path = "../agentops-core" }
sqlx.workspace = true
serde_json.workspace = true
uuid.workspace = true
time.workspace = true
async-trait.workspace = true
thiserror.workspace = true

[dev-dependencies]
tokio.workspace = true
```

`crates/agentops-store/src/lib.rs`:

```rust
//! The Postgres implementation of the `Store` trait.
```

- [ ] **Step 5: Write the development Postgres and environment files**

`docker-compose.yml`:

```yaml
services:
  postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: agentops
      POSTGRES_PASSWORD: agentops
      POSTGRES_DB: agentops
    # Mapped to 55433 — on this machine 5432 and 55432 are taken by an ssh tunnel and another project's container.
    # On a collision, change this value and DATABASE_URL in .env together.
    ports: ["55433:5432"]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U agentops"]
      interval: 2s
      timeout: 3s
      retries: 20
```

`.env.example`:

```
DATABASE_URL=postgres://agentops:agentops@localhost:55433/agentops
```

- [ ] **Step 6: Verify the dependencies actually compile**

Run:
```bash
cargo build --workspace 2>&1 | tail -20
```

Expected: it ends with `Finished`. On a compile error, **stop here**, check the failing crate's version with `cargo search <name>`, and fix `Cargo.toml`. Do not move on to Task 2 without passing this step.

- [ ] **Step 7: Confirm Postgres starts**

Run:
```bash
docker compose up -d && docker compose ps
```

Expected: the `postgres` service is `healthy`.

- [ ] **Step 8: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add Cargo.toml rust-toolchain.toml crates docker-compose.yml .env.example
git commit -m "chore: scaffold cargo workspace with core and store crates"
```

---

### Task 2: Investigation domain types

**Files:**
- Create: `crates/agentops-core/src/investigation.rs`
- Modify: `crates/agentops-core/src/lib.rs`
- Test: `crates/agentops-core/src/investigation.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: the `agentops-core` crate from Task 1
- Produces:
  - `enum InvestigationStatus { Queued, Running, Completed, Failed }` — `as_str()` → `"queued"|"running"|"completed"|"failed"`, plus a `FromStr` implementation
  - `enum TriggeredBy { User, Alarm { source: String } }` — `kind_str()` → `"user"|"alarm"`, `source()` → `Option<&str>`
  - `struct Investigation { id: Uuid, title: String, prompt: String, status: InvestigationStatus, triggered_by: TriggeredBy, queued_at: OffsetDateTime, started_at: Option<OffsetDateTime>, finished_at: Option<OffsetDateTime>, updated_at: OffsetDateTime }`

- [ ] **Step 1: Write the failing test**

Write into `crates/agentops-core/src/investigation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_through_db_string() {
        for s in [
            InvestigationStatus::Queued,
            InvestigationStatus::Running,
            InvestigationStatus::Completed,
            InvestigationStatus::Failed,
        ] {
            assert_eq!(s.as_str().parse::<InvestigationStatus>().unwrap(), s);
        }
    }

    #[test]
    fn status_rejects_unknown_string() {
        assert!("bogus".parse::<InvestigationStatus>().is_err());
    }

    #[test]
    fn triggered_by_user_has_no_source() {
        let t = TriggeredBy::User;
        assert_eq!(t.kind_str(), "user");
        assert_eq!(t.source(), None);
    }

    #[test]
    fn triggered_by_alarm_carries_source() {
        let t = TriggeredBy::Alarm { source: "cpu-high".into() };
        assert_eq!(t.kind_str(), "alarm");
        assert_eq!(t.source(), Some("cpu-high"));
    }

    #[test]
    fn terminal_statuses_are_identified() {
        assert!(!InvestigationStatus::Queued.is_terminal());
        assert!(!InvestigationStatus::Running.is_terminal());
        assert!(InvestigationStatus::Completed.is_terminal());
        assert!(InvestigationStatus::Failed.is_terminal());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-core 2>&1 | tail -20`
Expected: FAIL — `cannot find type InvestigationStatus in this scope`

- [ ] **Step 3: Write the minimal implementation**

Write **above** the test module in `crates/agentops-core/src/investigation.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// Investigation status. Maps one-to-one to the database string and is the same set as the CHECK constraint in `migrations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl InvestigationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// A state that no longer transitions.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl fmt::Display for InvestigationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown investigation status: {0}")]
pub struct ParseStatusError(String);

impl FromStr for InvestigationStatus {
    type Err = ParseStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(ParseStatusError(other.to_owned())),
        }
    }
}

/// What triggered the investigation. Only `Alarm` carries a `source` — the
/// `trigger_source_iff_alarm` CHECK constraint in the database enforces this invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TriggeredBy {
    User,
    Alarm { source: String },
}

impl TriggeredBy {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Alarm { .. } => "alarm",
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::User => None,
            Self::Alarm { source } => Some(source),
        }
    }
}

/// One investigation. Three distinct timestamps: queued, started, finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Investigation {
    pub id: Uuid,
    pub title: String,
    pub prompt: String,
    pub status: InvestigationStatus,
    pub triggered_by: TriggeredBy,
    pub queued_at: OffsetDateTime,
    pub started_at: Option<OffsetDateTime>,
    pub finished_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}
```

Add to `crates/agentops-core/src/lib.rs`:

```rust
pub mod investigation;

pub use investigation::{Investigation, InvestigationStatus, TriggeredBy};
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-core 2>&1 | tail -15`
Expected: PASS — `test result: ok. 5 passed`

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-core/src/investigation.rs crates/agentops-core/src/lib.rs
git commit -m "feat(core): add investigation domain types"
```

---

### Task 3: Step and termination reason types

**Purpose:** Block the second review's P1 defects — the missing `tool_use_id` and the unstructured termination reason — at the type level.

**Files:**
- Create: `crates/agentops-core/src/step.rs`
- Modify: `crates/agentops-core/src/lib.rs`
- Test: `crates/agentops-core/src/step.rs` (inline)

**Interfaces:**
- Consumes: `agentops-core` from Task 2
- Produces:
  - `enum Phase { All, Chat, Triage, Rca, Mitigation }` — `as_str()`, `FromStr`, and the constant `INVESTIGATION_ORDER: [Phase; 3]`
  - `enum TerminalReason` — `Refusal { category: Option<String> }`, `ContextWindowExceeded`, `MaxTokens`, `TurnLimitExceeded`, `WallClockExceeded`, `ToolTimeout { tool: String }`, `ShutdownRequested`, `TaskPanicked`, `UnknownStopReason(String)`
  - `enum StepKind` — `Thinking { summary }`, `Text { text }`, `ToolCall { tool_use_id, tool, input }`, `ToolResult { tool_use_id, tool, output, is_error }`, `ArtifactWritten { artifact_id }`, `Terminated { reason, detail }`, `Error { message }`. `kind_str()` gives the database's `kind` column value.
  - `struct AgentStep { investigation_id: Uuid, seq: i64, phase: Phase, kind: StepKind, created_at: OffsetDateTime }`
  - `const STEP_PAYLOAD_VERSION: i32 = 1`

- [ ] **Step 1: Write the failing test**

`crates/agentops-core/src/step.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn phase_round_trips() {
        for p in [Phase::All, Phase::Chat, Phase::Triage, Phase::Rca, Phase::Mitigation] {
            assert_eq!(p.as_str().parse::<Phase>().unwrap(), p);
        }
    }

    #[test]
    fn investigation_order_is_triage_rca_mitigation() {
        assert_eq!(
            Phase::INVESTIGATION_ORDER,
            [Phase::Triage, Phase::Rca, Phase::Mitigation]
        );
    }

    #[test]
    fn tool_call_carries_tool_use_id() {
        let k = StepKind::ToolCall {
            tool_use_id: "toolu_abc".into(),
            tool: "prom__query".into(),
            input: serde_json::json!({"q": "up"}),
        };
        assert_eq!(k.kind_str(), "tool_call");
        assert_eq!(k.tool_use_id(), Some("toolu_abc"));
    }

    #[test]
    fn tool_result_pairs_with_same_id() {
        let r = StepKind::ToolResult {
            tool_use_id: "toolu_abc".into(),
            tool: "prom__query".into(),
            output: "1".into(),
            is_error: false,
        };
        assert_eq!(r.kind_str(), "tool_result");
        assert_eq!(r.tool_use_id(), Some("toolu_abc"));
    }

    #[test]
    fn non_tool_kinds_have_no_tool_use_id() {
        assert_eq!(StepKind::Text { text: "hi".into() }.tool_use_id(), None);
    }

    #[test]
    fn terminated_kind_carries_structured_reason() {
        let k = StepKind::Terminated {
            reason: TerminalReason::Refusal { category: Some("cyber".into()) },
            detail: None,
        };
        assert_eq!(k.kind_str(), "terminated");
    }

    /// Confirms every TerminalReason variant serializes. An internally-tagged enum
    /// cannot serialize a newtype variant wrapping a string, so without this test
    /// it would be discovered as a runtime panic in plan 2.
    #[test]
    fn every_terminal_reason_serializes() {
        let reasons = [
            TerminalReason::Refusal { category: Some("cyber".into()) },
            TerminalReason::Refusal { category: None },
            TerminalReason::ContextWindowExceeded,
            TerminalReason::MaxTokens,
            TerminalReason::TurnLimitExceeded,
            TerminalReason::WallClockExceeded,
            TerminalReason::ToolTimeout { tool: "prom__query".into() },
            TerminalReason::ShutdownRequested,
            TerminalReason::TaskPanicked,
            TerminalReason::UnknownStopReason { stop_reason: "future_variant".into() },
        ];
        for r in reasons {
            let step = AgentStep {
                investigation_id: Uuid::nil(),
                seq: 0,
                phase: Phase::All,
                kind: StepKind::Terminated { reason: r.clone(), detail: None },
                created_at: time::OffsetDateTime::UNIX_EPOCH,
            };
            // payload_json uses expect internally, so a non-serializable variant panics
            let payload = step.payload_json();
            let back = StepKind::from_payload_json(&payload).unwrap();
            assert_eq!(back, StepKind::Terminated { reason: r, detail: None });
        }
    }

    /// The payload carries a version field — matching the database's `payload ? 'v'` CHECK.
    #[test]
    fn payload_serialization_includes_version() {
        let step = AgentStep {
            investigation_id: Uuid::nil(),
            seq: 0,
            phase: Phase::Triage,
            kind: StepKind::Text { text: "hello".into() },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let payload = step.payload_json();
        assert_eq!(payload["v"], STEP_PAYLOAD_VERSION);
        assert_eq!(payload["text"], "hello");
    }

    #[test]
    fn payload_round_trips() {
        let kind = StepKind::ToolCall {
            tool_use_id: "toolu_1".into(),
            tool: "t".into(),
            input: serde_json::json!({"a": 1}),
        };
        let step = AgentStep {
            investigation_id: Uuid::nil(),
            seq: 3,
            phase: Phase::Rca,
            kind: kind.clone(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let payload = step.payload_json();
        assert_eq!(StepKind::from_payload_json(&payload).unwrap(), kind);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-core step 2>&1 | tail -20`
Expected: FAIL — `cannot find type Phase in this scope`

- [ ] **Step 3: Write the minimal implementation**

Above the test module in `crates/agentops-core/src/step.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// Schema version of the `agent_steps.payload` JSONB. Bump it when the shape changes.
pub const STEP_PAYLOAD_VERSION: i32 = 1;

/// Investigation phase. The same set as the scope axis of `instructions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    All,
    Chat,
    Triage,
    Rca,
    Mitigation,
}

impl Phase {
    /// The phases an investigation passes through in order. The fixed sequential sub-loop of design spec Section 5.3.
    pub const INVESTIGATION_ORDER: [Phase; 3] = [Phase::Triage, Phase::Rca, Phase::Mitigation];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Chat => "chat",
            Self::Triage => "triage",
            Self::Rca => "rca",
            Self::Mitigation => "mitigation",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown phase: {0}")]
pub struct ParsePhaseError(String);

impl FromStr for Phase {
    type Err = ParsePhaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "chat" => Ok(Self::Chat),
            "triage" => Ok(Self::Triage),
            "rca" => Ok(Self::Rca),
            "mitigation" => Ok(Self::Mitigation),
            other => Err(ParsePhaseError(other.to_owned())),
        }
    }
}

/// Why a phase or an investigation ended. Without structure, values like
/// `stop_details.category` end up crammed into a string (design spec, Section 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum TerminalReason {
    Refusal { category: Option<String> },
    ContextWindowExceeded,
    MaxTokens,
    TurnLimitExceeded,
    WallClockExceeded,
    ToolTimeout { tool: String },
    ShutdownRequested,
    TaskPanicked,
    /// **Must be a struct variant.** A newtype variant such as
    /// `UnknownStopReason(String)` cannot be serialized by an internally-tagged
    /// (`tag = "reason"`) enum — serde returns `cannot serialize tagged newtype
    /// variant ... containing a string` and `payload_json()`'s `.expect` panics.
    /// Plan 1's tests never take this path, but plan 2's agent loop does the moment
    /// an unknown `stop_reason` arrives — which is this variant's entire reason to exist.
    UnknownStopReason { stop_reason: String },
}

/// A durable event produced by the agent loop. Not produced per delta, but only
/// at semantic boundaries (design spec, Section 6.1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StepKind {
    Thinking {
        summary: String,
    },
    Text {
        text: String,
    },
    /// `tool_use_id` is the ID Anthropic issued. A single turn can carry several
    /// parallel tool calls, and without this there is no way to pair a call with its result.
    ToolCall {
        tool_use_id: String,
        tool: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        tool: String,
        output: String,
        is_error: bool,
    },
    ArtifactWritten {
        artifact_id: Uuid,
    },
    Terminated {
        reason: TerminalReason,
        detail: Option<String>,
    },
    Error {
        message: String,
    },
}

impl StepKind {
    /// The value of the database's `agent_steps.kind` column. The same set as the CHECK constraint.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Thinking { .. } => "thinking",
            Self::Text { .. } => "text",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::ArtifactWritten { .. } => "artifact",
            Self::Terminated { .. } => "terminated",
            Self::Error { .. } => "error",
        }
    }

    /// The key pairing a tool call with its result.
    pub fn tool_use_id(&self) -> Option<&str> {
        match self {
            Self::ToolCall { tool_use_id, .. } | Self::ToolResult { tool_use_id, .. } => {
                Some(tool_use_id)
            }
            _ => None,
        }
    }

    /// Restores from the JSONB payload.
    pub fn from_payload_json(v: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(v.clone())
    }
}

/// One row of an investigation's durable event log. `seq` increases monotonically within an investigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStep {
    pub investigation_id: Uuid,
    pub seq: i64,
    pub phase: Phase,
    pub kind: StepKind,
    pub created_at: OffsetDateTime,
}

impl AgentStep {
    /// The JSON stored in `agent_steps.payload`. Includes the version field.
    pub fn payload_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(&self.kind).expect("StepKind is always serializable");
        if let serde_json::Value::Object(map) = &mut v {
            map.insert("v".into(), serde_json::json!(STEP_PAYLOAD_VERSION));
        }
        v
    }
}
```

Add to `crates/agentops-core/src/lib.rs`:

```rust
pub mod step;

pub use step::{AgentStep, Phase, StepKind, TerminalReason, STEP_PAYLOAD_VERSION};
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-core 2>&1 | tail -15`
Expected: PASS — `14 passed` (Task 2's 5 plus this task's 9)

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-core/src/step.rs crates/agentops-core/src/lib.rs
git commit -m "feat(core): add agent step and terminal reason types"
```

---

### Task 4: Knowledge and chat types, and the error types

**Files:**
- Create: `crates/agentops-core/src/knowledge.rs`
- Create: `crates/agentops-core/src/chat.rs`
- Create: `crates/agentops-core/src/error.rs`
- Modify: `crates/agentops-core/src/lib.rs`

**Interfaces:**
- Consumes: `Phase` from Task 3
- Produces:
  - `struct Instruction { id: Uuid, phase: Phase, position: i32, title: String, body: String, enabled: bool, updated_at: OffsetDateTime }`
  - `struct Artifact { id: Uuid, investigation_id: Option<Uuid>, title: String, body: String, created_at: OffsetDateTime, updated_at: OffsetDateTime }`
  - `struct NewArtifact { title: String, body: String }` — the input passed to the terminal transaction
  - `enum ChatRole { User, Assistant }` — `as_str()`, `FromStr`
  - `struct ChatSession { id: Uuid, title: String, created_at: OffsetDateTime, updated_at: OffsetDateTime }`
  - `struct ChatMessage { session_id: Uuid, seq: i64, role: ChatRole, content: serde_json::Value, created_at: OffsetDateTime }`
  - `enum StoreError { NotFound, Conflict, Serialization(serde_json::Error), Backend(String) }`
  - `enum LlmError`, `enum ToolError`, `enum JobError`

- [ ] **Step 1: Write the failing test**

`crates/agentops-core/src/knowledge.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_sort_by_position_then_title() {
        let mut v = vec![
            ("b", 0),
            ("a", 1),
            ("a", 0),
        ];
        v.sort_by(|l, r| l.1.cmp(&r.1).then(l.0.cmp(r.0)));
        assert_eq!(v, vec![("a", 0), ("b", 0), ("a", 1)]);
    }

    #[test]
    fn new_artifact_has_no_id_until_stored() {
        let a = NewArtifact { title: "t".into(), body: "b".into() };
        assert_eq!(a.title, "t");
    }
}
```

`crates/agentops-core/src/chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_role_round_trips() {
        for r in [ChatRole::User, ChatRole::Assistant] {
            assert_eq!(r.as_str().parse::<ChatRole>().unwrap(), r);
        }
    }

    #[test]
    fn chat_role_rejects_system() {
        assert!("system".parse::<ChatRole>().is_err());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-core 2>&1 | tail -20`
Expected: FAIL — `cannot find type NewArtifact` / `cannot find type ChatRole`

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-core/src/knowledge.rs`:

```rust
use crate::step::Phase;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A phase-scoped instruction. `position` fixes the prompt assembly order —
/// without the ordering the system prompt's bytes differ per request and the
/// prompt cache misses 100% of the time (design spec, Section 8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instruction {
    pub id: Uuid,
    pub phase: Phase,
    pub position: i32,
    pub title: String,
    pub body: String,
    pub enabled: bool,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Uuid,
    pub investigation_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// An artifact not yet stored. The terminal transaction issues its ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    pub title: String,
    pub body: String,
}
```

`crates/agentops-core/src/chat.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown chat role: {0}")]
pub struct ParseRoleError(String);

impl FromStr for ChatRole {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => Err(ParseRoleError(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: Uuid,
    pub title: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub session_id: Uuid,
    pub seq: i64,
    pub role: ChatRole,
    pub content: serde_json::Value,
    pub created_at: OffsetDateTime,
}
```

`crates/agentops-core/src/error.rs`:

```rust
/// Storage-layer errors. Flattened to a string so the backend type
/// (`sqlx::Error`) never leaks into core, which knows no I/O crate (design spec, Section 4.1).
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    /// A conditional transition affected zero rows — another party already terminated it.
    #[error("conflicting state transition")]
    Conflict,
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("store backend error: {0}")]
    Backend(String),
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("provider returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed stream event: {0}")]
    MalformedEvent(String),
    #[error("stream idle timeout after {seconds}s")]
    IdleTimeout { seconds: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    /// The policy is `deny` — the default for a newly discovered tool (design spec, Section 9.1).
    #[error("tool denied by policy: {0}")]
    Denied(String),
    #[error("tool timed out after {seconds}s: {tool}")]
    Timeout { tool: String, seconds: u64 },
    #[error("tool transport error: {0}")]
    Transport(String),
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("job queue is shutting down")]
    ShuttingDown,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
}
```

Add to `crates/agentops-core/src/lib.rs`:

```rust
pub mod chat;
pub mod error;
pub mod knowledge;

pub use chat::{ChatMessage, ChatRole, ChatSession};
pub use error::{JobError, LlmError, StoreError, ToolError};
pub use knowledge::{Artifact, Instruction, NewArtifact};
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-core 2>&1 | tail -15`
Expected: PASS — `18 passed`

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-core/src
git commit -m "feat(core): add knowledge, chat, and error types"
```

---

### Task 5: Trait definitions and object-safety verification

**Purpose:** Prove **at compile time** that these can be held as `Arc<dyn Trait>`. This is the point where the second review asked about the object safety of returning a `BoxStream`.

**Files:**
- Create: `crates/agentops-core/src/traits.rs`
- Modify: `crates/agentops-core/src/lib.rs`

**Interfaces:**
- Consumes: every type from Tasks 2 through 4
- Produces:
  - `trait Store: Send + Sync` — every method implemented in this plan (see the code below)
  - `struct InvestigationPage { items: Vec<Investigation>, next_cursor: Option<(OffsetDateTime, Uuid)> }`
  - `struct ListFilter { status: Option<InvestigationStatus>, cursor: Option<(OffsetDateTime, Uuid)>, limit: i64 }`
  - `trait ToolRegistry`, `trait JobManager` — implemented in plans 2 and 3
  - the `type BoxStream<'a, T>` alias

- [ ] **Step 1: Write the failing test**

`crates/agentops-core/src/traits.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// If the trait were not object-safe, this function would not compile.
    /// Compilation itself is the test, not a runtime assertion.
    #[test]
    fn traits_are_object_safe() {
        fn assert_store(_: Arc<dyn Store>) {}
        fn assert_tools(_: Arc<dyn ToolRegistry>) {}
        fn assert_jobs(_: Arc<dyn JobManager>) {}
        let _ = assert_store;
        let _ = assert_tools;
        let _ = assert_jobs;
    }

    /// A value crossing an `.await` boundary must be `Send + 'static`.
    #[test]
    fn error_types_are_send_static() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<crate::error::StoreError>();
        assert_send_static::<crate::error::ToolError>();
        assert_send_static::<crate::error::JobError>();
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-core traits 2>&1 | tail -20`
Expected: FAIL — `cannot find trait Store in this scope`

- [ ] **Step 3: Write the minimal implementation**

Above the test module in `crates/agentops-core/src/traits.rs`:

```rust
use crate::chat::{ChatMessage, ChatRole, ChatSession};
use crate::error::{JobError, StoreError, ToolError};
use crate::investigation::{Investigation, InvestigationStatus};
use crate::knowledge::{Artifact, Instruction, NewArtifact};
use crate::step::{AgentStep, Phase, StepKind, TerminalReason};
use async_trait::async_trait;
use std::pin::Pin;
use time::OffsetDateTime;
use uuid::Uuid;

/// A stream alias carrying owned types only. Being `'static`, it can cross `.await`
/// boundaries, and being boxed it does not break trait object safety.
pub type BoxStream<'a, T> = Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'a>>;

/// A keyset pagination cursor. The `(queued_at, id)` tuple breaks ties on identical timestamps.
pub type Cursor = (OffsetDateTime, Uuid);

#[derive(Debug, Clone)]
pub struct ListFilter {
    pub status: Option<InvestigationStatus>,
    pub cursor: Option<Cursor>,
    pub limit: i64,
}

impl Default for ListFilter {
    fn default() -> Self {
        Self { status: None, cursor: None, limit: 50 }
    }
}

#[derive(Debug, Clone)]
pub struct InvestigationPage {
    pub items: Vec<Investigation>,
    pub next_cursor: Option<Cursor>,
}

#[async_trait]
pub trait Store: Send + Sync {
    // --- Investigations ---
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError>;
    async fn get_investigation(&self, id: Uuid) -> Result<Investigation, StoreError>;
    async fn list_investigations(
        &self,
        filter: &ListFilter,
    ) -> Result<InvestigationPage, StoreError>;

    /// `queued` → `running`. `Conflict` if it is already in another state.
    async fn mark_running(&self, id: Uuid) -> Result<(), StoreError>;

    /// Boot cleanup: every `running` becomes `failed`. Returns how many were cleaned.
    async fn fail_orphaned_running(&self, reason: &TerminalReason) -> Result<u64, StoreError>;

    /// IDs of `running` investigations whose `updated_at` is older than the threshold.
    async fn stale_running_ids(&self, older_than: OffsetDateTime) -> Result<Vec<Uuid>, StoreError>;

    /// Queue restoration at boot.
    async fn queued_ids(&self) -> Result<Vec<Uuid>, StoreError>;

    // --- Steps ---
    /// **`seq` is allocated by the database.** The crux is that the caller does not pass
    /// a `seq` — allowing it would let the watchdog and the live task each compute one and
    /// collide (spec Section 6.1.1). Returns the allocated `seq`.
    ///
    /// Also refreshes `investigations.updated_at` in the same transaction —
    /// the INV-4 watchdog uses that value to judge whether it has stalled.
    async fn append_step(
        &self,
        investigation_id: Uuid,
        phase: Phase,
        kind: &StepKind,
    ) -> Result<i64, StoreError>;

    async fn steps_after(&self, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError>;

    /// The `after` value the investigation detail page embeds in the stream URL (INV-2).
    async fn max_step_seq(&self, id: Uuid) -> Result<Option<i64>, StoreError>;

    // --- Instructions ---
    /// **Must use `ORDER BY position, title`.** Without the ordering the prompt cache breaks.
    async fn instructions_for(&self, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError>;
    async fn upsert_instruction(&self, ins: &Instruction) -> Result<(), StoreError>;
    async fn delete_instruction(&self, id: Uuid) -> Result<(), StoreError>;

    // --- Artifacts ---
    async fn get_artifact(&self, id: Uuid) -> Result<Artifact, StoreError>;
    async fn list_artifacts(&self, limit: i64) -> Result<Vec<Artifact>, StoreError>;

    // --- Termination (transactional) ---
    /// Saving the artifact, the `ArtifactWritten` step, and `running` → `completed` in
    /// one transaction. No partial success survives (design spec, Section 6.1).
    ///
    /// There is no `seq` parameter — the database allocates it inside the transaction.
    /// Letting the caller pass one would collide with the watchdog and lose the terminal step (spec Section 6.1.1).
    async fn complete_investigation(
        &self,
        id: Uuid,
        artifact: &NewArtifact,
    ) -> Result<Uuid, StoreError>;

    /// The `Terminated` step and `running` → `failed` in one transaction.
    async fn fail_investigation(
        &self,
        id: Uuid,
        reason: &TerminalReason,
    ) -> Result<(), StoreError>;

    // --- Chat ---
    async fn create_chat_session(&self, s: &ChatSession) -> Result<(), StoreError>;
    async fn list_chat_sessions(&self, limit: i64) -> Result<Vec<ChatSession>, StoreError>;
    async fn chat_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError>;

    /// The store allocates `seq` atomically. Chat has two writers, so computing
    /// `MAX(seq)+1` in the application races (design spec, Section 6.1.1).
    async fn append_chat_message(
        &self,
        session_id: Uuid,
        role: ChatRole,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError>;
}

/// The MCP tool registry. Implemented in plan 2.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    async fn list(&self) -> Result<Vec<ToolDef>, ToolError>;
    async fn call(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    /// The `{server}__{tool}` namespace. Serialization sorts by this name.
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

/// The investigation execution scheduler. Implemented in plan 3.
#[async_trait]
pub trait JobManager: Send + Sync {
    async fn enqueue(&self, investigation_id: Uuid) -> Result<(), JobError>;
}
```

Add to `crates/agentops-core/src/lib.rs`:

```rust
pub mod traits;

pub use traits::{
    BoxStream, Cursor, InvestigationPage, JobManager, ListFilter, Store, ToolDef, ToolOutput,
    ToolRegistry,
};
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-core 2>&1 | tail -15`
Expected: PASS — `20 passed`

- [ ] **Step 5: Verify `agentops-core` does not depend on an I/O crate**

Run:
```bash
cargo tree -p agentops-core --depth 1 | grep -E "sqlx|reqwest|tokio" && echo "VIOLATION" || echo "core is I/O-free: OK"
```
Expected: `core is I/O-free: OK`

- [ ] **Step 6: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-core/src/traits.rs crates/agentops-core/src/lib.rs
git commit -m "feat(core): define Store, ToolRegistry, and JobManager traits"
```

---

### Task 6: The Postgres migration

**Files:**
- Create: `migrations/0001_initial.sql`

**Interfaces:**
- Consumes: the string representations of the Task 2 through 4 types (the `as_str()` values must match the CHECK sets)
- Produces: the `investigations`, `agent_steps`, `instructions`, `artifacts`, `chat_sessions`, `chat_messages`, `mcp_servers`, and `tool_policies` tables, plus the `touch_updated_at()` trigger function

- [ ] **Step 1: Write the migration**

`migrations/0001_initial.sql` — transcribe the design document's Section 7 directly:

```sql
-- Investigations
CREATE TABLE investigations (
    id             UUID PRIMARY KEY,
    title          TEXT        NOT NULL,
    prompt         TEXT        NOT NULL,
    status         TEXT        NOT NULL
        CHECK (status IN ('queued','running','completed','failed')),
    triggered_by   TEXT        NOT NULL
        CHECK (triggered_by IN ('user','alarm')),
    trigger_source TEXT,
    queued_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The sole authority for agent_steps.seq (spec Section 6.1.1)
    next_step_seq  BIGINT      NOT NULL DEFAULT 0 CHECK (next_step_seq >= 0),
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
-- id must be DESC as well. A reverse btree scan flips every column uniformly, so a
-- (…, id ASC) index cannot produce (queued_at DESC, id DESC) in either direction and
-- Postgres adds a sort node.
CREATE INDEX ON investigations (status, queued_at DESC, id DESC);
CREATE INDEX ON investigations (queued_at DESC, id DESC);
CREATE INDEX ON investigations (updated_at) WHERE status = 'running';
CREATE INDEX ON investigations (queued_at) WHERE status = 'queued';

-- Agent steps
CREATE TABLE agent_steps (
    investigation_id UUID        NOT NULL REFERENCES investigations(id) ON DELETE CASCADE,
    seq              BIGINT      NOT NULL,
    phase            TEXT        NOT NULL
        CHECK (phase IN ('all','chat','triage','rca','mitigation')),
    kind             TEXT        NOT NULL
        CHECK (kind IN ('thinking','text','tool_call','tool_result',
                        'artifact','terminated','error')),
    payload          JSONB       NOT NULL CHECK (payload ? 'v'),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (investigation_id, seq),
    CONSTRAINT seq_non_negative CHECK (seq >= 0)
);

-- Instructions
CREATE TABLE instructions (
    id         UUID PRIMARY KEY,
    phase      TEXT        NOT NULL
        CHECK (phase IN ('all','chat','triage','rca','mitigation')),
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
    body             TEXT        NOT NULL,
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

-- MCP servers (secrets are never stored)
CREATE TABLE mcp_servers (
    id         UUID PRIMARY KEY,
    name       TEXT        NOT NULL UNIQUE,
    transport  TEXT        NOT NULL CHECK (transport IN ('stdio','http')),
    config     JSONB       NOT NULL,
    enabled    BOOLEAN     NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Tool policy. Deny-by-default is enforced by the application (no row means deny).
CREATE TABLE tool_policies (
    server_name TEXT NOT NULL REFERENCES mcp_servers(name) ON DELETE CASCADE,
    tool_name   TEXT NOT NULL,
    policy      TEXT NOT NULL CHECK (policy IN ('allow','deny')),
    mutating    BOOLEAN NOT NULL DEFAULT true,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_name, tool_name)
);

-- Automatic updated_at refresh
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

- [ ] **Step 2: Confirm the migration actually applies**

Run:
```bash
cargo install sqlx-cli --no-default-features --features rustls,postgres 2>&1 | tail -2
export DATABASE_URL=postgres://agentops:agentops@localhost:55433/agentops
sqlx database create 2>/dev/null; sqlx migrate run
```
Expected: `Applied 0001/migrate initial`

- [ ] **Step 3: Confirm the CHECK constraints actually reject**

Run:
```bash
psql "$DATABASE_URL" -c "INSERT INTO investigations (id,title,prompt,status,triggered_by) VALUES (gen_random_uuid(),'t','p','bogus','user');" 2>&1 | grep -q "violates check constraint" && echo "status CHECK: OK" || echo "status CHECK: FAILED"
psql "$DATABASE_URL" -c "INSERT INTO investigations (id,title,prompt,status,triggered_by,trigger_source) VALUES (gen_random_uuid(),'t','p','queued','user','x');" 2>&1 | grep -q "trigger_source_iff_alarm" && echo "trigger_source CHECK: OK" || echo "trigger_source CHECK: FAILED"
psql "$DATABASE_URL" -c "INSERT INTO agent_steps (investigation_id,seq,phase,kind,payload) VALUES (gen_random_uuid(),0,'triage','text','{}');" 2>&1 | grep -qE "check constraint|foreign key" && echo "payload v CHECK: OK" || echo "payload v CHECK: FAILED"
```
Expected: all three lines end with `: OK`

- [ ] **Step 4: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add migrations/0001_initial.sql
git commit -m "feat(store): add initial postgres migration"
```

---

### Task 7: The investigation store — create, read, conditional transition

> **`fail_orphaned_running` belongs to Task 8, not to this task.** That function has to insert a terminal step, so it needs the `steps` module; implementing it here would reference a function that does not yet exist and **fail to compile at this task's own verification step.** The first version did exactly that, and the Self-Review missed it.

**Files:**
- Create: `crates/agentops-store/src/investigations.rs`
- Modify: `crates/agentops-store/src/lib.rs`
- Test: `crates/agentops-store/tests/investigations.rs`

**Interfaces:**
- Consumes: the `Store` trait from Task 5, `Investigation` from Task 2, the schema from Task 6
- Produces:
  - `struct PgStore { pool: sqlx::PgPool }` — `PgStore::new(pool)`, `PgStore::pool()`
  - `pub(crate) fn backend(e: sqlx::Error) -> StoreError`
  - `pub(crate) fn row_to_investigation(row: &PgRow) -> Result<Investigation, StoreError>`
  - the free functions `create`, `get`, `list`, `mark_running`, `stale_running_ids`, `queued_ids` — `impl Store for PgStore` in `lib.rs` delegates to them
  - `fail_orphaned_running` is created by **Task 8**

- [ ] **Step 1: Write the failing test**

`crates/agentops-store/tests/investigations.rs`:

```rust
use agentops_core::{Investigation, InvestigationStatus, ListFilter, Store, TriggeredBy};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

fn queued(title: &str) -> Investigation {
    let now = OffsetDateTime::now_utc();
    Investigation {
        id: Uuid::new_v4(),
        title: title.into(),
        prompt: "why is latency high".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_then_get_round_trips(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let inv = queued("first");
    store.create_investigation(&inv).await.unwrap();

    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(got.id, inv.id);
    assert_eq!(got.status, InvestigationStatus::Queued);
    assert_eq!(got.triggered_by, TriggeredBy::User);
    assert!(got.started_at.is_none());
    assert!(got.finished_at.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn alarm_trigger_preserves_source(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut inv = queued("alarm");
    inv.triggered_by = TriggeredBy::Alarm { source: "cpu-high".into() };
    store.create_investigation(&inv).await.unwrap();

    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(got.triggered_by, TriggeredBy::Alarm { source: "cpu-high".into() });
}

/// INV-4: the terminal transition is conditional. `queued` → `running` succeeds only once.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_4_mark_running_is_conditional(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let inv = queued("once");
    store.create_investigation(&inv).await.unwrap();

    store.mark_running(inv.id).await.unwrap();
    let got = store.get_investigation(inv.id).await.unwrap();
    assert_eq!(got.status, InvestigationStatus::Running);
    assert!(got.started_at.is_some(), "started_at must be set on running");

    // The second call is a Conflict
    let err = store.mark_running(inv.id).await.unwrap_err();
    assert!(
        matches!(err, agentops_core::StoreError::Conflict),
        "second mark_running must conflict, got {err:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_ids_restores_the_queue(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let a = queued("a");
    let b = queued("b");
    store.create_investigation(&a).await.unwrap();
    store.create_investigation(&b).await.unwrap();
    store.mark_running(a.id).await.unwrap();

    let ids = store.queued_ids().await.unwrap();
    assert_eq!(ids, vec![b.id]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_status_and_pages_by_cursor(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    for i in 0..5 {
        let mut inv = queued(&format!("inv-{i}"));
        // Give each a different queued_at so the ordering is deterministic
        inv.queued_at = OffsetDateTime::now_utc() - time::Duration::seconds(i);
        store.create_investigation(&inv).await.unwrap();
    }

    let page1 = store
        .list_investigations(&ListFilter { limit: 2, ..Default::default() })
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some());

    let page2 = store
        .list_investigations(&ListFilter {
            limit: 2,
            cursor: page1.next_cursor,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page2.items.len(), 2);
    let ids1: Vec<_> = page1.items.iter().map(|i| i.id).collect();
    let ids2: Vec<_> = page2.items.iter().map(|i| i.id).collect();
    assert!(ids1.iter().all(|id| !ids2.contains(id)), "pages must not overlap");

    let only_queued = store
        .list_investigations(&ListFilter {
            status: Some(InvestigationStatus::Running),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(only_queued.items.is_empty());
}

/// Whether `id` breaks the tie on an identical `queued_at`. The previous test gives every
/// row a distinct timestamp and never takes this path.
#[sqlx::test(migrations = "../../migrations")]
async fn list_breaks_ties_on_id_at_identical_timestamps(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let same = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for i in 0..4 {
        let mut inv = queued(&format!("tie-{i}"));
        inv.queued_at = same;
        ids.push(inv.id);
        store.create_investigation(&inv).await.unwrap();
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list_investigations(&ListFilter { limit: 2, cursor, ..Default::default() })
            .await
            .unwrap();
        seen.extend(page.items.iter().map(|i| i.id));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    ids.sort_unstable();
    let mut got = seen.clone();
    got.sort_unstable();
    assert_eq!(got, ids, "every row must appear exactly once across pages");
    assert_eq!(seen.len(), 4, "no row duplicated or skipped at a tie boundary");
}

/// A `limit` of zero or below, or an oversized one, neither panics nor produces a SQL error.
#[sqlx::test(migrations = "../../migrations")]
async fn list_tolerates_degenerate_limits(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    store.create_investigation(&queued("one")).await.unwrap();

    for limit in [0_i64, -1, i64::MAX] {
        let page = store
            .list_investigations(&ListFilter { limit, ..Default::default() })
            .await
            .unwrap_or_else(|e| panic!("limit {limit} must not error, got {e:?}"));
        assert!(page.items.len() <= 1);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_missing_investigation_is_not_found(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let err = store.get_investigation(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, agentops_core::StoreError::NotFound));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
export DATABASE_URL=postgres://agentops:agentops@localhost:55433/agentops
cargo test -p agentops-store --test investigations 2>&1 | tail -20
```
Expected: FAIL — `cannot find struct PgStore` / `unresolved import agentops_store::PgStore`

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-store/src/investigations.rs`:

```rust
use agentops_core::{
    Cursor, Investigation, InvestigationPage, InvestigationStatus, ListFilter, StoreError,
    TriggeredBy,
};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// Database row to domain type. A string parse failure means the schema and the code
/// disagree, so it is raised as a `Backend` error.
///
/// It is `pub(crate)` because Task 8's `fail_orphaned_running` uses it too.
pub(crate) fn row_to_investigation(
    row: &sqlx::postgres::PgRow,
) -> Result<Investigation, StoreError> {
    let status: String = row.try_get("status").map_err(crate::backend)?;
    let triggered_by: String = row.try_get("triggered_by").map_err(crate::backend)?;
    let trigger_source: Option<String> = row.try_get("trigger_source").map_err(crate::backend)?;

    let triggered_by = match triggered_by.as_str() {
        "user" => TriggeredBy::User,
        "alarm" => TriggeredBy::Alarm {
            source: trigger_source.ok_or_else(|| {
                StoreError::Backend("alarm row without trigger_source".into())
            })?,
        },
        other => return Err(StoreError::Backend(format!("unknown triggered_by: {other}"))),
    };

    Ok(Investigation {
        id: row.try_get("id").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        prompt: row.try_get("prompt").map_err(crate::backend)?,
        status: status
            .parse::<InvestigationStatus>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        triggered_by,
        queued_at: row.try_get("queued_at").map_err(crate::backend)?,
        started_at: row.try_get("started_at").map_err(crate::backend)?,
        finished_at: row.try_get("finished_at").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

pub async fn create(pool: &PgPool, inv: &Investigation) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO investigations
           (id, title, prompt, status, triggered_by, trigger_source,
            queued_at, started_at, finished_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(inv.id)
    .bind(&inv.title)
    .bind(&inv.prompt)
    .bind(inv.status.as_str())
    .bind(inv.triggered_by.kind_str())
    .bind(inv.triggered_by.source())
    .bind(inv.queued_at)
    .bind(inv.started_at)
    .bind(inv.finished_at)
    .bind(inv.updated_at)
    .execute(pool)
    .await
    .map_err(crate::backend)?;
    Ok(())
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Investigation, StoreError> {
    let row = sqlx::query("SELECT * FROM investigations WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(crate::backend)?
        .ok_or(StoreError::NotFound)?;
    row_to_investigation(&row)
}

/// Keyset pagination. Ordering by `(queued_at, id)` breaks ties on identical timestamps.
pub async fn list(pool: &PgPool, f: &ListFilter) -> Result<InvestigationPage, StoreError> {
    // Clamp so a limit of zero or below, or an oversized one, becomes neither a SQL error nor an overflow.
    let limit = f.limit.clamp(1, 500);
    // Read limit + 1 to determine whether a next page exists.
    let fetch = limit + 1;
    let rows = match (&f.status, &f.cursor) {
        (Some(s), Some((ts, id))) => {
            sqlx::query(
                "SELECT * FROM investigations
                 WHERE status = $1 AND (queued_at, id) < ($2, $3)
                 ORDER BY queued_at DESC, id DESC LIMIT $4",
            )
            .bind(s.as_str())
            .bind(ts)
            .bind(id)
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (Some(s), None) => {
            sqlx::query(
                "SELECT * FROM investigations WHERE status = $1
                 ORDER BY queued_at DESC, id DESC LIMIT $2",
            )
            .bind(s.as_str())
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (None, Some((ts, id))) => {
            sqlx::query(
                "SELECT * FROM investigations WHERE (queued_at, id) < ($1, $2)
                 ORDER BY queued_at DESC, id DESC LIMIT $3",
            )
            .bind(ts)
            .bind(id)
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
        (None, None) => {
            sqlx::query(
                "SELECT * FROM investigations ORDER BY queued_at DESC, id DESC LIMIT $1",
            )
            .bind(fetch)
            .fetch_all(pool)
            .await
        }
    }
    .map_err(crate::backend)?;

    let mut items: Vec<Investigation> = rows
        .iter()
        .map(row_to_investigation)
        .collect::<Result<_, _>>()?;

    let next_cursor: Option<Cursor> = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|i| (i.queued_at, i.id))
    } else {
        None
    };

    Ok(InvestigationPage { items, next_cursor })
}

/// `queued` → `running`. Being conditional, a second call returns `Conflict`.
pub async fn mark_running(pool: &PgPool, id: Uuid) -> Result<(), StoreError> {
    let res = sqlx::query(
        "UPDATE investigations SET status = 'running', started_at = now()
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(crate::backend)?;

    if res.rows_affected() == 0 {
        return Err(StoreError::Conflict);
    }
    Ok(())
}

pub async fn stale_running_ids(
    pool: &PgPool,
    older_than: OffsetDateTime,
) -> Result<Vec<Uuid>, StoreError> {
    sqlx::query_scalar(
        "SELECT id FROM investigations WHERE status = 'running' AND updated_at < $1",
    )
    .bind(older_than)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)
}

pub async fn queued_ids(pool: &PgPool) -> Result<Vec<Uuid>, StoreError> {
    sqlx::query_scalar(
        "SELECT id FROM investigations WHERE status = 'queued' ORDER BY queued_at",
    )
    .fetch_all(pool)
    .await
    .map_err(crate::backend)
}
```

- [ ] **Step 4: Write `PgStore` and the delegating implementation in `lib.rs`**

`crates/agentops-store/src/lib.rs`:

```rust
//! The Postgres implementation of the `Store` trait.

use agentops_core::{
    AgentStep, Artifact, ChatMessage, ChatRole, ChatSession, Investigation, InvestigationPage,
    Instruction, ListFilter, NewArtifact, Phase, StepKind, Store, StoreError, TerminalReason,
};
use async_trait::async_trait;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

mod artifacts;
mod chat;
mod instructions;
mod investigations;
mod steps;

/// `sqlx::Error` to `StoreError`. The core does not know sqlx, so it is flattened here.
pub(crate) fn backend(e: sqlx::Error) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl Store for PgStore {
    async fn create_investigation(&self, inv: &Investigation) -> Result<(), StoreError> {
        investigations::create(&self.pool, inv).await
    }

    async fn get_investigation(&self, id: Uuid) -> Result<Investigation, StoreError> {
        investigations::get(&self.pool, id).await
    }

    async fn list_investigations(&self, f: &ListFilter) -> Result<InvestigationPage, StoreError> {
        investigations::list(&self.pool, f).await
    }

    async fn mark_running(&self, id: Uuid) -> Result<(), StoreError> {
        investigations::mark_running(&self.pool, id).await
    }

    async fn fail_orphaned_running(&self, reason: &TerminalReason) -> Result<u64, StoreError> {
        // Lives in the steps module because it inserts a terminal step (Task 8)
        steps::fail_orphaned_running(&self.pool, reason).await
    }

    async fn stale_running_ids(&self, older_than: OffsetDateTime) -> Result<Vec<Uuid>, StoreError> {
        investigations::stale_running_ids(&self.pool, older_than).await
    }

    async fn queued_ids(&self) -> Result<Vec<Uuid>, StoreError> {
        investigations::queued_ids(&self.pool).await
    }

    async fn append_step(
        &self,
        investigation_id: Uuid,
        phase: Phase,
        kind: &StepKind,
    ) -> Result<i64, StoreError> {
        steps::append(&self.pool, investigation_id, phase, kind).await
    }

    async fn steps_after(&self, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError> {
        steps::after(&self.pool, id, after_seq).await
    }

    async fn max_step_seq(&self, id: Uuid) -> Result<Option<i64>, StoreError> {
        steps::max_seq(&self.pool, id).await
    }

    async fn instructions_for(&self, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
        instructions::for_phases(&self.pool, phases).await
    }

    async fn upsert_instruction(&self, ins: &Instruction) -> Result<(), StoreError> {
        instructions::upsert(&self.pool, ins).await
    }

    async fn delete_instruction(&self, id: Uuid) -> Result<(), StoreError> {
        instructions::delete(&self.pool, id).await
    }

    async fn get_artifact(&self, id: Uuid) -> Result<Artifact, StoreError> {
        artifacts::get(&self.pool, id).await
    }

    async fn list_artifacts(&self, limit: i64) -> Result<Vec<Artifact>, StoreError> {
        artifacts::list(&self.pool, limit).await
    }

    async fn complete_investigation(
        &self,
        id: Uuid,
        artifact: &NewArtifact,
    ) -> Result<Uuid, StoreError> {
        artifacts::complete_investigation(&self.pool, id, artifact).await
    }

    async fn fail_investigation(
        &self,
        id: Uuid,
        reason: &TerminalReason,
    ) -> Result<(), StoreError> {
        artifacts::fail_investigation(&self.pool, id, reason).await
    }

    async fn create_chat_session(&self, s: &ChatSession) -> Result<(), StoreError> {
        chat::create_session(&self.pool, s).await
    }

    async fn list_chat_sessions(&self, limit: i64) -> Result<Vec<ChatSession>, StoreError> {
        chat::list_sessions(&self.pool, limit).await
    }

    async fn chat_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
        chat::messages(&self.pool, session_id).await
    }

    async fn append_chat_message(
        &self,
        session_id: Uuid,
        role: ChatRole,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError> {
        chat::append_message(&self.pool, session_id, role, content).await
    }
}
```

- [ ] **Step 5: Create four stub modules — they must include the signatures**

Because `lib.rs` calls functions from these four modules, **empty files will not compile.** Put the final signature in each file with only the body left as `todo!()`. Tasks 8 through 11 fill in the bodies.

`crates/agentops-store/src/steps.rs`:

```rust
//! Task 8 fills in the body.
#![allow(unused_variables)]

use agentops_core::{AgentStep, Phase, StepKind, StoreError, TerminalReason};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn append(
    pool: &PgPool,
    investigation_id: Uuid,
    phase: Phase,
    kind: &StepKind,
) -> Result<i64, StoreError> {
    todo!()
}

pub async fn after(pool: &PgPool, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError> {
    todo!()
}

pub async fn max_seq(pool: &PgPool, id: Uuid) -> Result<Option<i64>, StoreError> {
    todo!()
}

pub async fn fail_orphaned_running(
    pool: &PgPool,
    reason: &TerminalReason,
) -> Result<u64, StoreError> {
    todo!()
}
```

`crates/agentops-store/src/instructions.rs`:

```rust
//! Task 9 fills in the body.
#![allow(unused_variables)]

use agentops_core::{Instruction, Phase, StoreError};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn for_phases(pool: &PgPool, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
    todo!()
}

pub async fn upsert(pool: &PgPool, ins: &Instruction) -> Result<(), StoreError> {
    todo!()
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), StoreError> {
    todo!()
}
```

`crates/agentops-store/src/artifacts.rs`:

```rust
//! Task 10 fills in the body.
#![allow(unused_variables)]

use agentops_core::{Artifact, NewArtifact, StoreError, TerminalReason};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Artifact, StoreError> {
    todo!()
}

pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<Artifact>, StoreError> {
    todo!()
}

pub async fn complete_investigation(
    pool: &PgPool,
    id: Uuid,
    artifact: &NewArtifact,
) -> Result<Uuid, StoreError> {
    todo!()
}

pub async fn fail_investigation(
    pool: &PgPool,
    id: Uuid,
    reason: &TerminalReason,
) -> Result<(), StoreError> {
    todo!()
}
```

`crates/agentops-store/src/chat.rs`:

```rust
//! Task 11 fills in the body.
#![allow(unused_variables)]

use agentops_core::{ChatMessage, ChatRole, ChatSession, StoreError};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn create_session(pool: &PgPool, s: &ChatSession) -> Result<(), StoreError> {
    todo!()
}

pub async fn list_sessions(pool: &PgPool, limit: i64) -> Result<Vec<ChatSession>, StoreError> {
    todo!()
}

pub async fn messages(pool: &PgPool, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
    todo!()
}

pub async fn append_message(
    pool: &PgPool,
    session_id: Uuid,
    role: ChatRole,
    content: &serde_json::Value,
) -> Result<i64, StoreError> {
    todo!()
}
```

`#![allow(unused_variables)]` suppresses roughly 21 unused-variable warnings produced by the `todo!()` bodies. **Remove this attribute from all four files once Task 11 is done** — Task 12's clippy gate catches it along with any remaining `todo!()`.

- [ ] **Step 6: Run the investigation tests to verify they pass**

Run:
```bash
export DATABASE_URL=postgres://agentops:agentops@localhost:55433/agentops
cargo test -p agentops-store --test investigations 2>&1 | tail -15
```
Expected: PASS — `8 passed`

- [ ] **Step 7: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-store
git commit -m "feat(store): implement investigation create, list, and conditional transitions"
```

---

### Task 8: The step store — database-side `seq` allocation and replay

**Purpose:** The storage layer for INV-2 (replay) and the spec's Section 6.1.1 (`seq` allocation). **The application never computes `seq`** — with two computation paths (the live task and the watchdog), a collision silently loses the terminal step.

**Files:**
- Modify: `crates/agentops-store/src/steps.rs`
- Test: `crates/agentops-store/tests/steps.rs`

**Interfaces:**
- Consumes: `AgentStep`, `StepKind`, and `TerminalReason` from Task 3; `PgStore`, `backend`, and `row_to_investigation` from Task 7
- Produces:
  - `pub async fn append(pool: &PgPool, investigation_id: Uuid, phase: Phase, kind: &StepKind) -> Result<i64, StoreError>` — returns the allocated `seq`
  - `pub async fn after(pool: &PgPool, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError>`
  - `pub async fn max_seq(pool: &PgPool, id: Uuid) -> Result<Option<i64>, StoreError>`
  - `pub async fn fail_orphaned_running(pool: &PgPool, reason: &TerminalReason) -> Result<u64, StoreError>`
  - `pub(crate) async fn allocate_seq(conn: &mut PgConnection, id: Uuid) -> Result<i64, StoreError>` — Task 10's terminal transaction uses this too
  - `pub(crate) async fn insert_step_strict(conn: &mut PgConnection, step: &AgentStep) -> Result<(), StoreError>` — no `ON CONFLICT`; for terminal steps

- [ ] **Step 1: Write the failing test**

`crates/agentops-store/tests/steps.rs`:

```rust
use agentops_core::{
    AgentStep, Investigation, InvestigationStatus, Phase, Store, StepKind, TriggeredBy,
    STEP_PAYLOAD_VERSION,
};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

async fn running_investigation(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    store.mark_running(inv.id).await.unwrap();
    inv.id
}

/// Section 6.1.1 — the database allocates `seq`, so the caller does not manage 0, 1, 2, and so on.
#[sqlx::test(migrations = "../../migrations")]
async fn seq_is_allocated_by_the_database(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;

    let a = store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "a".into() })
        .await
        .unwrap();
    let b = store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "b".into() })
        .await
        .unwrap();
    assert_eq!((a, b), (0, 1), "the store, not the caller, assigns seq");
}

/// TEST-4 — replay by the after parameter. A new connection receives only what follows after.
#[sqlx::test(migrations = "../../migrations")]
async fn test_4_steps_after_returns_only_later_seqs(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    for n in 0..5 {
        store
            .append_step(id, Phase::Triage, &StepKind::Text { text: format!("t{n}") })
            .await
            .unwrap();
    }

    let from_start = store.steps_after(id, -1).await.unwrap();
    assert_eq!(from_start.len(), 5);
    assert_eq!(from_start[0].seq, 0, "must be ordered ascending");

    let after_2 = store.steps_after(id, 2).await.unwrap();
    assert_eq!(after_2.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![3, 4]);

    let after_end = store.steps_after(id, 99).await.unwrap();
    assert!(after_end.is_empty(), "out-of-range after yields empty, not an error");
}

/// TEST-11 — the tool_use_id of parallel tool calls survives the round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn test_11_tool_use_ids_survive_round_trip(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;

    for (i, tid) in ["toolu_a", "toolu_b", "toolu_c"].iter().enumerate() {
        store
            .append_step(
                id,
                Phase::Triage,
                &StepKind::ToolCall {
                    tool_use_id: (*tid).into(),
                    tool: "prom__query".into(),
                    input: serde_json::json!({"i": i}),
                },
            )
            .await
            .unwrap();
    }

    let steps = store.steps_after(id, -1).await.unwrap();
    let ids: Vec<_> = steps.iter().filter_map(|s| s.kind.tool_use_id()).collect();
    assert_eq!(ids, vec!["toolu_a", "toolu_b", "toolu_c"]);
}

/// Concurrent appends never receive the same seq. Without database allocation this fails.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_appends_get_distinct_seqs(pool: sqlx::PgPool) {
    let store = std::sync::Arc::new(PgStore::new(pool));
    let id = running_investigation(&store).await;

    let mut handles = Vec::new();
    for n in 0..20 {
        let store = std::sync::Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store
                .append_step(id, Phase::Triage, &StepKind::Text { text: format!("{n}") })
                .await
        }));
    }
    let mut seqs = Vec::new();
    for h in handles {
        seqs.push(h.await.unwrap().expect("concurrent append must not fail"));
    }
    seqs.sort_unstable();
    assert_eq!(seqs, (0..20).collect::<Vec<i64>>(), "no gaps, no duplicates");
}

#[sqlx::test(migrations = "../../migrations")]
async fn payload_carries_version_field(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
        .await
        .unwrap();

    let v: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM agent_steps WHERE investigation_id = $1 AND seq = 0",
    )
    .bind(id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(v["v"], STEP_PAYLOAD_VERSION);
}

/// INV-4 — the updated_at the watchdog reads must advance on a step append too.
/// Without that, an in-flight investigation is misjudged as stalled.
#[sqlx::test(migrations = "../../migrations")]
async fn inv_4_append_step_touches_investigation_updated_at(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    let before = store.get_investigation(id).await.unwrap().updated_at;

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
        .await
        .unwrap();

    let after = store.get_investigation(id).await.unwrap().updated_at;
    assert!(
        after > before,
        "append_step must bump investigations.updated_at (before={before}, after={after})"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn max_seq_reports_none_then_highest(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running_investigation(&store).await;
    assert_eq!(store.max_step_seq(id).await.unwrap(), None);

    for _ in 0..3 {
        store
            .append_step(id, Phase::Triage, &StepKind::Text { text: "x".into() })
            .await
            .unwrap();
    }
    assert_eq!(store.max_step_seq(id).await.unwrap(), Some(2));
}

/// INV-4 and TEST-20 — boot cleanup touches only the ID set it locked.
#[sqlx::test(migrations = "../../migrations")]
async fn test_20_boot_cleanup_only_touches_locked_ids(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let a = queued("a");
    let b = queued("b");
    store.create_investigation(&a).await.unwrap();
    store.create_investigation(&b).await.unwrap();
    store.mark_running(a.id).await.unwrap();

    let n = store
        .fail_orphaned_running(&TerminalReason::ShutdownRequested)
        .await
        .unwrap();
    assert_eq!(n, 1, "only the running investigation is failed");

    let ga = store.get_investigation(a.id).await.unwrap();
    assert_eq!(ga.status, InvestigationStatus::Failed);
    assert!(ga.finished_at.is_some(), "finished_at must be set when failing");
    assert_eq!(
        store.get_investigation(b.id).await.unwrap().status,
        InvestigationStatus::Queued,
        "queued investigations survive boot cleanup"
    );

    // TEST-19 — a terminated investigation has exactly one terminal step
    let steps = store.steps_after(a.id, -1).await.unwrap();
    let terminated: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s.kind, StepKind::Terminated { .. }))
        .collect();
    assert_eq!(terminated.len(), 1, "exactly one Terminated step must land");
    assert!(
        store.steps_after(b.id, -1).await.unwrap().is_empty(),
        "an untouched investigation gets no step"
    );
}

/// TEST-19 — cleaning several investigations at once still gives each exactly one terminal step.
#[sqlx::test(migrations = "../../migrations")]
async fn test_19_every_cleaned_investigation_gets_exactly_one_terminal_step(
    pool: sqlx::PgPool,
) {
    let store = PgStore::new(pool);
    let mut ids = Vec::new();
    for n in 0..5 {
        let inv = queued(&format!("r{n}"));
        store.create_investigation(&inv).await.unwrap();
        store.mark_running(inv.id).await.unwrap();
        // Mix in ordinary steps before cleanup so seq is not 0
        store
            .append_step(inv.id, Phase::Triage, &StepKind::Text { text: "work".into() })
            .await
            .unwrap();
        ids.push(inv.id);
    }

    let n = store
        .fail_orphaned_running(&TerminalReason::TaskPanicked)
        .await
        .unwrap();
    assert_eq!(n, 5);

    for id in ids {
        let steps = store.steps_after(id, -1).await.unwrap();
        let terminated = steps
            .iter()
            .filter(|s| matches!(s.kind, StepKind::Terminated { .. }))
            .count();
        assert_eq!(terminated, 1, "investigation {id} must have exactly one Terminated step");
        assert_eq!(
            store.get_investigation(id).await.unwrap().status,
            InvestigationStatus::Failed
        );
    }
}
```

Set the `use` statements at the top of the test file to:

```rust
use agentops_core::{
    Investigation, InvestigationStatus, Phase, Store, StepKind, TerminalReason, TriggeredBy,
    STEP_PAYLOAD_VERSION,
};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

/// The same shape as the helper in the Task 7 tests. Duplicated per file — not large
/// enough to justify a shared test-utility crate.
fn queued(title: &str) -> Investigation {
    let now = OffsetDateTime::now_utc();
    Investigation {
        id: Uuid::new_v4(),
        title: title.into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-store --test steps 2>&1 | tail -20`
Expected: FAIL — `not yet implemented` (a panic from `todo!()`)

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-store/src/steps.rs`:

```rust
use agentops_core::{AgentStep, Phase, StepKind, StoreError, TerminalReason};
use sqlx::{PgConnection, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

fn row_to_step(row: &sqlx::postgres::PgRow) -> Result<AgentStep, StoreError> {
    let phase: String = row.try_get("phase").map_err(crate::backend)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(crate::backend)?;
    Ok(AgentStep {
        investigation_id: row.try_get("investigation_id").map_err(crate::backend)?,
        seq: row.try_get("seq").map_err(crate::backend)?,
        phase: phase
            .parse::<Phase>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        kind: StepKind::from_payload_json(&payload)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
    })
}

/// **The single point of `seq` allocation.** Allocation and the `updated_at` refresh the
/// watchdog needs happen in one statement — there is no separate UPDATE to forget, and the
/// row lock serializes appends per investigation.
pub(crate) async fn allocate_seq(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<i64, StoreError> {
    sqlx::query_scalar(
        "UPDATE investigations
            SET next_step_seq = next_step_seq + 1, updated_at = now()
          WHERE id = $1
        RETURNING next_step_seq - 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::backend)?
    .ok_or(StoreError::NotFound)
}

/// An ordinary step append. The database allocates `seq`, so the caller does not pass one.
pub async fn append(
    pool: &PgPool,
    investigation_id: Uuid,
    phase: Phase,
    kind: &StepKind,
) -> Result<i64, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;
    let seq = allocate_seq(&mut tx, investigation_id).await?;
    let step = AgentStep {
        investigation_id,
        seq,
        phase,
        kind: kind.clone(),
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;
    tx.commit().await.map_err(crate::backend)?;
    Ok(seq)
}

/// **Uses no `ON CONFLICT`.** Because the database allocates `seq`, a conflict cannot
/// occur, and if one does it is a bug — so it must be raised as an error rather than
/// swallowed silently. Terminal steps (Task 10) use this function too.
pub(crate) async fn insert_step_strict(
    conn: &mut PgConnection,
    step: &AgentStep,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO agent_steps (investigation_id, seq, phase, kind, payload, created_at)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(step.investigation_id)
    .bind(step.seq)
    .bind(step.phase.as_str())
    .bind(step.kind.kind_str())
    .bind(step.payload_json())
    .bind(step.created_at)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        // A PK violation means seq allocation was bypassed — do not pass over it silently
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return StoreError::Conflict;
            }
        }
        crate::backend(e)
    })?;
    Ok(())
}

/// Orphan cleanup at startup. **The UPDATE is confined to the ID set selected.**
///
/// Under READ COMMITTED a fresh snapshot is taken per statement, so an investigation that
/// `mark_running` moved to `running` after the `SELECT ... FOR UPDATE` is not among the
/// locked rows. An unconditional UPDATE with `WHERE status = 'running'` would fail that
/// investigation without a `Terminated` step.
pub async fn fail_orphaned_running(
    pool: &PgPool,
    reason: &TerminalReason,
) -> Result<u64, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM investigations WHERE status = 'running' FOR UPDATE",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(crate::backend)?;

    for id in &ids {
        let seq = allocate_seq(&mut tx, *id).await?;
        let step = AgentStep {
            investigation_id: *id,
            seq,
            phase: Phase::All,
            kind: StepKind::Terminated {
                reason: reason.clone(),
                detail: None,
            },
            created_at: OffsetDateTime::now_utc(),
        };
        insert_step_strict(&mut tx, &step).await?;
    }

    let updated: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'failed', finished_at = now()
          WHERE id = ANY($1) AND status = 'running'
        RETURNING id",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(crate::backend)?;

    // A mismatch between the locked set and the updated set means the lock logic broke
    if updated.len() != ids.len() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::Backend(format!(
            "boot cleanup locked {} rows but updated {}",
            ids.len(),
            updated.len()
        )));
    }

    tx.commit().await.map_err(crate::backend)?;
    Ok(updated.len() as u64)
}

pub async fn after(pool: &PgPool, id: Uuid, after_seq: i64) -> Result<Vec<AgentStep>, StoreError> {
    let rows = sqlx::query(
        "SELECT * FROM agent_steps WHERE investigation_id = $1 AND seq > $2 ORDER BY seq",
    )
    .bind(id)
    .bind(after_seq)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)?;
    rows.iter().map(row_to_step).collect()
}

pub async fn max_seq(pool: &PgPool, id: Uuid) -> Result<Option<i64>, StoreError> {
    sqlx::query_scalar("SELECT MAX(seq) FROM agent_steps WHERE investigation_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(crate::backend)
}

```

> **Do not create a helper that computes the next `seq` with `MAX(seq)+1`.** The first version had `next_seq_for`, and that was the cause of the P0 — when a path reading `MAX(seq)` coexists with a path using an in-memory counter, two writers reach the same value. `allocate_seq` is the only thing that produces a `seq`.

Set the `use` statements at the top of `crates/agentops-store/src/steps.rs` to the following, and remove `#![allow(unused_variables)]`:

```rust
use agentops_core::{AgentStep, Phase, StepKind, StoreError, TerminalReason};
use sqlx::{PgConnection, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-store --test steps 2>&1 | tail -15`
Expected: PASS — `9 passed`

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-store
git commit -m "feat(store): allocate step seq in the database, not the caller"
```

---

### Task 9: The instruction store — deterministic ordering

**Purpose:** Fix the read order so the prompt cache does not break. The spec's Section 8.3 requires it, but the first version had no ordering and the review flagged it.

**Files:**
- Modify: `crates/agentops-store/src/instructions.rs`
- Test: `crates/agentops-store/tests/instructions.rs`

**Interfaces:**
- Consumes: `Instruction` from Task 4, `Phase` from Task 3
- Produces:
  - `pub async fn for_phases(pool: &PgPool, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError>`
  - `pub async fn upsert(pool: &PgPool, ins: &Instruction) -> Result<(), StoreError>`
  - `pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), StoreError>`

- [ ] **Step 1: Write the failing test**

`crates/agentops-store/tests/instructions.rs`:

```rust
use agentops_core::{Instruction, Phase, Store};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

fn ins(phase: Phase, position: i32, title: &str) -> Instruction {
    Instruction {
        id: Uuid::new_v4(),
        phase,
        position,
        title: title.into(),
        body: format!("body of {title}"),
        enabled: true,
        updated_at: OffsetDateTime::now_utc(),
    }
}

/// TEST-9 — prompt cache determinism. Without the ordering, Postgres guarantees no row
/// order and the assembled system prompt differs per request.
#[sqlx::test(migrations = "../../migrations")]
async fn test_9_instructions_are_ordered_by_position_then_title(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    // Insert in a deliberately shuffled order
    for i in [
        ins(Phase::Triage, 2, "zeta"),
        ins(Phase::Triage, 0, "beta"),
        ins(Phase::Triage, 0, "alpha"),
        ins(Phase::Triage, 1, "gamma"),
    ] {
        store.upsert_instruction(&i).await.unwrap();
    }

    let got = store.instructions_for(&[Phase::Triage]).await.unwrap();
    let titles: Vec<_> = got.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, vec!["alpha", "beta", "gamma", "zeta"]);
}

/// Reading the same input repeatedly must be byte-identical.
#[sqlx::test(migrations = "../../migrations")]
async fn test_9_repeated_reads_are_byte_identical(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    for n in 0..12 {
        store
            .upsert_instruction(&ins(Phase::Rca, 0, &format!("rule-{n:02}")))
            .await
            .unwrap();
    }

    let render = |v: &[Instruction]| {
        v.iter().map(|i| format!("{}\n{}", i.title, i.body)).collect::<Vec<_>>().join("\n\n")
    };
    let a = render(&store.instructions_for(&[Phase::Rca]).await.unwrap());
    for _ in 0..5 {
        let b = render(&store.instructions_for(&[Phase::Rca]).await.unwrap());
        assert_eq!(a, b, "assembled prompt must be byte-identical across reads");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn multiple_phases_are_returned_together(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    store.upsert_instruction(&ins(Phase::All, 0, "global")).await.unwrap();
    store.upsert_instruction(&ins(Phase::Triage, 0, "triage-only")).await.unwrap();
    store.upsert_instruction(&ins(Phase::Rca, 0, "rca-only")).await.unwrap();

    let got = store.instructions_for(&[Phase::All, Phase::Triage]).await.unwrap();
    let titles: Vec<_> = got.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles.len(), 2);
    assert!(titles.contains(&"global"));
    assert!(titles.contains(&"triage-only"));
    assert!(!titles.contains(&"rca-only"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn disabled_instructions_are_excluded(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut off = ins(Phase::Triage, 0, "off");
    off.enabled = false;
    store.upsert_instruction(&off).await.unwrap();
    store.upsert_instruction(&ins(Phase::Triage, 1, "on")).await.unwrap();

    let got = store.instructions_for(&[Phase::Triage]).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "on");
}

#[sqlx::test(migrations = "../../migrations")]
async fn upsert_replaces_body_for_same_phase_and_title(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut i = ins(Phase::Triage, 0, "same");
    store.upsert_instruction(&i).await.unwrap();
    i.body = "revised".into();
    store.upsert_instruction(&i).await.unwrap();

    let got = store.instructions_for(&[Phase::Triage]).await.unwrap();
    assert_eq!(got.len(), 1, "(phase, title) is unique");
    assert_eq!(got[0].body, "revised");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_removes_the_instruction(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let i = ins(Phase::Triage, 0, "temp");
    store.upsert_instruction(&i).await.unwrap();
    store.delete_instruction(i.id).await.unwrap();
    assert!(store.instructions_for(&[Phase::Triage]).await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-store --test instructions 2>&1 | tail -20`
Expected: FAIL — `not yet implemented`

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-store/src/instructions.rs`:

```rust
use agentops_core::{Instruction, Phase, StoreError};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn row_to_instruction(row: &sqlx::postgres::PgRow) -> Result<Instruction, StoreError> {
    let phase: String = row.try_get("phase").map_err(crate::backend)?;
    Ok(Instruction {
        id: row.try_get("id").map_err(crate::backend)?,
        phase: phase
            .parse::<Phase>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        position: row.try_get("position").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        body: row.try_get("body").map_err(crate::backend)?,
        enabled: row.try_get("enabled").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

/// **`ORDER BY position, title` is required.** Without the ordering, the assembled system
/// system prompt differs per request and the prompt cache hit rate goes to zero (Section 8.3).
pub async fn for_phases(pool: &PgPool, phases: &[Phase]) -> Result<Vec<Instruction>, StoreError> {
    let names: Vec<String> = phases.iter().map(|p| p.as_str().to_owned()).collect();
    let rows = sqlx::query(
        "SELECT * FROM instructions
         WHERE enabled AND phase = ANY($1)
         ORDER BY position, title",
    )
    .bind(&names)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)?;
    rows.iter().map(row_to_instruction).collect()
}

pub async fn upsert(pool: &PgPool, ins: &Instruction) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO instructions (id, phase, position, title, body, enabled, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (phase, title) DO UPDATE
           SET position = EXCLUDED.position,
               body = EXCLUDED.body,
               enabled = EXCLUDED.enabled",
    )
    .bind(ins.id)
    .bind(ins.phase.as_str())
    .bind(ins.position)
    .bind(&ins.title)
    .bind(&ins.body)
    .bind(ins.enabled)
    .bind(ins.updated_at)
    .execute(pool)
    .await
    .map_err(crate::backend)?;
    Ok(())
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM instructions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(crate::backend)?;
    Ok(())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-store --test instructions 2>&1 | tail -15`
Expected: PASS — `6 passed`

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-store
git commit -m "feat(store): implement instructions with deterministic ordering"
```

---

### Task 10: Artifacts and transactional termination

**Purpose:** The second review's P1 — if saving the artifact, the step, and the status transition commit separately at termination, a partial success survives.

**Files:**
- Modify: `crates/agentops-store/src/artifacts.rs`
- Test: `crates/agentops-store/tests/artifacts.rs`

**Interfaces:**
- Consumes: `Artifact` and `NewArtifact` from Task 4; `allocate_seq` and `insert_step_strict` from Task 8
- Produces:
  - `pub async fn get(pool: &PgPool, id: Uuid) -> Result<Artifact, StoreError>`
  - `pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<Artifact>, StoreError>`
  - `pub async fn complete_investigation(pool: &PgPool, id: Uuid, a: &NewArtifact) -> Result<Uuid, StoreError>`
  - `pub async fn fail_investigation(pool: &PgPool, id: Uuid, reason: &TerminalReason) -> Result<(), StoreError>`

- [ ] **Step 1: Write the failing test**

`crates/agentops-store/tests/artifacts.rs`:

```rust
use agentops_core::{
    Investigation, InvestigationStatus, NewArtifact, Store, StepKind, StoreError, TerminalReason,
    TriggeredBy,
};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

async fn running(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let inv = Investigation {
        id: Uuid::new_v4(),
        title: "t".into(),
        prompt: "p".into(),
        status: InvestigationStatus::Queued,
        triggered_by: TriggeredBy::User,
        queued_at: now,
        started_at: None,
        finished_at: None,
        updated_at: now,
    };
    store.create_investigation(&inv).await.unwrap();
    store.mark_running(inv.id).await.unwrap();
    inv.id
}

/// TEST-14 — termination is one transaction. The success path.
#[sqlx::test(migrations = "../../migrations")]
async fn test_14_complete_commits_all_three_writes(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    let artifact_id = store
        .complete_investigation(
            id,
            &NewArtifact { title: "RCA".into(), body: "# Conclusion\nThe cause is X".into() },
        )
        .await
        .unwrap();

    let inv = store.get_investigation(id).await.unwrap();
    assert_eq!(inv.status, InvestigationStatus::Completed);
    assert!(inv.finished_at.is_some());

    let a = store.get_artifact(artifact_id).await.unwrap();
    assert_eq!(a.title, "RCA");
    assert_eq!(a.investigation_id, Some(id));

    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::ArtifactWritten { artifact_id });
}

/// TEST-14 — that no partial commit survives. **Verified by injecting an error** — a
/// test that only exercises the success path does not verify this item (spec Section 12.1).
///
/// The artifact insert succeeds and then the terminal step insert is made to fail.
/// Pre-inserting a row at the same `seq` that violates `agent_steps`'s `payload ? 'v'`
/// CHECK makes the next insert fail on the primary key — at which point both the
/// artifact and the status transition must roll back.
#[sqlx::test(migrations = "../../migrations")]
async fn test_14_partial_failure_rolls_back_everything(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    // Leaving next_step_seq at 0, occupy seq 0 in advance.
    // complete_investigation gets 0 from allocate_seq and collides on insert.
    sqlx::query(
        "INSERT INTO agent_steps (investigation_id, seq, phase, kind, payload)
         VALUES ($1, 0, 'all', 'text', '{\"v\":1,\"kind\":\"text\",\"text\":\"squatter\"}')",
    )
    .bind(id)
    .execute(store.pool())
    .await
    .unwrap();

    let err = store
        .complete_investigation(id, &NewArtifact { title: "lost".into(), body: "b".into() })
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Conflict),
        "a seq collision must surface, not be swallowed; got {err:?}"
    );

    // All three must roll back
    assert_eq!(
        store.get_investigation(id).await.unwrap().status,
        InvestigationStatus::Running,
        "status must not have transitioned"
    );
    assert!(
        store.list_artifacts(10).await.unwrap().is_empty(),
        "artifact must not survive the rollback"
    );
    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1, "only the pre-inserted squatter remains");
}

/// TEST-13 — the shutdown race. **Verified with real concurrency** — sequential calls do
/// not verify this item (spec Section 12.1).
#[sqlx::test(migrations = "../../migrations")]
async fn test_13_concurrent_terminal_writes_yield_exactly_one_winner(pool: sqlx::PgPool) {
    let store = std::sync::Arc::new(PgStore::new(pool));
    let id = running(&store).await;

    let s1 = std::sync::Arc::clone(&store);
    let s2 = std::sync::Arc::clone(&store);
    let (completed, failed) = tokio::join!(
        async move {
            s1.complete_investigation(id, &NewArtifact { title: "c".into(), body: "b".into() })
                .await
        },
        async move { s2.fail_investigation(id, &TerminalReason::ShutdownRequested).await },
    );

    let winners = [completed.is_ok(), failed.is_ok()].iter().filter(|ok| **ok).count();
    assert_eq!(winners, 1, "exactly one terminal write may succeed");

    let loser_err = if completed.is_err() {
        format!("{:?}", completed.unwrap_err())
    } else {
        format!("{:?}", failed.unwrap_err())
    };
    assert!(loser_err.contains("Conflict"), "loser must get Conflict, got {loser_err}");

    // TEST-19 — exactly one terminal step from the winner survives
    let steps = store.steps_after(id, -1).await.unwrap();
    let terminal = steps
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                StepKind::Terminated { .. } | StepKind::ArtifactWritten { .. }
            )
        })
        .count();
    assert_eq!(terminal, 1, "exactly one terminal step must land");
    assert!(store.get_investigation(id).await.unwrap().status.is_terminal());
}

/// On failure no artifact is created, but a Terminated step is left behind.
#[sqlx::test(migrations = "../../migrations")]
async fn fail_records_structured_reason(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;

    store
        .fail_investigation(id, &TerminalReason::Refusal { category: Some("cyber".into()) })
        .await
        .unwrap();

    let steps = store.steps_after(id, -1).await.unwrap();
    assert_eq!(steps.len(), 1);
    match &steps[0].kind {
        StepKind::Terminated { reason, .. } => assert_eq!(
            reason,
            &TerminalReason::Refusal { category: Some("cyber".into()) }
        ),
        other => panic!("expected Terminated, got {other:?}"),
    }
    assert!(store.list_artifacts(10).await.unwrap().is_empty());
}

/// The artifact survives even when the investigation is deleted (ON DELETE SET NULL).
#[sqlx::test(migrations = "../../migrations")]
async fn artifact_survives_investigation_deletion(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let id = running(&store).await;
    let aid = store
        .complete_investigation(id, &NewArtifact { title: "keep".into(), body: "b".into() })
        .await
        .unwrap();

    sqlx::query("DELETE FROM investigations WHERE id = $1")
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();

    let a = store.get_artifact(aid).await.unwrap();
    assert_eq!(a.investigation_id, None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_artifacts_is_newest_first(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    for n in 0..3 {
        let id = running(&store).await;
        store
            .complete_investigation(id, &NewArtifact { title: format!("a{n}"), body: "b".into() })
            .await
            .unwrap();
    }
    let list = store.list_artifacts(10).await.unwrap();
    assert_eq!(list.len(), 3);
    for w in list.windows(2) {
        assert!(w[0].created_at >= w[1].created_at, "must be newest-first");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-store --test artifacts 2>&1 | tail -20`
Expected: FAIL — `not yet implemented`

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-store/src/artifacts.rs`:

```rust
use agentops_core::{
    AgentStep, Artifact, NewArtifact, Phase, StepKind, StoreError, TerminalReason,
};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::steps::{allocate_seq, insert_step_strict};

fn row_to_artifact(row: &sqlx::postgres::PgRow) -> Result<Artifact, StoreError> {
    Ok(Artifact {
        id: row.try_get("id").map_err(crate::backend)?,
        investigation_id: row.try_get("investigation_id").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        body: row.try_get("body").map_err(crate::backend)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Artifact, StoreError> {
    let row = sqlx::query("SELECT * FROM artifacts WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(crate::backend)?
        .ok_or(StoreError::NotFound)?;
    row_to_artifact(&row)
}

pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<Artifact>, StoreError> {
    let rows = sqlx::query("SELECT * FROM artifacts ORDER BY created_at DESC, id DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(crate::backend)?;
    rows.iter().map(row_to_artifact).collect()
}

/// Saving the artifact, the `ArtifactWritten` step, and `running` → `completed` in
/// **in one transaction.** Committing them separately leaves a partial success.
///
/// **The status transition is attempted first.** If the conditional UPDATE catches no
/// row, another party already terminated it, so there is no need to create an artifact
/// or consume a `seq`. Reversing the order produces a pointless insert and rollback on the conflict path.
///
/// There is no `seq` parameter — the database allocates it inside the transaction (spec Section 6.1.1).
pub async fn complete_investigation(
    pool: &PgPool,
    id: Uuid,
    artifact: &NewArtifact,
) -> Result<Uuid, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    // 1. The conditional transition first. If it catches here, nothing else is done.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'completed', finished_at = now()
          WHERE id = $1 AND status = 'running'
        RETURNING id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::backend)?;

    if claimed.is_none() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::Conflict);
    }

    // 2. The artifact
    let artifact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO artifacts (id, investigation_id, title, body) VALUES ($1,$2,$3,$4)",
    )
    .bind(artifact_id)
    .bind(id)
    .bind(&artifact.title)
    .bind(&artifact.body)
    .execute(&mut *tx)
    .await
    .map_err(crate::backend)?;

    // 3. The terminal step. It uses no ON CONFLICT, so it cannot be lost.
    let seq = allocate_seq(&mut tx, id).await?;
    let step = AgentStep {
        investigation_id: id,
        seq,
        phase: Phase::All,
        kind: StepKind::ArtifactWritten { artifact_id },
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(artifact_id)
}

/// The `Terminated` step and `running` → `failed` in one transaction.
/// The transition is attempted first for the same reason as `complete_investigation`.
pub async fn fail_investigation(
    pool: &PgPool,
    id: Uuid,
    reason: &TerminalReason,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE investigations SET status = 'failed', finished_at = now()
          WHERE id = $1 AND status = 'running'
        RETURNING id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::backend)?;

    if claimed.is_none() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::Conflict);
    }

    let seq = allocate_seq(&mut tx, id).await?;
    let step = AgentStep {
        investigation_id: id,
        seq,
        phase: Phase::All,
        kind: StepKind::Terminated {
            reason: reason.clone(),
            detail: None,
        },
        created_at: OffsetDateTime::now_utc(),
    };
    insert_step_strict(&mut tx, &step).await?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-store --test artifacts 2>&1 | tail -15`
Expected: PASS — `6 passed`

- [ ] **Step 5: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-store
git commit -m "feat(store): implement transactional investigation termination"
```

---

### Task 11: The chat store — atomic `seq` allocation

**Purpose:** The `chat_messages.seq` race condition the review flagged. With two writers (the HTTP handler and the streaming task), computing `MAX(seq)+1` in the application collides.

**Files:**
- Modify: `crates/agentops-store/src/chat.rs`
- Test: `crates/agentops-store/tests/chat.rs`

**Interfaces:**
- Consumes: `ChatSession`, `ChatMessage`, and `ChatRole` from Task 4
- Produces:
  - `pub async fn create_session(pool: &PgPool, s: &ChatSession) -> Result<(), StoreError>`
  - `pub async fn list_sessions(pool: &PgPool, limit: i64) -> Result<Vec<ChatSession>, StoreError>`
  - `pub async fn messages(pool: &PgPool, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError>`
  - `pub async fn append_message(pool: &PgPool, session_id: Uuid, role: ChatRole, content: &serde_json::Value) -> Result<i64, StoreError>` — returns the allocated `seq`

- [ ] **Step 1: Write the failing test**

`crates/agentops-store/tests/chat.rs`:

```rust
use agentops_core::{ChatRole, ChatSession, Store};
use agentops_store::PgStore;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

async fn session(store: &PgStore) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let s = ChatSession {
        id: Uuid::new_v4(),
        title: "chat".into(),
        created_at: now,
        updated_at: now,
    };
    store.create_chat_session(&s).await.unwrap();
    s.id
}

#[sqlx::test(migrations = "../../migrations")]
async fn append_returns_monotonic_seq(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;

    let a = store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!("hi"))
        .await
        .unwrap();
    let b = store
        .append_chat_message(sid, ChatRole::Assistant, &serde_json::json!("hello"))
        .await
        .unwrap();
    assert_eq!((a, b), (0, 1));
}

/// TEST-7 — seq does not collide when there are two writers.
/// If the store did not allocate atomically, this test would fail on a PK violation.
#[sqlx::test(migrations = "../../migrations")]
async fn test_7_concurrent_appends_do_not_collide(pool: sqlx::PgPool) {
    let store = Arc::new(PgStore::new(pool));
    let sid = session(&store).await;

    let mut handles = Vec::new();
    for n in 0..20 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let role = if n % 2 == 0 { ChatRole::User } else { ChatRole::Assistant };
            store
                .append_chat_message(sid, role, &serde_json::json!(n))
                .await
        }));
    }

    let mut seqs = Vec::new();
    for h in handles {
        seqs.push(h.await.unwrap().expect("concurrent append must not fail"));
    }
    seqs.sort_unstable();
    assert_eq!(seqs, (0..20).collect::<Vec<i64>>(), "seqs must be 0..20 with no gaps or dupes");

    let msgs = store.chat_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 20);
}

#[sqlx::test(migrations = "../../migrations")]
async fn messages_are_ordered_by_seq(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;
    for n in 0..5 {
        store
            .append_chat_message(sid, ChatRole::User, &serde_json::json!(n))
            .await
            .unwrap();
    }
    let msgs = store.chat_messages(sid).await.unwrap();
    assert_eq!(msgs.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
}

/// The updated_at used for sidebar ordering advances when a message is appended.
#[sqlx::test(migrations = "../../migrations")]
async fn append_touches_session_updated_at(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let sid = session(&store).await;
    let before = store.list_chat_sessions(10).await.unwrap()[0].updated_at;

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_chat_message(sid, ChatRole::User, &serde_json::json!("x"))
        .await
        .unwrap();

    let after = store.list_chat_sessions(10).await.unwrap()[0].updated_at;
    assert!(after > before);
}

#[sqlx::test(migrations = "../../migrations")]
async fn sessions_are_newest_updated_first(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let old = session(&store).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let new = session(&store).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store
        .append_chat_message(old, ChatRole::User, &serde_json::json!("bump"))
        .await
        .unwrap();

    let list = store.list_chat_sessions(10).await.unwrap();
    assert_eq!(list[0].id, old, "the session touched most recently comes first");
    assert_eq!(list[1].id, new);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentops-store --test chat 2>&1 | tail -20`
Expected: FAIL — `not yet implemented`

- [ ] **Step 3: Write the minimal implementation**

`crates/agentops-store/src/chat.rs`:

```rust
use agentops_core::{ChatMessage, ChatRole, ChatSession, StoreError};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn row_to_session(row: &sqlx::postgres::PgRow) -> Result<ChatSession, StoreError> {
    Ok(ChatSession {
        id: row.try_get("id").map_err(crate::backend)?,
        title: row.try_get("title").map_err(crate::backend)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
        updated_at: row.try_get("updated_at").map_err(crate::backend)?,
    })
}

fn row_to_message(row: &sqlx::postgres::PgRow) -> Result<ChatMessage, StoreError> {
    let role: String = row.try_get("role").map_err(crate::backend)?;
    Ok(ChatMessage {
        session_id: row.try_get("session_id").map_err(crate::backend)?,
        seq: row.try_get("seq").map_err(crate::backend)?,
        role: role
            .parse::<ChatRole>()
            .map_err(|e| StoreError::Backend(e.to_string()))?,
        content: row.try_get("content").map_err(crate::backend)?,
        created_at: row.try_get("created_at").map_err(crate::backend)?,
    })
}

pub async fn create_session(pool: &PgPool, s: &ChatSession) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO chat_sessions (id, title, created_at, updated_at) VALUES ($1,$2,$3,$4)",
    )
    .bind(s.id)
    .bind(&s.title)
    .bind(s.created_at)
    .bind(s.updated_at)
    .execute(pool)
    .await
    .map_err(crate::backend)?;
    Ok(())
}

pub async fn list_sessions(pool: &PgPool, limit: i64) -> Result<Vec<ChatSession>, StoreError> {
    let rows = sqlx::query(
        "SELECT * FROM chat_sessions ORDER BY updated_at DESC, id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(crate::backend)?;
    rows.iter().map(row_to_session).collect()
}

pub async fn messages(pool: &PgPool, session_id: Uuid) -> Result<Vec<ChatMessage>, StoreError> {
    let rows = sqlx::query("SELECT * FROM chat_messages WHERE session_id = $1 ORDER BY seq")
        .bind(session_id)
        .fetch_all(pool)
        .await
        .map_err(crate::backend)?;
    rows.iter().map(row_to_message).collect()
}

/// Allocates `seq` atomically **inside the database.**
///
/// Chat has two writers — the HTTP handler writing the user message and the streaming
/// task writing the assistant message. Having the application compute and pass
/// `MAX(seq)+1` races. Taking the session row with `FOR UPDATE` serializes per session,
/// and `updated_at` is refreshed in the same transaction.
pub async fn append_message(
    pool: &PgPool,
    session_id: Uuid,
    role: ChatRole,
    content: &serde_json::Value,
) -> Result<i64, StoreError> {
    let mut tx = pool.begin().await.map_err(crate::backend)?;

    // Locking the session row is this session's append serialization point
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM chat_sessions WHERE id = $1 FOR UPDATE")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(crate::backend)?;
    if exists.is_none() {
        tx.rollback().await.map_err(crate::backend)?;
        return Err(StoreError::NotFound);
    }

    let max: Option<i64> =
        sqlx::query_scalar("SELECT MAX(seq) FROM chat_messages WHERE session_id = $1")
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(crate::backend)?;
    let seq = max.map_or(0, |m| m + 1);

    sqlx::query(
        "INSERT INTO chat_messages (session_id, seq, role, content) VALUES ($1,$2,$3,$4)",
    )
    .bind(session_id)
    .bind(seq)
    .bind(role.as_str())
    .bind(content)
    .execute(&mut *tx)
    .await
    .map_err(crate::backend)?;

    // For the sidebar's most-recent-first ordering
    sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(crate::backend)?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(seq)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentops-store 2>&1 | tail -20`
Expected: PASS — `34 passed` across `agentops-store` (investigations 8, steps 9, instructions 6, artifacts 6, chat 5). `agentops-core` is a separate crate at `20 passed`.

- [ ] **Step 5: Confirm no `todo!()` remains**

Run:
```bash
grep -rn "todo!()" crates/ && echo "TODO REMAINS - fix before commit" || echo "no todo!() left: OK"
```
Expected: `no todo!() left: OK`

- [ ] **Step 6: `cargo fmt`, then commit**

```bash
cargo fmt --all
git add crates/agentops-store
git commit -m "feat(store): implement chat with atomic per-session seq allocation"
```

---

### Task 12: The drift-guard CI

**Purpose:** Turn the design document's promise that "documentation and code stay current" into **a build failure**. Assign IDs to the test items in the spec's Section 12.1 and the invariants in Section 6.1, and have CI check that those IDs exist in the tests.

**Files:**
- Create: `scripts/check_spec_test_ids.py`
- Create: `scripts/check_stale_after.py`
- Create: `.github/workflows/ci.yml`
- Modify: `docs/superpowers/specs/2026-07-30-agentops-design.md` (add ID annotations to Sections 6.1 and 12.1)
- Modify: `CLAUDE.md` (update the "what is still missing" section)

**Interfaces:**
- Consumes: the test function names from Tasks 7 through 11 — `inv_4_*`, `test_4_*`, `test_7_*`, `test_9_*`, `test_11_*`, `test_13_*`, `test_14_*`, `test_19_*`, `test_20_*`
- Produces: four CI jobs — `fmt`, `clippy`, `test`, `drift-guards`

- [ ] **Step 1: Add ID annotations to the spec**

Append an ID to the end of each invariant title in Section 6.1 of `docs/superpowers/specs/2026-07-30-agentops-design.md`:

```
**1. An investigation is not bound to an HTTP request.** `[INV-1]`
**2. Replay is driven by the URL's `after` parameter. ...** `[INV-2]`
**3. Broadcast lag (`Lagged`) must be handled.** `[INV-3]`
**4. Zombie investigations are blocked at two layers.** `[INV-4]`
```

Append an ID to each of the **20** items in Section 12.1 as well. Because the item numbers run 1–14, then 19, 20, then 15–18 (19 and 20 were inserted after 14 during the third review), assign IDs **by item content, not by number**.

Then add the following paragraph before Section 12.1:

```markdown
The `[TEST-N]` and `[INV-N]` IDs given to each item **must appear in a test function
name** (the `test_4_...`, `inv_2_...` form). `scripts/check_spec_test_ids.py` checks this
in CI, so deleting an invariant from the spec leaves an orphan test and deleting a test is
blocked by CI. An item not yet implemented carries the `plan 2` or `plan 3` marker below
and is excluded from the check.
```

**The IDs plan 1 implements (nine)** — once this task is done, `check_spec_test_ids.py` requires them:

| ID | Test function | Task |
|---|---|---|
| `INV-4` | `inv_4_mark_running_is_conditional`, `inv_4_append_step_touches_investigation_updated_at` | 7, 8 |
| `TEST-4` | `test_4_steps_after_returns_only_later_seqs` | 8 |
| `TEST-7` | `test_7_concurrent_appends_do_not_collide` | 11 |
| `TEST-9` | `test_9_instructions_are_ordered_by_position_then_title`, `test_9_ties_across_phases_are_broken_by_id` | 9 |
| `TEST-11` | `test_11_tool_use_ids_survive_round_trip` | 8 |
| `TEST-13` | `test_13_concurrent_terminal_writes_yield_exactly_one_winner` | 10 |
| `TEST-14` | `test_14_complete_commits_all_three_writes`, `test_14_partial_failure_rolls_back_everything` | 10 |
| `TEST-19` | `test_19_every_cleaned_investigation_gets_exactly_one_terminal_step` | 8 |
| `TEST-20` | `test_20_race_interleaving_leaves_newly_running_investigation_untouched` | 8 |

Mark the rest (`INV-1`, `INV-2`, `INV-3`, `TEST-1`, `TEST-2`, `TEST-3`, `TEST-5`, `TEST-6`, `TEST-8`, `TEST-10`, `TEST-12`, `TEST-15`, `TEST-16`, `TEST-17`, `TEST-18`) with `(plan 2)` or `(plan 3)` to exclude them from the check.

> **`TEST-20` lives in the `#[cfg(test)] mod tests` inside `crates/agentops-store/src/steps.rs`, not under `crates/agentops-store/tests/`.** Constructing the interleaving between the lock and the UPDATE requires calling the `pub(crate)` functions `select_orphans` and `terminate_orphans` directly in the middle of a transaction, and neither is visible from an external integration test crate. **The traceability script must therefore scan the unit tests in `src/`, not only the `tests/` directory.** Scanning only `tests/` would fail to find `TEST-20` and pass silently.
>
> The first version of this table pointed at `test_20_boot_cleanup_only_touches_locked_ids`. That test did not verify the READ COMMITTED scoping its name claimed (it passed even with `AND id = ANY($1)` removed) — because the fixture's `b` never becomes `running` at any point, so the remaining `WHERE status = 'running'` condition already excludes it. It was replaced by a genuine interleaving test, and the name changed with it. Evidence: `7f30f43`

`INV-2` spans the storage layer (`steps_after`) and the HTTP layer (the `?after=` route). Plan 1 implements only the storage layer, so `INV-2` is marked `(plan 3)` in the spec and `TEST-4` covers the storage layer.

- [ ] **Step 2: Write the traceability check script**

`scripts/check_spec_test_ids.py`:

```python
#!/usr/bin/env python3
"""Check that the spec's [INV-N] and [TEST-N] IDs exist in test function names.

A mechanism that turns documentation-to-code drift into a build failure. Deleting an
invariant from the spec exposes an orphan test; deleting a test is blocked by this check.

An ID with `(plan 2)` or `(plan 3)` on the same line is not yet in implementation scope and
is skipped.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC_GLOB = "docs/superpowers/specs/*.md"
ID_RE = re.compile(r"\[((?:INV|TEST)-\d+)\]")
DEFERRED_RE = re.compile(r"\((?:계획|plan)\s*[23]\)", re.I)


def spec_ids() -> dict[str, bool]:
    """ID to whether it is in implementation scope now."""
    found: dict[str, bool] = {}
    for path in sorted(ROOT.glob(SPEC_GLOB)):
        for line in path.read_text(encoding="utf-8").splitlines():
            deferred = bool(DEFERRED_RE.search(line))
            for m in ID_RE.finditer(line):
                # When an ID appears on several lines, one in-scope occurrence makes it in-scope
                found[m.group(1)] = found.get(m.group(1), False) or not deferred
    return found


def test_ids() -> set[str]:
    """Extract IDs from test function names. inv_4_foo gives INV-4."""
    ids: set[str] = set()
    name_re = re.compile(r"\bfn\s+((?:inv|test)_\d+)_")
    for path in ROOT.glob("crates/**/*.rs"):
        for m in name_re.finditer(path.read_text(encoding="utf-8")):
            prefix, num = m.group(1).split("_")
            ids.add(f"{prefix.upper()}-{num}")
    return ids


def main() -> int:
    spec = spec_ids()
    tests = test_ids()

    in_scope = {i for i, active in spec.items() if active}
    missing = sorted(in_scope - tests, key=lambda s: (s.split("-")[0], int(s.split("-")[1])))
    orphaned = sorted(tests - set(spec), key=lambda s: (s.split("-")[0], int(s.split("-")[1])))

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
```

- [ ] **Step 3: Write the `stale_after` expiry check script**

`scripts/check_stale_after.py`:

```python
#!/usr/bin/env python3
"""Find documents whose OKF frontmatter stale_after has passed.

OKF declares expiry; it does not compute it. This script computes it.
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
        when = dt.date.fromisoformat(m.group(1))
        if when < today:
            expired.append((str(path.relative_to(ROOT)), m.group(1)))

    if expired:
        print("FAIL: documents past their stale_after:")
        for p, when in expired:
            print(f"  {p} (stale_after: {when})")
        print("\nCheck the document against the current code, then extend stale_after")
        print("and add a verified[] entry, or update or retire it.")
        return 1

    print(f"OK: {checked} documents with frontmatter, none expired")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 4: Confirm both scripts pass**

Run:
```bash
chmod +x scripts/*.py
python3 scripts/check_spec_test_ids.py
python3 scripts/check_stale_after.py
```
Expected: both print a line beginning with `OK:` and exit 0. If `check_spec_test_ids.py` fails, the ID assignment in Step 1 disagrees with the test function names from Tasks 7 through 11 — reconcile them.

- [ ] **Step 5: Write the CI workflow**

`.github/workflows/ci.yml`:

```yaml
name: ci

on:
  push: { branches: [main] }
  pull_request:

env:
  CARGO_TERM_COLOR: always
  # CI uses a service container, so there is no port collision — only local uses 55433
  DATABASE_URL: postgres://agentops:agentops@localhost:5432/agentops

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo fmt --all --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: Swatinem/rust-cache@v2
      # Only runtime-validated queries are used, so it compiles without a database
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17-alpine
        env:
          POSTGRES_USER: agentops
          POSTGRES_PASSWORD: agentops
          POSTGRES_DB: agentops
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U agentops"
          --health-interval 2s --health-timeout 3s --health-retries 20
    steps:
      - uses: actions/checkout@v4
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --all-targets
      # Fails when a code example in the documentation no longer compiles.
      # The original brief's axum::Server example was of this kind.
      - run: cargo test --workspace --doc

  # There is no sqlx-offline job. This project does not use the `query!` macro, so
  # `cargo sqlx prepare` reports "no queries found" and writes an empty .sqlx, and
  # `--check` passes with or without a cache — spending a postgres service and four
  # minutes of sqlx-cli installation on a job that checks nothing. Schema drift is
  # caught by the integration tests in the `test` job above (spec Section 7).

  drift-guards:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: "3.12" }
      # Whether the spec's invariant and test item IDs exist in the actual tests
      - run: python3 scripts/check_spec_test_ids.py
      # Whether any document is past its revalidation date
      - run: python3 scripts/check_stale_after.py
```

- [ ] **Step 6: Confirm no `todo!()` or `allow(unused_variables)` remains**

Run:
```bash
grep -rn "todo!()" crates/ && echo "TODO REMAINS" || echo "no todo!(): OK"
grep -rn "allow(unused_variables)" crates/ && echo "ALLOW REMAINS" || echo "no stub allow: OK"
```
Expected: both lines end with `: OK`. Any leftover stub from Task 7 Step 5 is caught here.

- [ ] **Step 7: Make clippy and fmt pass locally**

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: zero clippy warnings. Fix any that appear.

- [ ] **Step 8: Update the "what is still missing" section of `CLAUDE.md`**

In that section of `CLAUDE.md`, change the status of all four guards to "in place" and replace the sentence below the table with:

```markdown
**Three of the four devices are in CI** (`.github/workflows/ci.yml`). The `drift-guards`
job checks spec ID traceability and `stale_after` expiry; the `test` job checks doc tests.
Schema drift is caught not by `cargo sqlx prepare --check` but by **the integration tests
that run against a migrated database** — this project does not use the `query!` macro, so
`--check` checks nothing (spec Section 7).

A remaining limitation: these devices guarantee **that what the spec claims corresponds to
what the tests verify**, but not that the tests are actually correct. An invariant's ID can
be in a test name while that test verifies the invariant wrongly. That belongs to code
review.
```

- [ ] **Step 9: Append to `docs/log.md`**

```bash
cat >> docs/log.md <<'EOF'
- `added` — `.github/workflows/ci.yml`, `scripts/check_spec_test_ids.py`, `scripts/check_stale_after.py` — introduced the four drift guards into CI. It checks whether the spec's `[INV-N]` and `[TEST-N]` IDs exist in test function names, so a document-code mismatch becomes a build failure
- `revised` — `superpowers/specs/2026-07-30-agentops-design.md` — gave traceability IDs to the Section 6.1 invariants and the Section 12.1 test items
EOF
```

- [ ] **Step 10: Verify everything, then commit**

Run:
```bash
export DATABASE_URL=postgres://agentops:agentops@localhost:55433/agentops
cargo fmt --all --check && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace --all-targets 2>&1 | tail -8 && \
python3 scripts/check_spec_test_ids.py && \
python3 scripts/check_stale_after.py
```
Expected: everything passes, with the last two lines beginning `OK:`.

```bash
git add .github scripts docs CLAUDE.md
git commit -m "ci: add drift guards for spec-test traceability and doc freshness"
```

- [ ] **Step 11: Update the graph (a CLAUDE.md convention)**

Run:
```bash
graphify . --update
```
Expected: the new files (crate sources, scripts, CI) are added to the graph. Code files go through AST extraction, so there is no LLM call.

```bash
git add docs/log.md && git commit -m "docs: log graph update after foundation implementation" --allow-empty
```

---

## Self-Review

**1. Spec coverage — checking plan 1's scope against the spec's requirements**

| Spec requirement | Task |
|---|---|
| Section 4.1 crate boundaries (core must not depend on an I/O crate) | Task 1, Task 5 Step 5 (`cargo tree` verification) |
| Section 5.1 domain types (including `tool_use_id` and `TerminalReason`) | Tasks 2, 3 |
| Section 5.2 trait definitions, object safety, `Send + 'static` | Task 5 |
| Section 5.3 sequential phase progression (`INVESTIGATION_ORDER`) | Task 3 |
| Section 5.4 loop limits | **plan 2** (used in the agent loop) |
| Section 6.1 INV-2 replay (the `steps_after` storage layer) | Task 8 |
| Section 6.1 INV-4 conditional transitions, orphan cleanup, the watchdog query | Tasks 7, 8, 10 |
| Section 6.1.1 `seq` allocation (single writer for investigations, serialization for chat) | Tasks 8, 11 |
| Section 6.1.2 separating the protocol transcript (steps hold summaries only) | Task 3 (`Thinking { summary }`) |
| Section 6.1.3 a persisted step is not a delta | Task 3 documents it, **plan 2** honors it |
| Section 6.1 terminal transactionality | Task 10 |
| Section 7 the full schema, CHECKs, indexes, triggers | Task 6 |
| Section 7 the `payload` version field | Tasks 3, 6, 8 |
| Section 8.3 determinism of instruction ordering | Task 9 |
| Section 9.1 the tool policy table (deny by default) | Task 6 (the schema), **plan 2** (enforcement) |
| Section 12.1 TEST-4, 7, 9, 11, 13, 14 | Tasks 8, 9, 10, 11 |
| Section 15 CI (fmt, clippy, test, doc test, drift guards) | Task 12 |
| The four CLAUDE.md drift guards | Task 12 |

**Left outside plan 1's scope:** Section 5.4 loop limits, all of Section 8 (the LLM client), Section 9's MCP implementation, Section 10 HTTP/SSE, Section 11's runtime error-handling policy, Section 13's security bind. Each belongs to plan 2 or 3.

**2. Placeholder scan**

- The `todo!()` in Task 7 Step 5 is deliberate temporary scaffolding, and Task 11 Step 5 verifies its removal — it is not an abandoned placeholder
- No instructions of the "add appropriate error handling" kind. Every error path names a concrete type (`StoreError::Conflict` and so on)
- Every code step has a runnable code block
- Task 12 Step 6 explicitly handles the case where `.sqlx` may be empty — it is not left to guesswork

**3. Type consistency**

- `StoreError::Conflict` — defined in Task 4, used in Tasks 7 and 10. Consistent.
- `Phase::as_str()` / `FromStr` — defined in Task 3, used in Tasks 8 and 9. Consistent.
- `AgentStep::payload_json()` — defined in Task 3, used in Task 8. Consistent.
- `StepKind::from_payload_json()` — defined in Task 3, used in Task 8. Consistent.
- `allocate_seq` / `insert_step_strict` — defined `pub(crate)` in Task 8, used in Task 10. Task 8 precedes Task 10, so there is no ordering problem.
- `row_to_investigation` — defined `pub(crate)` in Task 7, used in Task 8. Consistent.
- `Store::append_step` returns `seq` rather than taking it — defined in Task 5, implemented in Task 8, used by the tests of Tasks 8 and 10. Consistent.
- `PgStore::pool()` — defined in Task 7, used in the tests of Tasks 8, 10, and 11. Consistent.
- `ListFilter::default()` — defined in Task 5 (`limit: 50`), used as `..Default::default()` in Task 7's tests. Consistent.
- `crate::backend` — defined in Task 7 Step 4 (`lib.rs`), used throughout Tasks 7 through 11. Consistent.

**The first version's real defect and its fix (found in the third review):**

The first version placed `fail_orphaned_running` in Task 7, and that function
called Task 8's `next_seq_for` and `insert_terminated`. The Self-Review wrote
that this was "resolved by the stub instructions in Task 7 Step 5", but **that
was not true** — those instructions said "create empty modules only", and an
empty module cannot compile a call to a function that does not exist. The
reviewer compiled it for real and reproduced
`error[E0432]: unresolved import` and `error[E0425]: cannot find function`.

Two things were fixed:
1. `fail_orphaned_running` was **moved to Task 8** — it inserts a terminal step,
   so it logically belongs to the `steps` module too. Task 7 no longer
   references `steps`.
2. Task 7 Step 5's stubs were changed to **include the signatures**. Not empty
   files, but the final signature with a `todo!()` body, and Task 12 Step 6
   verifies the scaffolding is removed.

**The lesson:** a Self-Review does not prove resolution by writing "resolved".
Only actually compiling the plan's code proves it.

---

## Verification history

This plan received two rounds of independent review, and both **actually compiled the plan's code**.

**Round 1 (before fixes)** — one blocker: Task 7 did not compile at its own verification step. The Self-Review wrote that "the stub instructions resolved it", but those instructions said "create empty modules only", and an empty file cannot compile a call to a function that does not exist. Five majors (the duplicated `seq` allocation path, the `UnknownStopReason` serialization panic, the boot cleanup's lock scope, TEST-13 and TEST-14 not verifying what they claim, and a vacuous `sqlx-offline` job). My own claim that `serde_json::Value` would fail to compile against `Eq` was **refuted** — it is implemented.

**Round 2 (after fixes)** — confirming that round 1's fixes actually compile. Three majors:

| Defect | Cause |
|---|---|
| `StepKind` missing from `traits.rs` imports | Dropped when the trait signature changed to `kind: &StepKind` |
| `StepKind` missing from `store/src/lib.rs` imports | Same cause |
| `rust-version = "1.88"` is too low | **`sqlx 0.9.0` itself requires 1.94.** The reason I had written (edition2024 in `time` / `getrandom`) was not the actual constraint |

Final state after the fixes: 20 `agentops-core` tests plus 34 `agentops-store` tests passing, clippy clean under `-D warnings`, fmt clean, concurrency tests passing 6/6 on repeat, and all of Task 12's gates passing.

**What round 2 confirmed** — all five of my suspicions turned out to be fine: `is_unique_violation()` resolves without a trait import; `RETURNING next_step_seq - 1` infers as `Option<i64>` with no cast; calling `allocate_seq` from a transaction that took `FOR UPDATE` in `fail_orphaned_running` reuses the same lock in the same transaction and does not deadlock; a triple-escaped JSONB literal satisfies the `payload ? 'v'` CHECK; and `?` dropping the transaction rolls back all three writes.

**Environment note:** the reviewer's machine had port 5432 occupied, so `docker-compose.yml` failed to bind. This is not a defect in the plan, but with another local Postgres present the port must be changed.

## Preview of plans 2 and 3

These are written after plan 1 finishes, **looking at the code as it actually is** at that point. Writing them now would make them guesses about code that does not exist.

**Plan 2 — the agent:** the Anthropic Messages API client (raw `reqwest`, the SSE state machine, seven `stop_reason` branches), the MCP tool registry (`rmcp` 3.0.1, name-ordered sorting, deny-by-default policy enforcement), and the per-phase loop (Section 5.3's sequential sub-loop, Section 5.4's seven limits, Section 6.1.2's protocol transcript). Deliverable: one investigation running end to end from a CLI. Covers TEST-5, 6, 11, 12, 15, 16, 17.

**Plan 3 — the web:** Axum 0.8 routes, the two SSE layers (Section 6.1.3's `step`/`delta` split, Section 10.2.1's wire contract), `JobManager` (a semaphore, a `JoinSet`, the watchdog, the six-stage shutdown), the askama plus HTMX three-pane UI, and chat. Deliverable: it works in a browser. Covers TEST-1, 2, 3, 8, 10, 13, 18.

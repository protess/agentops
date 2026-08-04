# agentops

A self-hosted, open-source DevOps investigation agent, written in Rust.

You describe an incident in plain language. The agent runs a bounded
investigation — triage, then root-cause analysis, then mitigation — calling
tools over MCP as it goes, streaming every step to your browser, and leaving
a written artifact behind. Investigations survive page reloads, server
restarts, and disconnects, because they are owned by the server and not by
your HTTP connection.

No vendor lock-in in either direction: the cloud you observe and the LLM you
run it with are both swappable.

> [!WARNING]
> **v0.1 has no authentication.** The default bind address is
> `127.0.0.1:3000` (loopback), and that bind is the only thing standing
> between agentops and the network. Do not set `AGENTOPS_BIND` to `0.0.0.0`
> or any public interface — that exposes an unauthenticated server that can
> call your tools. Put it behind a reverse proxy that authenticates, or keep
> it on loopback.

## Status

v0.1 is a working vertical slice: an investigation launched from the browser
runs to completion, streams live, and persists. It is early software. The
[known limitations](docs/superpowers/known-limitations.md) are written down
honestly, including the ones that are still open.

- 4 crates, 263 tests
- CI runs `fmt`, `clippy`, `test`, and four drift guards
- No authentication, no multi-tenancy, no access control (see the warning above)

## Requirements

| | |
|---|---|
| Rust | 1.94 or newer (`sqlx` 0.9 sets this floor). `rust-toolchain.toml` pins the channel, so `rustup` selects it for you. |
| Postgres | 17. A `docker-compose.yml` is included; Docker is only needed if you use it. |
| An Anthropic API key | `ANTHROPIC_API_KEY` |
| Python | 3.12, for the drift-guard scripts. Not needed to build or run the server. |

No Node.js. No CDN. Frontend assets are committed to the repository.

## Quick start

```bash
# 1. Start Postgres. Host port 55433, chosen so it will not collide with a
#    local 5432. Wait for the healthcheck before continuing.
docker compose up -d
docker compose ps          # wait until postgres shows (healthy)

# 2. Apply the schema. THIS STEP IS REQUIRED — the server does not migrate
#    on startup; see "Migrations" below.
docker compose exec -T postgres psql -U agentops -d agentops \
  -v ON_ERROR_STOP=1 -q < migrations/0001_initial.sql

# 3. Point the app at it
export DATABASE_URL='postgres://agentops:agentops@localhost:55433/agentops'
export ANTHROPIC_API_KEY='sk-ant-...'

# 4. Run
cargo run -p agentops-server
```

Then open <http://127.0.0.1:3000>. The root path redirects to `/incidents`.

Verify it came up:

```bash
curl -i http://127.0.0.1:3000/api/health     # 200 once the database answers
```

A `.env.example` holds the same `DATABASE_URL`; copy it to `.env` if you
prefer that to exporting.

### Migrations

**The server does not apply migrations at startup.** It connects, then
immediately runs boot recovery, which queries `investigations` — against an
empty database that fails with:

```
Error: store backend error: error returned from database: relation "investigations" does not exist
```

Apply `migrations/0001_initial.sql` yourself, by either route:

```bash
# Through the compose container — no extra tooling
docker compose exec -T postgres psql -U agentops -d agentops \
  -v ON_ERROR_STOP=1 -q < migrations/0001_initial.sql

# Or with sqlx-cli, if you have it
cargo install sqlx-cli --no-default-features --features postgres
sqlx migrate run                    # reads DATABASE_URL
```

The test suite is unaffected either way: `#[sqlx::test]` creates and migrates
a throwaway database per test. That is why this gap survived — the tests
never touch a hand-made database. It is tracked as **L18** in the
[known limitations](docs/superpowers/known-limitations.md).

### Configuration

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `DATABASE_URL` | yes | — | Postgres connection string |
| `ANTHROPIC_API_KEY` | yes | — | Key for the LLM provider. Read at startup, so a missing key fails immediately rather than on the first investigation. |
| `AGENTOPS_BIND` | no | `127.0.0.1:3000` | Listen address. Read the warning above before changing it. |
| `RUST_LOG` | no | off | Standard `tracing` filter, e.g. `RUST_LOG=agentops_server=debug` |

### If something fails

| Symptom | Cause and fix |
|---|---|
| `relation "investigations" does not exist` | The schema was never applied. See **Migrations** above. |
| `DATABASE_URL is not set` / `ANTHROPIC_API_KEY is not set` | Both are read at startup and neither has a default. |
| `Bind for 0.0.0.0:55433 failed: port is already allocated` | Something else holds 55433 — often a container left behind by a deleted worktree. Find it with `docker ps -a --filter publish=55433`. Either remove it, or change the port in **both** `docker-compose.yml` and `DATABASE_URL`. |
| `sqlx@0.9.0 requires rustc 1.94.0` | The toolchain is too old. `rustup update`. |
| `Protocol("unexpected response from SSLRequest: 0x00")` during tests | Missing `--test-threads=4`. See **Development** below — the failure surfaces in an unrelated test and looks like a code defect. |

## How it works

```
Browser ──HTMX/SSE──► agentops-server ──► JobManager ──► investigation runner
                            │                                    │
                            │                                    ├─► LLM provider (Anthropic)
                            ▼                                    └─► MCP tool servers
                        Postgres  ◄──────── every step persisted ─┘
```

An investigation is a background job, not a request handler. The browser
subscribes to a step stream; the server replays what it missed from the
database and then switches to live events. If the browser goes away, the
investigation keeps running. If the server restarts, a boot sweep and a
watchdog reclaim anything left mid-flight.

### Workspace layout

| Crate | Responsibility |
|---|---|
| `agentops-core` | Domain types and traits. **No I/O dependencies** — this is enforced by the compiler and is the only reason the split exists. |
| `agentops-store` | Postgres persistence |
| `agentops-agent` | Anthropic streaming, MCP client, the phase loop, the investigation runner |
| `agentops-server` | Axum, SSE, HTMX rendering, `JobManager`, watchdog |

## Development

```bash
cargo test --workspace --all-targets -- --test-threads=4
```

**The `--test-threads=4` is a correctness requirement, not a performance
tweak.** `#[sqlx::test]` creates a connection pool per test, and this
workspace has 19 test binaries. At the default parallelism they exhaust
Postgres's `max_connections=100`, and the failure surfaces as
`Protocol("unexpected response from SSLRequest: 0x00")` **in an unrelated
test**, which looks exactly like a code defect. CI pins the same value.

Other checks CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --doc
python3 scripts/check_spec_test_ids.py     # spec invariant IDs exist in tests
python3 scripts/check_stale_after.py       # no document is past its revalidation date
python3 scripts/check_index_complete.py    # the bundle index matches the filesystem
```

The test suite needs a reachable `DATABASE_URL` but **not** a migrated
database — `#[sqlx::test]` creates and migrates one per test. It does not need
`ANTHROPIC_API_KEY`; the provider is faked.

### Knowledge graph

`CLAUDE.md` asks contributors to query the project's knowledge graph before
opening files. It is **not** in the repository — `graphify-out/` is
gitignored, because a committed graph goes stale silently and a stale graph is
worse than none. Regenerate it after cloning:

```bash
graphify . --update --code-only   # code only, deterministic, no API key
graphify . --update               # adds the documents; needs an LLM
```

### Frontend assets

`crates/agentops-server/static/` holds committed files: htmx 2.0.4, the SSE
extension 2.2.2, and a Tailwind v4 build. There is no build step in the
normal workflow — if the `tailwindcss` binary is not installed, the committed
`static/app.css` is used as-is. Rebuild instructions, including the version
pin and the reason for it, are in
[`docs/frontend-assets.md`](docs/frontend-assets.md).

## Documentation

Project knowledge is kept as an [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
bundle: every document declares its own lifecycle, provenance, and review
history in its frontmatter, and CI fails the build when a document goes past
its revalidation date or the index drifts from the filesystem.

| | |
|---|---|
| [`docs/index.md`](docs/index.md) | Bundle root — the document list and the conventions |
| [`docs/superpowers/specs/2026-07-30-agentops-design.md`](docs/superpowers/specs/2026-07-30-agentops-design.md) | The design spec |
| [`docs/superpowers/known-limitations.md`](docs/superpowers/known-limitations.md) | What is open, and why |
| [`docs/log.md`](docs/log.md) | Append-only change log |
| [`CLAUDE.md`](CLAUDE.md) | Working conventions for agents contributing to this repository |

## Contributing

Read [`CLAUDE.md`](CLAUDE.md) first — it is short, and it is binding for both
humans and agents. Two conventions matter more than the rest:

- **Assert counts, not presence.** `assert_eq!(rows.filter(..).count(), 1)`,
  not `assert!(rows.iter().any(..))`. Two defects in this repository hid
  behind `any()` because it cannot tell one row from two.
- **Mutation-test your tests.** Break the guard you just wrote and confirm the
  test fails. Several tests here passed while verifying nothing, and only
  this caught them.

Documents are never deleted. To retire one, set `status: deprecated`, point
the replacement's `supersedes` at it, and append a line to `docs/log.md`.

## License

Not yet chosen. Until a `LICENSE` file lands, no license is granted.

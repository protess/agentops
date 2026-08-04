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

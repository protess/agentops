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
    // Clamp so a limit of zero or below, or an oversized one, becomes neither a SQL
    // error nor an overflow (the same convention as investigations::list and artifacts::list).
    let limit = limit.clamp(1, 500);
    let rows =
        sqlx::query("SELECT * FROM chat_sessions ORDER BY updated_at DESC, id DESC LIMIT $1")
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
///
/// Unlike investigation steps (Task 8) this uses no counter column — a chat message is
/// written once, on completion (spec Section 6.2), so a short transaction suffices.
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

    sqlx::query("INSERT INTO chat_messages (session_id, seq, role, content) VALUES ($1,$2,$3,$4)")
        .bind(session_id)
        .bind(seq)
        .bind(role.as_str())
        .bind(content)
        .execute(&mut *tx)
        .await
        .map_err(crate::backend)?;

    // For the sidebar's recency ordering. The `t_chat_sessions_touch` trigger overwrites
    // NEW.updated_at with now() on UPDATE, so the value assigned here is itself ignored —
    // the purpose of this statement is not the assignment but the UPDATE that fires the
    // trigger. chat_sessions has no other real column to update, which makes it different
    // from other modules where the trigger runs as a side effect of touching a real
    // column.
    sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(crate::backend)?;

    tx.commit().await.map_err(crate::backend)?;
    Ok(seq)
}

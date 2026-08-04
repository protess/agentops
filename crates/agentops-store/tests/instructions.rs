use agentops_core::{Instruction, Phase, Store};
use agentops_store::PgStore;
use time::OffsetDateTime;
use uuid::Uuid;

fn ins(phase: Phase, position: i32, title: &str) -> Instruction {
    ins_with_id(Uuid::new_v4(), phase, position, title)
}

fn ins_with_id(id: Uuid, phase: Phase, position: i32, title: &str) -> Instruction {
    Instruction {
        id,
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

/// TEST-9 — `(phase, title)` is unique, but `position, title` alone is not a total
/// order: different phases can hold the same `(position, title)` (querying several
/// phases at once, as in `for_phases(&[All, Triage])`, is the agent's normal call
/// pattern). Without the `id` tiebreaker, Postgres orders those tied rows arbitrarily,
/// and that order can change after a vacuum or a plan change, so the assembled prompt
/// differs per request. This test deliberately creates that tie and verifies it is
/// broken by ascending `id`.
///
/// The ids are assigned **deliberately opposite to alphabetical phase order** — since
/// `'all' < 'triage'`, an implementation using `phase` as the third tiebreaker (which the
/// user explicitly rejected) produces a different result here, distinguishing it from the
/// `id` tiebreaker. Assigning ids from low to high as `Triage, All` means: sorting by id
/// returns `Triage` first, while sorting by phase returns `All` first, so the two
/// implementations diverge.
#[sqlx::test(migrations = "../../migrations")]
async fn test_9_ties_across_phases_are_broken_by_id(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let safety_on_triage = Uuid::from_u128(1); // low id, later by phase
    let safety_on_all = Uuid::from_u128(2); // high id, earlier by phase
    let tone_on_triage = Uuid::from_u128(3);
    let tone_on_all = Uuid::from_u128(4);

    // Insert in a deliberately shuffled order, with (position=0, title) overlapping
    // across phases.
    store
        .upsert_instruction(&ins_with_id(tone_on_all, Phase::All, 0, "tone"))
        .await
        .unwrap();
    store
        .upsert_instruction(&ins_with_id(safety_on_all, Phase::All, 0, "safety"))
        .await
        .unwrap();
    store
        .upsert_instruction(&ins_with_id(tone_on_triage, Phase::Triage, 0, "tone"))
        .await
        .unwrap();
    store
        .upsert_instruction(&ins_with_id(safety_on_triage, Phase::Triage, 0, "safety"))
        .await
        .unwrap();

    let got = store
        .instructions_for(&[Phase::All, Phase::Triage])
        .await
        .unwrap();
    let ids: Vec<Uuid> = got.iter().map(|i| i.id).collect();
    // Ascending id: [1,2,3,4]. Using phase as the tiebreaker instead yields
    // [safety_on_all, safety_on_triage, tone_on_all, tone_on_triage]
    // [2,1,4,3] and fails this assertion.
    assert_eq!(
        ids,
        vec![safety_on_triage, safety_on_all, tone_on_triage, tone_on_all],
        "ties on (position, title) across phases must break on id, not on phase"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn multiple_phases_are_returned_together(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    store
        .upsert_instruction(&ins(Phase::All, 0, "global"))
        .await
        .unwrap();
    store
        .upsert_instruction(&ins(Phase::Triage, 0, "triage-only"))
        .await
        .unwrap();
    store
        .upsert_instruction(&ins(Phase::Rca, 0, "rca-only"))
        .await
        .unwrap();

    let got = store
        .instructions_for(&[Phase::All, Phase::Triage])
        .await
        .unwrap();
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
    store
        .upsert_instruction(&ins(Phase::Triage, 1, "on"))
        .await
        .unwrap();

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

/// Upserting again with the same `(phase, title)` must preserve the original row's `id` —
/// a different `Uuid::new_v4()` from the new call is ignored.
#[sqlx::test(migrations = "../../migrations")]
async fn upsert_preserves_original_id_on_conflict(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let original = ins(Phase::Triage, 0, "same");
    store.upsert_instruction(&original).await.unwrap();

    let mut resubmitted = ins(Phase::Triage, 0, "same");
    resubmitted.id = Uuid::new_v4();
    assert_ne!(
        resubmitted.id, original.id,
        "test setup must use a fresh id"
    );
    store.upsert_instruction(&resubmitted).await.unwrap();

    let got = store.instructions_for(&[Phase::Triage]).await.unwrap();
    assert_eq!(got.len(), 1, "(phase, title) is unique");
    assert_eq!(got[0].id, original.id, "id must not churn on conflict");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_removes_the_instruction(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let i = ins(Phase::Triage, 0, "temp");
    store.upsert_instruction(&i).await.unwrap();
    store.delete_instruction(i.id).await.unwrap();
    assert!(store
        .instructions_for(&[Phase::Triage])
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_of_missing_id_is_not_found(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let err = store.delete_instruction(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, agentops_core::StoreError::NotFound));
}

/// `update_instruction` selects its update target by `id` — unlike `upsert_instruction`,
/// which selects by `(phase, title)` (Task 11 review Critical). This pins the simplest
/// path first, changing only `body` and `position` without touching
/// `(phase, title)`.
#[sqlx::test(migrations = "../../migrations")]
async fn update_by_id_changes_body_and_position(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let mut i = ins(Phase::Triage, 0, "edit-me");
    store.upsert_instruction(&i).await.unwrap();

    i.position = 5;
    i.body = "revised".into();
    store.update_instruction(&i).await.unwrap();

    let got = store.instructions_for(&[Phase::Triage]).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, i.id, "the id must be unchanged");
    assert_eq!(got[0].position, 5);
    assert_eq!(got[0].body, "revised");
}

#[sqlx::test(migrations = "../../migrations")]
async fn update_of_missing_id_is_not_found(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let missing = ins(Phase::Triage, 0, "nope");
    let err = store.update_instruction(&missing).await.unwrap_err();
    assert!(matches!(err, agentops_core::StoreError::NotFound));
}

/// Regression test — task-11-review.md Critical. Calling `update_instruction` with
/// `id=B` while submitting a `(phase, title)` already held by `id=A` must raise
/// `Conflict` from the unique index violation rather than silently overwriting A the way
/// `upsert_instruction` would, and both A and B must keep their original values.
#[sqlx::test(migrations = "../../migrations")]
async fn update_that_collides_with_another_rows_identity_is_a_conflict(pool: sqlx::PgPool) {
    let store = PgStore::new(pool);
    let a = ins(Phase::Chat, 0, "foo");
    let b = ins(Phase::Chat, 1, "bar");
    store.upsert_instruction(&a).await.unwrap();
    store.upsert_instruction(&b).await.unwrap();

    let mut b_as_foo = b.clone();
    b_as_foo.title = "foo".into();
    b_as_foo.body = "corrupted".into();
    let err = store.update_instruction(&b_as_foo).await.unwrap_err();
    assert!(
        matches!(err, agentops_core::StoreError::Conflict),
        "got {err:?}"
    );

    let rows = store.instructions_for(&[Phase::Chat]).await.unwrap();
    assert_eq!(rows.len(), 2, "rows must not merge or disappear");
    let a_after = rows.iter().find(|r| r.id == a.id).unwrap();
    assert_eq!(a_after.body, a.body, "A must be untouched");
    let b_after = rows.iter().find(|r| r.id == b.id).unwrap();
    assert_eq!(
        b_after.title, "bar",
        "B must not change from a rejected request either"
    );
    assert_eq!(b_after.body, b.body);
}

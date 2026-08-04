use agentops_agent::prompt::{assemble_system, sort_tools};
use agentops_core::{Instruction, Phase, ToolDef};
use uuid::Uuid;

fn ins(phase: Phase, position: i32, title: &str, body: &str) -> Instruction {
    Instruction {
        id: Uuid::new_v4(),
        phase,
        position,
        title: title.into(),
        body: body.into(),
        enabled: true,
        updated_at: time::OffsetDateTime::now_utc(),
    }
}

fn def(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: "d".into(),
        input_schema: serde_json::json!({"type":"object"}),
    }
}

/// TEST-9B — a system prompt assembled from identical input must be byte-identical.
#[test]
fn test_9b_assembled_prompt_is_byte_identical_for_identical_input() {
    let instructions = vec![
        ins(Phase::All, 0, "tone", "be terse"),
        ins(Phase::Triage, 0, "scope", "check dashboards first"),
    ];
    let first = assemble_system(&instructions, Phase::Triage);
    for _ in 0..10 {
        assert_eq!(assemble_system(&instructions, Phase::Triage), first);
    }
}

/// TEST-9B — a timestamp or a UUID in the prompt invalidates the entire prefix cache
/// (spec Section 8.3). An instruction's id and updated_at must not leak out.
#[test]
fn test_9b_prompt_contains_no_ids_or_timestamps() {
    let i = ins(Phase::Triage, 0, "t", "b");
    let id = i.id.to_string();
    let year = i.updated_at.year().to_string();
    let out = assemble_system(&[i], Phase::Triage);

    assert!(
        !out.contains(&id),
        "an instruction UUID leaked into the prompt"
    );
    assert!(
        !out.contains(&year),
        "a timestamp leaked out (found the year {year}): {out}"
    );
}

/// Instructions preserve the order received. instructions_for already sorted them by
/// `position, title, id`, so they must not be re-sorted here.
#[test]
fn instruction_order_is_preserved_not_re_sorted() {
    let out = assemble_system(
        &[
            ins(Phase::All, 0, "zzz", "first body"),
            ins(Phase::All, 1, "aaa", "second body"),
        ],
        Phase::Triage,
    );
    let a = out
        .find("first body")
        .expect("the first instruction is missing");
    let b = out
        .find("second body")
        .expect("the second instruction is missing");
    assert!(a < b, "the received order was reversed:\n{out}");
}

/// A valid prompt must come out even with no instructions — sending an empty string as
/// system makes Anthropic reject it.
#[test]
fn empty_instructions_still_produce_a_usable_prompt() {
    let out = assemble_system(&[], Phase::Rca);
    assert!(!out.trim().is_empty());
    assert!(out.contains("rca"), "the phase name must be present: {out}");
}

/// The phase name appears in the prompt and must differ per phase.
#[test]
fn each_phase_gets_a_distinct_prompt() {
    let i = vec![ins(Phase::All, 0, "t", "b")];
    let t = assemble_system(&i, Phase::Triage);
    let r = assemble_system(&i, Phase::Rca);
    let m = assemble_system(&i, Phase::Mitigation);
    assert_ne!(t, r);
    assert_ne!(r, m);
    assert_ne!(t, m);
}

/// TEST-9B — tools[] must sort by name regardless of input order.
/// Shuffling the MCP server connection order must yield the same array (spec Section 9).
#[test]
fn test_9b_tools_sort_independently_of_input_order() {
    let want: Vec<String> = vec!["a__x".into(), "b__y".into(), "c__z".into()];

    for input in [
        vec![def("c__z"), def("a__x"), def("b__y")],
        vec![def("b__y"), def("c__z"), def("a__x")],
        vec![def("a__x"), def("b__y"), def("c__z")],
    ] {
        let got: Vec<String> = sort_tools(input).into_iter().map(|d| d.name).collect();
        assert_eq!(got, want);
    }
}

/// The sort must be stable — two identical names (impossible in practice) must not panic.
#[test]
fn sort_tools_is_total_and_does_not_panic_on_duplicates() {
    let got = sort_tools(vec![def("same"), def("same")]);
    assert_eq!(got.len(), 2);
}

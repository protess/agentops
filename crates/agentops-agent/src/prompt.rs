//! System prompt and `tools[]` assembly (spec Sections 8.3 and 9).
//!
//! **This file's output must be byte-identical for identical input.** The prompt cache's
//! prefix renders as `tools` then `system`, so if either is unstable the hit rate goes to
//! zero. The minimum cache unit on Opus 5 is 512 tokens.
//!
//! **The previous phases' summaries (`carried`) do not go here.** They differ per phase,
//! so putting them in the system prompt changes the prefix at every phase transition.
//! Section 8.3's rule to put dynamic context toward the end of `messages` is exactly this,
//! and `carried` goes into the first user message (`crate::phase`).

use agentops_core::{Instruction, Phase, ToolDef};

/// The per-phase system prompt.
///
/// `instructions` arrives in the order `Store::instructions_for` already sorted by
/// `position, title, id`. **Do not re-sort here** — that would override the intent the
/// operator expressed through `position`.
pub fn assemble_system(instructions: &[Instruction], phase: Phase) -> String {
    let mut out = String::new();

    out.push_str("You are an SRE investigation agent.\n");
    out.push_str("Current phase: ");
    out.push_str(phase.as_str());
    out.push_str("\n\n");

    out.push_str(match phase {
        Phase::Triage => "Establish what is broken and how bad it is. Do not propose fixes yet.\n",
        Phase::Rca => "Find the root cause. Cite the evidence you used.\n",
        Phase::Mitigation => {
            "Propose mitigations. If there is nothing to act on, say so explicitly.\n"
        }
        // The investigation loop runs only these three phases (INVESTIGATION_ORDER).
        //
        // `All` is for instruction scope only and never arrives here. **`Chat` is
        // different** — spec Section 6.2 states chat uses the same LLM pipeline while
        // injecting `Phase::Chat` instructions, so plan 3's HTTP surface really does pass
        // this value. At that point the wording below leans toward investigation context
        // and does not suit a chat panel, so plan 3 must supply its own.
        Phase::All | Phase::Chat => "Follow the operator instructions below.\n",
    });

    if !instructions.is_empty() {
        out.push_str("\n## Operator instructions\n");
        for i in instructions {
            // Only title and body. Including id or updated_at invalidates the cache on
            // every request — precisely the mistake Section 8.3 forbids.
            out.push_str("\n### ");
            out.push_str(&i.title);
            out.push('\n');
            out.push_str(&i.body);
            out.push('\n');
        }
    }

    out
}

/// **Sorted by namespaced name.** `tools` renders at the very front of the prompt prefix,
/// so if the order of an MCP server's `tools/list` response or of server connections
/// varies between runs, the cache misses every time (spec Section 9).
///
/// `sort_by` is a stable sort, so the relative order of equally-named entries is preserved too.
///
/// **Premise: `serde_json` is built without `preserve_order`.** `input_schema` is a
/// `serde_json::Value`, and the default `Map` is a `BTreeMap`, so keys serialize sorted
/// and the bytes are the same whatever order an MCP server sends the schema in.
/// With `preserve_order` on, `Map` becomes an `IndexMap` and **preserves insertion
/// order**, at which point the server's response order leaks into the prompt prefix and
/// breaks the cache. This file's tests look only at tool **name** order and would not
/// catch that regression. Check with `cargo tree` after Task 8 adds `rmcp`.
pub fn sort_tools(mut tools: Vec<ToolDef>) -> Vec<ToolDef> {
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

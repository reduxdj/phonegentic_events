//! Shared behavioral guidance for both the Mac on-device agent and the server
//! agent (agent-parity-phase.md item 2).
//!
//! Persona-driven prompt bits stay in [`crate::persona::render_system_prompt`];
//! tool schemas stay in [`crate::tools`]. This module owns the middle layer —
//! delivery, number/dictation handling, and the manager vs stranger privilege
//! rules — so a change lands once and both runtimes get it.
//!
//! The Dart app mirrors [`render_agent_guidance`] as a constant and a parity
//! test asserts the shared block matches the golden vector in
//! `test-vectors/agent_guidance.expected.txt`.

/// Max age (secs) the server agent should trust a synced client working-context
/// snapshot before ignoring it as stale.
pub const AGENT_STATE_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// Soft cap on snapshot JSON bytes the orchestrator accepts / serves.
pub const AGENT_STATE_MAX_BYTES: usize = 8_192;

/// Rolling turn window the client should keep in the snapshot.
pub const AGENT_STATE_MAX_TURNS: usize = 20;

/// Shared behavioral guidance block appended by both agents.
///
/// Runtime-specific material (wake word, local audio, telephony ESL quirks)
/// stays in each agent — only the shared middle goes here.
pub fn render_agent_guidance() -> String {
    let mut b = String::new();
    b.push_str(DELIVERY_AND_DICTATION);
    b.push_str(MANAGER_PRIVILEGE_RULES);
    b
}

/// Spoken-delivery + number/dictation rules (moved from the server agent's
/// former `DELIVERY_GUIDANCE` so the Mac coach shares the same behavior).
const DELIVERY_AND_DICTATION: &str = "\n\n## Delivery (spoken aloud)\n\
Start your reply promptly with a brief, natural lead-in — e.g. \"Sure,\" \
\"Okay,\" \"Got it —\", \"Absolutely,\" — then continue. Vary the lead-in; \
never sound formulaic or robotic. When addressing someone by name, keep the \
name in the same breath (\"Thanks for calling, Patrick!\"), not after a pause.\n\
\n\
## Numbers & dictation\n\
When a caller gives a phone number, code, or spells something, they PAUSE \
between groups — do NOT treat a brief pause as the end of their turn, don't \
talk over them, and don't re-ask while they're still going. Digits may arrive \
as words (\"eight hundred\") or numerals — combine them across pauses into \
one number. A US phone number is 10 digits (11 with a leading 1); once you \
have a complete number, read it back to confirm before acting on it.\n";

/// Manager vs stranger privilege rules. The server agent also injects a
/// call-start identity block based on `manager_e164`; this shared text is the
/// standing rule both runtimes keep in the prompt.
const MANAGER_PRIVILEGE_RULES: &str = "\n\
## Manager privilege rules\n\
The account owner (manager / host) may ask you to act on their behalf: look up \
contacts, send or reply to texts, dial or transfer calls, and start conferences. \
Confirm the specifics (who / what number / what message) and then do it.\n\
\n\
Outside callers are receptionist-scoped: be helpful, answer questions, and take \
a message or relay a request to the owner. Do NOT send texts, dial out, or add \
people to calls on an outside caller's command — those are the owner's \
privileges. If they need such an action, offer to pass the message to the owner.\n";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn guidance_matches_golden_vector() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-vectors/agent_guidance.expected.txt");
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
        assert_eq!(render_agent_guidance(), expected);
    }

    #[test]
    fn guidance_covers_delivery_dictation_and_manager() {
        let g = render_agent_guidance();
        assert!(g.contains("## Delivery (spoken aloud)"));
        assert!(g.contains("## Numbers & dictation"));
        assert!(g.contains("## Manager privilege rules"));
        assert!(g.contains("read it back to confirm"));
        assert!(g.contains("receptionist-scoped"));
    }
}

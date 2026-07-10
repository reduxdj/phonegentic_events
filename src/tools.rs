//! Canonical agent tool definitions — the actions the phone agent can invoke.
//!
//! Single source of truth shared by the Mac app (which mirrors these names) and
//! the Rust server agent (which feeds them to Anthropic tool-use). Server-
//! relevant subset: conference, messaging, calendar, and call control. Manager/
//! outbound/UI-only tools (make_call, tear-sheets, reminders, gmail, voice
//! cloning, call history) live only in the app and are intentionally omitted.

use serde_json::{json, Value};

/// A tool the agent can call. `input_schema` is a JSON Schema object for the
/// parameters (directly usable as an Anthropic tool `input_schema`).
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

impl ToolDef {
    /// Serialize to an Anthropic tool-use definition `{name, description, input_schema}`.
    pub fn to_anthropic(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        })
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// The full server-relevant tool set.
pub fn agent_tools() -> Vec<ToolDef> {
    vec![
        // --- Conference ---
        ToolDef {
            name: "add_conference_participant",
            description: "Put the current call on hold and dial another party to add them to a conference. Requires an explicit, clear request from the host.",
            input_schema: obj(json!({ "number": { "type": "string", "description": "E.164 phone number or SIP URI to dial" } }), &["number"]),
        },
        ToolDef {
            name: "merge_conference",
            description: "Bridge all active call legs into one conference (requires at least two legs). After this you are a CONNECTOR, not a participant — you can no longer hear or be heard, so announce the handoff and stop; do not promise to stay or ask questions afterward.",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "hold_call",
            description: "Hold or resume the current call.",
            input_schema: obj(json!({ "action": { "type": "string", "enum": ["hold", "resume"] } }), &["action"]),
        },
        ToolDef {
            name: "hold_conference_leg",
            description: "Put a specific conference participant on hold.",
            input_schema: obj(json!({ "number": { "type": "string", "description": "The participant's E.164 number" } }), &["number"]),
        },
        ToolDef {
            name: "unhold_conference_leg",
            description: "Resume a specific held conference participant.",
            input_schema: obj(json!({ "number": { "type": "string" } }), &["number"]),
        },
        ToolDef {
            name: "hangup_conference_leg",
            description: "Disconnect a specific conference participant.",
            input_schema: obj(json!({ "number": { "type": "string" } }), &["number"]),
        },
        ToolDef {
            name: "list_conference_legs",
            description: "List each conference leg and its status (ringing, active, held, merged).",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "request_manager_conference",
            description: "When the remote party asks to conference someone in, text the manager to request approval before proceeding.",
            input_schema: obj(json!({ "reason": { "type": "string", "description": "Why the caller wants to conference" } }), &["reason"]),
        },
        // --- Messaging ---
        ToolDef {
            name: "send_sms",
            description: "Send an SMS/MMS. The first message to a new recipient must identify you and include Phonegentic + https://phonegentic.ai.",
            input_schema: obj(json!({
                "to": { "type": "string", "description": "E.164 recipient" },
                "text": { "type": "string" },
                "media_url": { "type": "string", "description": "Optional MMS attachment URL" }
            }), &["to", "text"]),
        },
        ToolDef {
            name: "reply_sms",
            description: "Reply in the currently active SMS conversation (no number needed).",
            input_schema: obj(json!({ "text": { "type": "string" } }), &["text"]),
        },
        // --- Calendar ---
        ToolDef {
            name: "create_calendar_event",
            description: "Create a calendar event. Prefer explicit date/time fields over relative phrasing.",
            input_schema: obj(json!({
                "title": { "type": "string" },
                "date": { "type": "string", "description": "YYYY-MM-DD" },
                "start_time": { "type": "string", "description": "HH:MM 24h" },
                "end_time": { "type": "string", "description": "HH:MM 24h" },
                "description": { "type": "string" },
                "location": { "type": "string" },
                "invitee_name": { "type": "string" },
                "invitee_email": { "type": "string" }
            }), &["title", "date", "start_time"]),
        },
        ToolDef {
            name: "read_calendar",
            description: "Read calendar events for a given date (to check availability).",
            input_schema: obj(json!({ "date": { "type": "string", "description": "YYYY-MM-DD" } }), &["date"]),
        },
        // --- Call control ---
        ToolDef {
            name: "transfer_call",
            description: "Blind-transfer the active call to another number or SIP URI, then disconnect our side. The call leaves you entirely — announce it and stop; you cannot listen or speak afterward.",
            input_schema: obj(json!({ "target": { "type": "string", "description": "E.164 number or SIP URI" } }), &["target"]),
        },
        ToolDef {
            name: "send_dtmf",
            description: "Send DTMF tones to navigate an IVR menu (e.g. \"1\", \"123#\", \"*9\").",
            input_schema: obj(json!({ "tones": { "type": "string" } }), &["tones"]),
        },
        ToolDef {
            name: "check_locale",
            description: "Get the tenant's country code, expected phone-number length, and format rules before dialing.",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "end_call",
            description: "End the current call.",
            input_schema: obj(json!({}), &[]),
        },
    ]
}

/// The subset of [`agent_tools`] enabled for a persona. Empty `enabled` => none
/// (text-only conversational agent); otherwise the tools whose names are listed.
pub fn tools_for(enabled: &[String]) -> Vec<ToolDef> {
    agent_tools()
        .into_iter()
        .filter(|t| enabled.iter().any(|e| e == t.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_set_shape() {
        let all = agent_tools();
        assert_eq!(all.len(), 16);
        // names unique
        let mut names: Vec<&str> = all.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 16);
        // anthropic shape
        let sms = all.iter().find(|t| t.name == "send_sms").unwrap();
        let v = sms.to_anthropic();
        assert_eq!(v["name"], "send_sms");
        assert_eq!(v["input_schema"]["required"], json!(["to", "text"]));
    }

    #[test]
    fn tools_for_filters() {
        let sel = tools_for(&["send_sms".into(), "end_call".into(), "bogus".into()]);
        assert_eq!(sel.len(), 2);
    }
}

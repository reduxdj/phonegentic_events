//! Phonegentic real-time sync wire schema (Kickoff 4).
//!
//! Single source of truth for the events flowing over `WSS /sync/v1` (backed by
//! NATS JetStream). Consumed by the orchestrator (WSS bridge) and the agent
//! (NATS publisher) as a git dependency; the Dart app mirrors these types and is
//! kept honest by the shared JSON vectors in the tests below.
//!
//! Wire shape (per docs/KICKOFF_4_SYNC.md):
//!   Server → Client EVENT:  { schema_version, seq, tenant_id, at, type, payload }
//!   Server → Client CONTROL (ack/error/pong): { type, ... }   (no seq/envelope)
//!   Client → Server COMMAND: { type, ... , request_id? }
//!
//! `type` is the flat discriminant (e.g. "call.transcript.delta", "agent.state").

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current schema version string carried in every server event envelope.
pub mod persona;

pub const SCHEMA_VERSION: &str = "1.0";

/// Who produced a transcript segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Caller,
    Agent,
}

/// Live agent state for the `agent.state` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStateKind {
    Listening,
    Thinking,
    Speaking,
}

// ---------------------------------------------------------------------------
// Server → Client
// ---------------------------------------------------------------------------

/// A sequenced, server-authoritative event. Serializes flat:
/// `{ schema_version, seq, tenant_id, at, type, payload }` — the `type`/`payload`
/// come from the flattened [`ServerEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub schema_version: String,
    /// JetStream stream sequence — clients track it and `resume { since_seq }`.
    pub seq: u64,
    pub tenant_id: String,
    /// RFC 3339 timestamp (kept as a string so the crate stays chrono-free).
    pub at: String,
    #[serde(flatten)]
    pub event: ServerEvent,
}

impl Envelope {
    pub fn new(seq: u64, tenant_id: impl Into<String>, at: impl Into<String>, event: ServerEvent) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            seq,
            tenant_id: tenant_id.into(),
            at: at.into(),
            event,
        }
    }
}

/// Server → Client domain events (the payload of an [`Envelope`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ServerEvent {
    #[serde(rename = "call.started")]
    CallStarted { call_id: String, from: Option<String>, to: Option<String> },
    #[serde(rename = "call.answered")]
    CallAnswered { call_id: String },
    #[serde(rename = "call.transcript.delta")]
    CallTranscriptDelta {
        call_id: String,
        role: Role,
        text: String,
        /// True when this segment is finalized (vs. an interim partial).
        #[serde(rename = "final")]
        is_final: bool,
    },
    #[serde(rename = "call.tool_call")]
    CallToolCall { call_id: String, name: String, args: Value },
    #[serde(rename = "call.ended")]
    CallEnded { call_id: String, reason: Option<String>, duration_secs: Option<u64> },
    #[serde(rename = "agent.state")]
    AgentState { call_id: Option<String>, state: AgentStateKind },
    #[serde(rename = "persona.updated")]
    PersonaUpdated { display_name: String, category: String, opening_line: String, voice_id: Option<String> },
    #[serde(rename = "voice.updated")]
    VoiceUpdated { voice_id: String },
    #[serde(rename = "tenant.error")]
    TenantError { code: String, message: String },
}

/// Server → Client control frames (responses; not sequenced, no envelope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerControl {
    Ack { request_id: String },
    Error { request_id: Option<String>, code: String, message: String },
    Pong,
}

// ---------------------------------------------------------------------------
// Client → Server
// ---------------------------------------------------------------------------

/// Client → Server commands. `request_id` lets the client correlate the
/// server's `ack`/`error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    /// Replay missed events after (re)connect.
    Resume { since_seq: u64 },
    #[serde(rename = "persona.update")]
    PersonaUpdate { persona: Value, request_id: String },
    #[serde(rename = "voice.update")]
    VoiceUpdate { voice_id: String, request_id: String },
    #[serde(rename = "call.interrupt")]
    CallInterrupt { call_id: String, request_id: String },
    Ping,
}

// ---------------------------------------------------------------------------
// NATS subjects (per-tenant). Kept here so orchestrator + agent agree.
// ---------------------------------------------------------------------------

/// Subject the agent/orchestrator PUBLISH server→client events to.
pub fn events_subject(tenant_id: &str) -> String {
    format!("pg.tenant.{tenant_id}.events")
}
/// Subject client→server commands are published to (consumed server-side).
pub fn commands_subject(tenant_id: &str) -> String {
    format!("pg.tenant.{tenant_id}.commands")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Golden wire vectors (the Dart mirror MUST match these) ----------

    #[test]
    fn transcript_delta_envelope_wire_shape() {
        let env = Envelope::new(
            42,
            "01TENANT",
            "2026-07-04T20:00:00Z",
            ServerEvent::CallTranscriptDelta {
                call_id: "c1".into(),
                role: Role::Caller,
                text: "hello".into(),
                is_final: true,
            },
        );
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(
            v,
            json!({
                "schema_version": "1.0",
                "seq": 42,
                "tenant_id": "01TENANT",
                "at": "2026-07-04T20:00:00Z",
                "type": "call.transcript.delta",
                "payload": { "call_id": "c1", "role": "caller", "text": "hello", "final": true }
            })
        );
        // round-trips
        assert_eq!(serde_json::from_value::<Envelope>(v).unwrap(), env);
    }

    #[test]
    fn agent_state_wire_shape() {
        let env = Envelope::new(1, "t", "now", ServerEvent::AgentState { call_id: None, state: AgentStateKind::Speaking });
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["type"], "agent.state");
        assert_eq!(v["payload"], json!({ "call_id": null, "state": "speaking" }));
    }

    #[test]
    fn control_frames_wire_shape() {
        assert_eq!(serde_json::to_value(ServerControl::Ack { request_id: "r1".into() }).unwrap(),
                   json!({ "type": "ack", "request_id": "r1" }));
        assert_eq!(serde_json::to_value(ServerControl::Pong).unwrap(), json!({ "type": "pong" }));
    }

    #[test]
    fn client_commands_wire_shape() {
        assert_eq!(serde_json::to_value(ClientCommand::Resume { since_seq: 7 }).unwrap(),
                   json!({ "type": "resume", "since_seq": 7 }));
        let c = ClientCommand::PersonaUpdate { persona: json!({"display_name":"X"}), request_id: "r2".into() };
        assert_eq!(serde_json::to_value(&c).unwrap(),
                   json!({ "type": "persona.update", "persona": {"display_name":"X"}, "request_id": "r2" }));
        assert_eq!(serde_json::to_value(ClientCommand::Ping).unwrap(), json!({ "type": "ping" }));
    }

    #[test]
    fn subjects() {
        assert_eq!(events_subject("01T"), "pg.tenant.01T.events");
        assert_eq!(commands_subject("01T"), "pg.tenant.01T.commands");
    }
}

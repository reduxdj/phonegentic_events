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

    // --- Context sync (Kickoff 6A) -----------------------------------------
    // The client pushes its contacts + call summaries so the SERVER agent can
    // greet callers by name / recall past calls. Keyed by e164. `source_ts` is
    // the client's last-edit time (unix secs) — the server does source_ts-guarded
    // last-write-wins (a stale/replayed frame never clobbers a newer edit).
    /// Upsert a contact snapshot (create or update by e164).
    #[serde(rename = "context.contact.upsert")]
    ContextContactUpsert {
        e164: String,
        display_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tags: Option<Vec<String>>,
        source_ts: i64,
        request_id: String,
    },
    /// Delete a contact snapshot by e164. (Associated call summaries are kept —
    /// they're e164-keyed and the call happened regardless.)
    #[serde(rename = "context.contact.delete")]
    ContextContactDelete {
        e164: String,
        source_ts: i64,
        request_id: String,
    },
    /// Upsert a call-summary snapshot (idempotent by client-supplied call_id).
    #[serde(rename = "context.call_summary.upsert")]
    ContextCallSummaryUpsert {
        call_id: String,
        e164: String,
        summary_text: String,
        started_at: i64,
        duration_seconds: i64,
        /// "client" | "server" — who handled the call.
        handled_by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topics: Option<Vec<String>>,
        request_id: String,
    },
    /// Nuclear: delete ALL context rows (contacts + summaries) for the tenant.
    /// `require_confirm` MUST be true or the server rejects it (safety).
    #[serde(rename = "context.purge_all")]
    ContextPurgeAll {
        require_confirm: bool,
        request_id: String,
    },

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
    fn context_commands_wire_shape() {
        // contact.upsert — optional notes/tags omitted when None.
        let c = ClientCommand::ContextContactUpsert {
            e164: "+14155551234".into(),
            display_name: "Sarah Chen".into(),
            notes: None,
            tags: None,
            source_ts: 1_783_000_000,
            request_id: "r1".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({
                "type": "context.contact.upsert",
                "e164": "+14155551234",
                "display_name": "Sarah Chen",
                "source_ts": 1_783_000_000,
                "request_id": "r1"
            })
        );
        assert_eq!(serde_json::from_value::<ClientCommand>(serde_json::to_value(&c).unwrap()).unwrap(), c);

        // contact.upsert — with notes + tags present.
        let c2 = ClientCommand::ContextContactUpsert {
            e164: "+1".into(),
            display_name: "N".into(),
            notes: Some("VIP".into()),
            tags: Some(vec!["lead".into(), "vip".into()]),
            source_ts: 1,
            request_id: "r".into(),
        };
        let v2 = serde_json::to_value(&c2).unwrap();
        assert_eq!(v2["notes"], json!("VIP"));
        assert_eq!(v2["tags"], json!(["lead", "vip"]));

        assert_eq!(
            serde_json::to_value(ClientCommand::ContextContactDelete {
                e164: "+14155551234".into(),
                source_ts: 42,
                request_id: "r2".into(),
            })
            .unwrap(),
            json!({ "type": "context.contact.delete", "e164": "+14155551234", "source_ts": 42, "request_id": "r2" })
        );

        let cs = ClientCommand::ContextCallSummaryUpsert {
            call_id: "01CALL".into(),
            e164: "+14155551234".into(),
            summary_text: "Discussed pricing.".into(),
            started_at: 1_783_000_100,
            duration_seconds: 320,
            handled_by: "server".into(),
            topics: Some(vec!["pricing".into()]),
            request_id: "r3".into(),
        };
        let vcs = serde_json::to_value(&cs).unwrap();
        assert_eq!(vcs["type"], "context.call_summary.upsert");
        assert_eq!(vcs["call_id"], "01CALL");
        assert_eq!(vcs["handled_by"], "server");
        assert_eq!(vcs["topics"], json!(["pricing"]));
        assert_eq!(serde_json::from_value::<ClientCommand>(vcs).unwrap(), cs);

        assert_eq!(
            serde_json::to_value(ClientCommand::ContextPurgeAll {
                require_confirm: true,
                request_id: "r4".into(),
            })
            .unwrap(),
            json!({ "type": "context.purge_all", "require_confirm": true, "request_id": "r4" })
        );
    }

    #[test]
    fn subjects() {
        assert_eq!(events_subject("01T"), "pg.tenant.01T.events");
        assert_eq!(commands_subject("01T"), "pg.tenant.01T.commands");
    }
}

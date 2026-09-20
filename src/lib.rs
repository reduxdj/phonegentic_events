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
pub mod agent_guidance;
pub mod calendar;
pub mod messaging;
pub mod persona;
pub mod phone_number;
pub mod speech_numbers;
pub mod tools;

pub use agent_guidance::{
    render_agent_guidance, AGENT_STATE_MAX_AGE_SECS, AGENT_STATE_MAX_BYTES, AGENT_STATE_MAX_TURNS,
};
pub use phone_number::{parse_spoken_phone, resolve_dialable, ParsedPhone};
pub use speech_numbers::format_numbers_for_speech;

pub const SCHEMA_VERSION: &str = "1.0";

/// Who produced a transcript segment.
///
/// `Human` is the phone owner speaking on a call a person answered — the far
/// lane of the droplet's caller-leg transcription fork when no agent is in the
/// room. It exists because that lane used to go out as `Agent` ("our side")
/// and every client badged the owner's own words as "AI" (2026-09-18 and
/// 2026-09-20 on agent-dev). Clients from 1.0.141 map `human` to "You";
/// older clients fall back to `agent`, which is what they showed before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Caller,
    Agent,
    Human,
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
    /// A call recording is staged in Spaces and ready for the Mac app to pull
    /// (offline-call-recordings). `call_id` is the archive id when known.
    #[serde(rename = "recording.ready")]
    RecordingReady {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_s: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes: Option<i64>,
        created_at: i64,
    },
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

    /// Sent to a PROVIDER device: run this and reply with `capability.result`.
    /// A control frame (not a `ServerEvent`) precisely because control frames
    /// are unsequenced and never replayed — see the note on `CapabilityRequest`.
    #[serde(rename = "capability.invoke")]
    CapabilityInvoke { request_id: String, capability: String, args: Value },

    /// Sent to the REQUESTER: the provider's answer, or a failure the requester
    /// should surface verbatim. Failure codes travel as `Error`:
    /// `capability_unavailable` (no provider online),
    /// `capability_timeout` (provider took too long),
    /// `capability_failed` (provider ran it and it errored).
    #[serde(rename = "capability.response")]
    CapabilityResponse { request_id: String, ok: bool, content: String },
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

    /// Upsert the client's rolling working-context snapshot (recent turns +
    /// open task/intent + scratch notes) so the SERVER agent can continue a
    /// task if the device goes offline mid-work. Per-tenant LWW by `source_ts`.
    /// `snapshot` is compact JSON; orchestrator enforces
    /// [`AGENT_STATE_MAX_BYTES`]. See agent-parity-phase.md item 3.
    #[serde(rename = "context.agent_state.upsert")]
    ContextAgentStateUpsert {
        snapshot: Value,
        source_ts: i64,
        request_id: String,
    },

    // --- Capability delegation (remote-web-search) -------------------------
    // A device that cannot run something asks another of the SAME tenant's
    // devices to run it. Built for `web_search`, which needs a real Chrome over
    // CDP and therefore exists only on desktop — iOS and the droplet agent can
    // only delegate. Kept generic so later desktop-only powers (read a file,
    // search my mail) reuse one router rather than growing a message each.
    //
    // These are NOT durable. The orchestrator delivers them straight to the
    // target device's socket, never through JetStream: a request from a 09:00
    // call replayed onto a laptop reconnecting at 17:00 would run a stale query
    // for nobody.
    /// Ask another device of this tenant to run a capability.
    #[serde(rename = "capability.request")]
    CapabilityRequest {
        /// "web_search" today. Free-form so a new capability needs no crate
        /// change on the REQUESTER side — only the provider must understand it.
        capability: String,
        /// Capability-specific arguments. For web_search: `{"query": "..."}`.
        args: Value,
        request_id: String,
    },

    /// A provider returning the outcome. `request_id` echoes the invoke.
    #[serde(rename = "capability.result")]
    CapabilityResult {
        request_id: String,
        ok: bool,
        /// Formatted, LLM-ready text on success; a human-readable reason on
        /// failure. Deliberately opaque: the provider formats, the requester
        /// hands it to the model unchanged, and no repo has to model a result
        /// shape that will keep changing.
        content: String,
    },

    /// Announce what this device can do. Sent once right after connect and
    /// again whenever the set changes (user toggles the integration off).
    #[serde(rename = "capability.announce")]
    CapabilityAnnounce {
        /// e.g. `["web_search"]`. Empty is legal — "provider of nothing".
        capabilities: Vec<String>,
        /// "macos" | "ios" | "linux". Diagnostics and provider tie-breaking.
        platform: String,
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
    fn human_role_serialises_lowercase_and_round_trips() {
        let ev = ServerEvent::CallTranscriptDelta {
            call_id: "c1".into(),
            role: Role::Human,
            text: "hey amber".into(),
            is_final: true,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["payload"]["role"], "human");
        let back: ServerEvent = serde_json::from_value(v).unwrap();
        assert!(matches!(back, ServerEvent::CallTranscriptDelta { role: Role::Human, .. }));
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

    /// Capability delegation: the wire shapes three repos must agree on
    /// byte-for-byte (Rust orchestrator, Rust agent, hand-mirrored Dart client).
    /// A rename here is a silent cross-repo break, so pin the JSON explicitly.
    #[test]
    fn capability_frames_are_stable_on_the_wire() {
        let req = ClientCommand::CapabilityRequest {
            capability: "web_search".into(),
            args: json!({ "query": "weather in Portland" }),
            request_id: "r1".into(),
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({
                "type": "capability.request",
                "capability": "web_search",
                "args": { "query": "weather in Portland" },
                "request_id": "r1"
            })
        );
        assert_eq!(serde_json::from_value::<ClientCommand>(serde_json::to_value(&req).unwrap()).unwrap(), req);

        let res = ClientCommand::CapabilityResult {
            request_id: "r1".into(),
            ok: true,
            content: "Google results for \"weather\": ...".into(),
        };
        assert_eq!(serde_json::to_value(&res).unwrap()["type"], "capability.result");
        assert_eq!(serde_json::from_value::<ClientCommand>(serde_json::to_value(&res).unwrap()).unwrap(), res);

        let ann = ClientCommand::CapabilityAnnounce {
            capabilities: vec!["web_search".into()],
            platform: "macos".into(),
            request_id: "r2".into(),
        };
        assert_eq!(
            serde_json::to_value(&ann).unwrap(),
            json!({
                "type": "capability.announce",
                "capabilities": ["web_search"],
                "platform": "macos",
                "request_id": "r2"
            })
        );
        assert_eq!(serde_json::from_value::<ClientCommand>(serde_json::to_value(&ann).unwrap()).unwrap(), ann);

        // An announce with NO capabilities is legal and must survive the round
        // trip — it is how a device says "I turned the integration off".
        let none = ClientCommand::CapabilityAnnounce {
            capabilities: vec![],
            platform: "ios".into(),
            request_id: "r3".into(),
        };
        assert_eq!(serde_json::from_value::<ClientCommand>(serde_json::to_value(&none).unwrap()).unwrap(), none);

        let inv = ServerControl::CapabilityInvoke {
            request_id: "r1".into(),
            capability: "web_search".into(),
            args: json!({ "query": "q" }),
        };
        assert_eq!(
            serde_json::to_value(&inv).unwrap(),
            json!({
                "type": "capability.invoke",
                "request_id": "r1",
                "capability": "web_search",
                "args": { "query": "q" }
            })
        );

        let resp = ServerControl::CapabilityResponse {
            request_id: "r1".into(),
            ok: false,
            content: "Web search isn't available right now.".into(),
        };
        assert_eq!(serde_json::to_value(&resp).unwrap()["type"], "capability.response");
    }

    /// google_search must be in the shared catalogue or the server agent cannot
    /// advertise it (tool_defs() resolves names through `tools_for`).
    #[test]
    fn google_search_is_catalogued() {
        let defs = crate::tools::tools_for(&["google_search".to_string()]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "google_search");
        let a = defs[0].to_anthropic();
        assert_eq!(a["input_schema"]["required"], json!(["query"]));
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

        let asu = ClientCommand::ContextAgentStateUpsert {
            snapshot: json!({
                "turns": [{"role": "user", "text": "Call Bob about the invoice"}],
                "open_task": "Call Bob about invoice #44",
                "scratch_notes": "follow up Tuesday"
            }),
            source_ts: 1_783_000_200,
            request_id: "r5".into(),
        };
        let vasu = serde_json::to_value(&asu).unwrap();
        assert_eq!(vasu["type"], "context.agent_state.upsert");
        assert_eq!(vasu["source_ts"], 1_783_000_200);
        assert_eq!(vasu["snapshot"]["open_task"], "Call Bob about invoice #44");
        assert_eq!(serde_json::from_value::<ClientCommand>(vasu).unwrap(), asu);
    }

    #[test]
    fn subjects() {
        assert_eq!(events_subject("01T"), "pg.tenant.01T.events");
        assert_eq!(commands_subject("01T"), "pg.tenant.01T.commands");
    }
}

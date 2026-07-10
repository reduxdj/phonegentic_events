//! Canonical calendar-event model, mirroring the Mac app's `CalendarEvent` so
//! the agent can capture/represent events consistently across client + server.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventSource {
    Local,
    Calendly,
    Google,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_calendar_event_id: Option<String>,
    pub source: EventSource,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Unix seconds.
    pub start_time: i64,
    /// Unix seconds.
    pub end_time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitee_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitee_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// When set, switch to this job function when the event starts (app feature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_function_id: Option<i64>,
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "active".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_shape() {
        let e = CalendarEvent {
            google_calendar_event_id: None,
            source: EventSource::Local,
            title: "Callback".into(),
            description: None,
            start_time: 1_783_000_000,
            end_time: 1_783_003_600,
            invitee_name: Some("Patrick".into()),
            invitee_email: None,
            location: None,
            job_function_id: None,
            status: "active".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["source"], "local");
        assert_eq!(v["title"], "Callback");
        assert_eq!(v["invitee_name"], "Patrick");
        assert!(v.get("description").is_none());
        assert_eq!(serde_json::from_value::<CalendarEvent>(v).unwrap(), e);
    }
}

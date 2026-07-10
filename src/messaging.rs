//! Canonical SMS/MMS message + conversation models, mirroring the Mac app's
//! `SmsMessage`/`SmsConversation` so client and server agree on the wire shape
//! (used by the server messaging feature: inbound webhook -> agent -> send).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmsDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmsStatus {
    Queued,
    Sent,
    Delivered,
    Failed,
    Received,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmsMessage {
    /// Carrier/provider message id (Telnyx), if assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_type: String, // "telnyx"
    pub from: String,
    pub to: String,
    pub text: String,
    pub direction: SmsDirection,
    pub status: SmsStatus,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// Provider id of the message this one replies to (thread linkage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmsConversation {
    pub remote_phone: String,
    pub local_phone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<SmsMessage>,
    #[serde(default)]
    pub unread_count: u32,
    #[serde(default)]
    pub total_messages: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sms_wire_shape() {
        let m = SmsMessage {
            provider_id: None,
            provider_type: "telnyx".into(),
            from: "+14155551234".into(),
            to: "+15034472755".into(),
            text: "hi".into(),
            direction: SmsDirection::Inbound,
            status: SmsStatus::Received,
            created_at: 1_783_000_000,
            media_urls: vec![],
            error_reason: None,
            reply_to_provider_id: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["direction"], "inbound");
        assert_eq!(v["status"], "received");
        assert_eq!(v["from"], "+14155551234");
        assert!(v.get("media_urls").is_none()); // empty skipped
        // round-trips
        assert_eq!(serde_json::from_value::<SmsMessage>(v).unwrap(), m);
        let _ = json!({});
    }
}

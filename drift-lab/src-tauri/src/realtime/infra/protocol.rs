//! Phoenix Channels v1.0.0 envelope builders + frame parser. Pure
//! functions, no I/O, no Tokio. Lives in `infra` (not `domain`) because
//! the JSON shapes are a *wire* concern — they're tied to the Supabase
//! transport adapter, not to the realtime domain model.
//!
//! Every magic value is cited to its line in `supabase.py` so the two
//! sides can't silently drift.

use serde_json::{json, Value};

/// `phx_join` envelope. Verbatim from `supabase.py:322-338`.
pub fn join_envelope(topic: &str, join_ref: &str, api_key: &str) -> Value {
    json!({
        "topic": topic,
        "event": "phx_join",
        "ref": join_ref,
        "join_ref": join_ref,
        "payload": {
            "config": {
                "broadcast": { "ack": false, "self": false },
                "presence":  { "key": "" },
                "postgres_changes": [],
                "private": false
            },
            "access_token": api_key
        }
    })
}

/// Heartbeat envelope. Verbatim from `supabase.py:340-347`.
pub fn heartbeat_envelope(reference: &str) -> Value {
    json!({
        "topic": "phoenix",
        "event": "heartbeat",
        "payload": {},
        "ref": reference
    })
}

/// `phx_leave` envelope — Phoenix-level graceful channel exit. realtime-js
/// sends this before closing the underlying WebSocket so the server can
/// clean up the channel subscription (presence, broadcast routing)
/// promptly instead of waiting for the heartbeat-timeout sweep. The
/// reference matches the channel join's `join_ref` so the server can
/// thread the leave to the right subscription.
pub fn leave_envelope(topic: &str, join_ref: &str, reference: &str) -> Value {
    json!({
        "topic": topic,
        "event": "phx_leave",
        "join_ref": join_ref,
        "ref": reference,
        "payload": {}
    })
}

/// True iff the frame is a broadcast on `topic` whose inner
/// `payload.event` matches `event_filter` (an empty filter accepts all
/// inner events). Returns the *wrapped* event dict (i.e.
/// `frame.payload.payload`) for forwarding to the aggregator.
///
/// Returns `None` for heartbeat replies, presence diffs, and anything
/// else we don't care about.
pub fn extract_broadcast_payload<'a>(
    frame: &'a Value,
    topic: &str,
    event_filter: &str,
) -> Option<&'a Value> {
    if frame.get("topic")?.as_str()? != topic {
        return None;
    }
    if frame.get("event")?.as_str()? != "broadcast" {
        return None;
    }
    let payload = frame.get("payload")?;
    if payload.get("type")?.as_str()? != "broadcast" {
        return None;
    }
    if !event_filter.is_empty() && payload.get("event")?.as_str()? != event_filter {
        return None;
    }
    payload.get("payload")
}

/// Parse a `phx_reply` response to a join: returns `Some(true)` for
/// `status=ok`, `Some(false)` for `status=error`, `None` if the frame is
/// not a reply to our join (different `ref`, or not a `phx_reply` at all).
/// The optional `reason` is the server's error payload, for surfacing
/// in `RealtimeError::ChannelRejected`.
pub fn parse_join_reply(frame: &Value, join_ref: &str) -> Option<JoinReply> {
    if frame.get("event")?.as_str()? != "phx_reply" {
        return None;
    }
    if frame.get("ref")?.as_str()? != join_ref {
        return None;
    }
    let status = frame.pointer("/payload/status")?.as_str()?;
    match status {
        "ok" => Some(JoinReply::Ok),
        _ => {
            let reason = frame
                .pointer("/payload/response")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "(no response payload)".into());
            Some(JoinReply::Error(reason))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinReply {
    Ok,
    Error(String),
}

/// Format the topic name we send in every envelope. Mirrors
/// `supabase.py:148` (`'realtime:%s' % (channel,)`).
pub fn topic_for_channel(channel: &str) -> String {
    format!("realtime:{channel}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_envelope_carries_access_token() {
        let env = join_envelope("realtime:foo", "1", "jwt");
        assert_eq!(env["payload"]["access_token"], "jwt");
        assert_eq!(env["topic"], "realtime:foo");
        assert_eq!(env["event"], "phx_join");
    }

    #[test]
    fn broadcast_payload_extraction_matches_topic_and_event() {
        let frame = json!({
            "topic": "realtime:foo",
            "event": "broadcast",
            "payload": {
                "type": "broadcast",
                "event": "profiler-event",
                "payload": {"name": "x"}
            }
        });
        let p = extract_broadcast_payload(&frame, "realtime:foo", "profiler-event");
        assert_eq!(p.unwrap(), &json!({"name": "x"}));
    }

    #[test]
    fn empty_event_filter_accepts_any_inner_event() {
        let frame = json!({
            "topic": "realtime:foo",
            "event": "broadcast",
            "payload": {
                "type": "broadcast",
                "event": "anything",
                "payload": {"name": "x"}
            }
        });
        assert!(extract_broadcast_payload(&frame, "realtime:foo", "").is_some());
    }

    #[test]
    fn wrong_topic_is_dropped() {
        let frame = json!({
            "topic": "realtime:bar",
            "event": "broadcast",
            "payload": {"type": "broadcast", "event": "x", "payload": {}}
        });
        assert!(extract_broadcast_payload(&frame, "realtime:foo", "").is_none());
    }

    #[test]
    fn heartbeat_reply_is_dropped() {
        let frame = json!({
            "topic": "phoenix",
            "event": "phx_reply",
            "payload": {"status": "ok"}
        });
        assert!(extract_broadcast_payload(&frame, "realtime:foo", "").is_none());
    }

    #[test]
    fn parse_join_reply_recognises_ok() {
        let frame = json!({
            "topic": "realtime:foo",
            "event": "phx_reply",
            "payload": {"status": "ok", "response": {}},
            "ref": "1"
        });
        assert_eq!(parse_join_reply(&frame, "1"), Some(JoinReply::Ok));
    }

    #[test]
    fn parse_join_reply_carries_error_reason() {
        let frame = json!({
            "topic": "realtime:foo",
            "event": "phx_reply",
            "payload": {"status": "error", "response": {"reason": "nope"}},
            "ref": "1"
        });
        match parse_join_reply(&frame, "1") {
            Some(JoinReply::Error(s)) => assert!(s.contains("nope")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_join_reply_ignores_wrong_ref() {
        let frame = json!({
            "event": "phx_reply",
            "payload": {"status": "ok"},
            "ref": "99"
        });
        assert_eq!(parse_join_reply(&frame, "1"), None);
    }

    #[test]
    fn leave_envelope_targets_channel_topic_with_join_ref() {
        let env = leave_envelope("realtime:foo", "1", "leave");
        assert_eq!(env["topic"], "realtime:foo");
        assert_eq!(env["event"], "phx_leave");
        assert_eq!(env["join_ref"], "1");
        assert_eq!(env["ref"], "leave");
        assert_eq!(env["payload"], json!({}));
    }
}

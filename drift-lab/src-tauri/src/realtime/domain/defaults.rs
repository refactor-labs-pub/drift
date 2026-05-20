//! Compile-time constants the publisher and subscriber must agree on.
//!
//! Each value is cited to its line in the Python publisher
//! (`drift-profiler-python/.../sinks/supabase.py`) so the two sides can't
//! silently drift. Touch these only when the publisher changes.

/// Phoenix Channels protocol version. v1.0.0 is the named-field envelope
/// (`{topic, event, payload, ref, join_ref}`); v2.0.0 is positional arrays.
/// `realtime-js` and `realtime-py` both default to v1.0.0 → easier to debug
/// on the wire. Mirrors `supabase.py:70`.
pub const VSN: &str = "1.0.0";

/// Channel name the publisher uses when no `channel=` kwarg or
/// `SUPABASE_REALTIME_CHANNEL` env var is set. Mirrors `supabase.py:82`.
pub const DEFAULT_CHANNEL: &str = "drift-profiler-events";

/// Inner `payload.event` field the publisher sets when no `event_name=`
/// kwarg is supplied. Subscriber drops broadcasts whose `payload.event`
/// doesn't match this. Mirrors `supabase.py:137`.
pub const DEFAULT_EVENT_NAME: &str = "profiler-event";

/// Heartbeat cadence on the `phoenix` topic. The server times out at ~60 s
/// of silence; `realtime-js` sends every 30 s. We pick 25 s for safety.
/// Mirrors `supabase.py:74`.
pub const HEARTBEAT_SECS: u64 = 25;

/// Reconnect backoff steps (seconds). Matches `realtime-js`'s schedule —
/// see `supabase.py:79`. Each failed connect bumps one step; success
/// resets to 0.
pub const BACKOFF_STEPS_SECS: &[u64] = &[1, 2, 5, 10];

/// Settings → "Test Connection" wall-clock budget. The connect+join cycle
/// should finish within one RTT for a healthy Supabase project; 5 s is
/// generous enough to ride out a slow handshake without leaving the user
/// staring at a hung button.
pub const TEST_CONNECTION_BUDGET_SECS: u64 = 5;

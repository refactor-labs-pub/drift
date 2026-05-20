//! Domain layer for the realtime subsystem.
//!
//! Everything here is pure: no `tauri`, no `tokio`, no I/O. The types and
//! pure functions in this module are the bedrock the rest of the
//! subsystem builds on. The upper layers (`app`, `infra`, Tauri commands)
//! depend on this module; this module depends on nothing else in the
//! crate.

pub mod defaults;
pub mod errors;
pub mod profile;
pub mod settings;
pub mod stream_config;
pub mod url;

pub use errors::RealtimeError;
pub use profile::{ProfileId, RealtimeProfile, RealtimeSettings};
pub use settings::RealtimeConfig;
pub use stream_config::{resolve, EffectiveStreamConfig, StreamOverrides};

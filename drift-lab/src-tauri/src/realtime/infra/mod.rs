//! Infrastructure adapters — the I/O-touching implementations of the
//! ports in [`super::ports`]. Each adapter is the *only* place its
//! particular external dependency (tungstenite, tokio fs, tauri-plugin-
//! store) is allowed to appear.

pub mod aggregator_sink;
pub mod jsonl_sink;
pub mod profile_repository;
pub mod protocol;
pub mod repository;
pub mod tee_sink;
pub mod tungstenite_transport;
pub mod vault;

pub use aggregator_sink::{AggregatorHandle, AggregatorSink};
pub use jsonl_sink::JsonlSinkFactory;
pub use profile_repository::AppConfigProfileRepository;
pub use repository::AppConfigSettingsRepository;
pub use tee_sink::TeeSink;
pub use tungstenite_transport::TungsteniteTransport;
#[allow(deprecated)] // legacy constant — kept for the pre-multi-profile call sites.
pub use vault::REALTIME_API_KEY;
pub use vault::{
    namespaced_realtime_api_key_for, FileApiKeyVault, LEGACY_REALTIME_API_KEY,
};

//! Supabase Realtime subscriber subsystem.
//!
//! Layered per "Clean Architecture" — dependencies point inward:
//!
//! ```text
//!   commands (Tauri)  ──►  app (use cases)  ──►  ports (traits)  ◄──  infra (adapters)
//!                                                       ▲
//!                                                       │
//!                                                  domain (pure types)
//! ```
//!
//! * [`domain`] — types and pure functions. No `tauri`, `tokio`,
//!   `tungstenite`, or filesystem in this module.
//! * [`ports`] — the traits use cases depend on.
//! * [`app`] — use cases. One verb per file. No I/O imports; only
//!   trait objects from [`ports`].
//! * [`infra`] — concrete adapters that implement the traits. The only
//!   place WSS, JSON-over-disk, and the Tauri store are allowed to
//!   appear inside this subsystem.
//!
//! The Tauri command shims live OUTSIDE this module (in
//! `event_source_commands.rs` and `commands.rs`) and instantiate use
//! cases + adapters per invocation.

pub mod app;
pub mod domain;
pub mod infra;
pub mod ports;

/// Install the process-wide `rustls` [`CryptoProvider`] required by
/// every WSS connect this subsystem performs. **Must be called once at
/// process startup**, before any `TungsteniteTransport` method runs.
///
/// `rustls` 0.23 stopped auto-selecting a provider; the first
/// `connect_async` call without one panics in `tokio_tungstenite` with
/// "Could not automatically determine the process-level CryptoProvider".
/// In the Tauri app that panic is swallowed by the async-command
/// machinery, which presents as "the Test button does nothing".
///
/// Eager (vs lazy / OnceLock-in-transport) because:
/// * Both the Test command AND the Subscribe (Start Listening) command
///   do TLS — installing once at startup removes any ordering hazard.
/// * One install at process boot is cheaper than `OnceLock::get_or_init`
///   on every transport call.
/// * Out-of-band code in the same process (the auto-updater, a future
///   reqwest client, etc.) also picks up the same provider.
///
/// Idempotent — returns `Ok(())` even if a provider is already
/// installed. Returns `Err` only if rustls can't recognise the
/// installed provider, which would be a build-misconfiguration bug.
pub fn init() -> Result<(), &'static str> {
    // `install_default` returns Err on the second call (first wins).
    // We treat that as success — what matters is that *some* provider
    // is installed by the time we return.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        Err("rustls CryptoProvider failed to install")
    } else {
        Ok(())
    }
}

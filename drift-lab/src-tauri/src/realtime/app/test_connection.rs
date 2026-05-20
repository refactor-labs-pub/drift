//! Use case: validate creds + channel reachability with one
//! connect+join+close cycle.
//!
//! One method, [`TestConnectionUseCase::execute`], serves BOTH UI surfaces
//! (Settings form and Active Scan debug button). Inputs are uniformly
//! optional: each `None`/empty falls through to the saved value (URL from
//! the repo, JWT from the vault, channel from settings → publisher
//! default). Settings passes whatever the form has typed; Active Scan
//! passes nothing and gets the saved values transparently.
//!
//! The use case never panics on a missing field — it returns
//! [`RealtimeError::InvalidUrl`] / [`RealtimeError::MissingApiKey`]
//! so the renderer can render a precise hint instead of a generic
//! failure.

use tokio_util::sync::CancellationToken;

use crate::realtime::domain::{
    resolve, RealtimeConfig, RealtimeError, StreamOverrides,
};
use crate::realtime::ports::{
    ApiKeyVault, RealtimeTransport, TestConnectionOutcome, TestStage,
};

pub struct TestConnectionUseCase<'a, V, T>
where
    V: ApiKeyVault + ?Sized,
    T: RealtimeTransport + ?Sized,
{
    /// Pre-resolved baseline config. Passed in by the command shim
    /// (typically the active profile projected via `RealtimeProfile::into`)
    /// so the use case stays unaware of where it came from. The
    /// `inputs.supabase_url` override wins over this; `inputs.channel`
    /// likewise.
    settings: RealtimeConfig,
    vault: &'a V,
    transport: &'a T,
}

/// Optional per-test overrides. Each `None` means "use the saved value".
/// `Some("")` is treated as `None` (the UI emits empty strings, not
/// nulls, from empty text inputs).
#[derive(Debug, Default, Clone)]
pub struct TestInputs {
    pub supabase_url: Option<String>,
    pub api_key: Option<String>,
    pub channel: Option<String>,
}

impl<'a, V, T> TestConnectionUseCase<'a, V, T>
where
    V: ApiKeyVault + ?Sized,
    T: RealtimeTransport + ?Sized,
{
    pub fn new(settings: RealtimeConfig, vault: &'a V, transport: &'a T) -> Self {
        Self {
            settings,
            vault,
            transport,
        }
    }

    /// Run one test. Any unset input falls through to the saved value:
    ///
    /// | Input          | Fallback chain                                       |
    /// |----------------|------------------------------------------------------|
    /// | `supabase_url` | settings.url                                         |
    /// | `api_key`      | vault.read() (SecretStore)                           |
    /// | `channel`      | settings.default_channel → publisher default        |
    ///
    /// `on_progress` is invoked at each transport stage so the renderer
    /// can swap the button label. `cancel` aborts mid-flight; a Stop
    /// click surfaces as [`TestConnectionOutcome::Failed`] carrying
    /// [`RealtimeError::Cancelled`] (typed result, not a thrown error).
    pub async fn execute(
        &self,
        inputs: TestInputs,
        on_progress: Box<dyn Fn(TestStage) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<TestConnectionOutcome, RealtimeError> {
        // ----- resolve url ----------------------------------------------------
        let supabase_url =
            first_non_empty(inputs.supabase_url.as_deref(), &self.settings.url)
                .ok_or_else(|| RealtimeError::InvalidUrl(String::new()))?;

        // ----- resolve key ---------------------------------------------------
        let api_key = match first_non_empty_owned(inputs.api_key) {
            Some(k) => k,
            None => self.vault.read()?, // bubbles `MissingApiKey`
        };

        // ----- resolve channel ----------------------------------------------
        // `resolve()` handles the channel + event_filter fall-through chain
        // already, including the publisher-side default. The test endpoint
        // ignores `event_filter` (it doesn't gate on inner events).
        let overrides = StreamOverrides::from_options(inputs.channel, None);
        let mut effective = resolve(&self.settings, &api_key, &overrides)?;
        // Override the URL with whatever the user typed (or fall through
        // to the saved one). `resolve()` keyed off `settings.url`; we want
        // the typed-in value to win when present.
        effective.supabase_url = supabase_url;

        self.transport
            .test_connection(&effective, on_progress, cancel)
            .await
    }
}

/// Borrow-friendly "pick the first non-empty after trimming" helper.
fn first_non_empty<'a>(primary: Option<&'a str>, fallback: &'a str) -> Option<String> {
    if let Some(p) = primary {
        let t = p.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let t = fallback.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Same idea but consumes the optional (`api_key` is sensitive — we
/// don't want a borrowed reference lingering).
fn first_non_empty_owned(primary: Option<String>) -> Option<String> {
    primary.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

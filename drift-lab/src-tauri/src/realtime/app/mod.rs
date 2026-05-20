//! Application layer — the use cases. Each module is one verb the
//! subsystem can be asked to perform. Dependencies on infrastructure
//! flow through the trait objects defined in [`super::ports`]; nothing
//! in here may import from `super::infra` directly.

pub mod profile_use_cases;
pub mod start_stream;
pub mod test_connection;
pub mod update_settings;

pub use profile_use_cases::{
    ActivateProfileUseCase, DeleteProfileUseCase, ListProfilesUseCase, SaveProfileInput,
    SaveProfileUseCase,
};
pub use start_stream::{StartStreamUseCase, StreamPlan};
pub use test_connection::{TestConnectionUseCase, TestInputs};
pub use update_settings::UpdateSettingsUseCase;

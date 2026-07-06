//! The iOS build pipeline. The [`IosBuilder`](super::IosBuilder) methods are
//! split across submodules by concern:
//!
//! - [`sim`] — Simulator builds: compile, install, and launch in Simulator.app
//! - [`device`] — device builds, code-signing, and identity/profile checks
//! - [`profile`] — provisioning-profile fetch, caching, and validity checks
//! - [`registration`] — device discovery, registration, and target resolution
//! - [`bundle`] — `.app` bundle assembly, asset compilation, and bin paths

mod bundle;
mod device;
mod profile;
mod registration;
mod sim;

pub use profile::decode_profile;

/// Simulator or device target — selects the SDK, triple suffix, and
/// platform keys that go into the iOS `Info.plist`.
enum IosTarget {
    Simulator,
    Device,
}

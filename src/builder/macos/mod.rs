//! The macOS build pipeline. The [`MacosBuilder`](super::MacosBuilder) methods
//! are split across submodules by concern:
//!
//! - [`steps`] core pipeline stages (compile, embed, package, notarize)
//! - [`bundle`] app and extension bundle assembly
//! - [`capabilities`] required capabilities, pinned-profile checks, and profile
//!   opt-in, without network calls
//! - [`profile`] strudel-managed Developer ID provisioning profiles
//! - [`provision`] App Store Connect changes for profiles and capabilities
//! - [`plan`] confirmation text for those changes
//! - [`sign`] code signing
//! - [`notarize`] DMG notarization submission, polling, and resume
//! - [`validators`] signing/notarization credential preflight checks

mod bundle;
mod capabilities;
mod notarize;
mod plan;
mod profile;
mod provision;
mod sign;
mod steps;
mod validators;

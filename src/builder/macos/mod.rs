//! The macOS build pipeline. The [`MacosBuilder`](super::MacosBuilder) methods
//! are split across submodules by concern:
//!
//! - [`steps`] core pipeline stages (compile, embed, package, notarize)
//! - [`bundle`] app and extension bundle assembly
//! - [`sign`] code signing
//! - [`notarize`] DMG notarization submission, polling, and resume
//! - [`validators`] signing/notarization credential preflight checks

mod bundle;
mod notarize;
mod sign;
mod steps;
mod validators;

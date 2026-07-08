//! The macOS build pipeline. The [`MacosBuilder`](super::MacosBuilder) methods
//! are split across submodules by concern:
//!
//! - [`steps`] the pipeline stages (compile, assemble, embed, sign, package)
//! - [`notarize`] DMG notarization submission, polling, and resume
//! - [`validators`] signing/notarization credential preflight checks

mod notarize;
mod steps;
mod validators;

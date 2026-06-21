//! Keychain handling for code-signing identities. Two distinct mechanisms live
//! here, kept in separate modules because they have opposite lifetimes:
//!
//! - [`temp`]: an ephemeral keychain built from a supplied `APPLE_CERTIFICATE`
//!   (the paid / CI path). Torn down when the build finishes so nothing is left
//!   on the machine.
//! - [`dev`]: a persistent keychain holding a free-provisioning development
//!   identity, reused across builds until the 7-day certificate expires.
//!
//! Credential preflight (which secrets a real `run` needs) lives alongside the
//! other config checks in [`super::validators`].

pub mod dev;
mod temp;

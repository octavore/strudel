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
//!
//! [`parse_identity_line`] parses `security find-identity` output and is
//! shared by [`temp`] and [`super::ios::device`]'s identity checks.

pub mod dev;
mod temp;

use std::sync::LazyLock;

use regex::Regex;

/// Parse one line of `security find-identity ... -p codesigning` output into
/// its hash and quoted name, e.g. `  1) 0123...CDEF "Developer ID Application:
/// Acme Inc (TEAMID)"` -> `("0123...CDEF", "Developer ID Application: Acme
/// Inc (TEAMID)")`.
pub(in crate::builder) fn parse_identity_line(line: &str) -> Option<(&str, &str)> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^\s*\d+\) (\S+) "([^"]+)""#).unwrap());
    let caps = RE.captures(line)?;
    Some((caps.get(1)?.as_str(), caps.get(2)?.as_str()))
}

mod anisette;
mod dev_services;
mod grandslam;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Serializable session returned by `login`; persist this to avoid
/// re-authentication. Password is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub apple_id: String,
    pub dsid: String,
    pub gs_token: String,
}

/// A team visible on the Apple developer portal.
#[derive(Debug, Clone)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub status: String,
}

/// A device registered on the Apple developer portal.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub udid: String,
    pub model: String,
    pub platform: String,
}

/// The output of `fetch_development_profile`.
/// `mobileprovision` is the raw 7-day `.mobileprovision` bytes.
/// `cert_der` + `key_pem` form the signing identity that must be imported to a
/// keychain.
pub struct DevProfile {
    pub mobileprovision: Vec<u8>,
    pub cert_der: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Entry point for all Apple ID and free-provisioning operations.
/// Construct once and reuse across calls.
pub struct AppleId {
    anisette: anisette::AnisetteProvider,
    agent: ureq::Agent,
}

impl AppleId {
    pub fn new() -> Result<Self> {
        let anisette = anisette::AnisetteProvider::new()?;
        // Apple's GSA endpoint uses a cert chain not in standard root stores.
        // SRP provides application-layer mutual authentication, so skipping
        // TLS verification here doesn't weaken the login security.
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .disable_verification(true)
                    .build(),
            )
            .build()
            .new_agent();
        Ok(AppleId { anisette, agent })
    }

    /// Returns the stable device identifier sourced from AKDevice (or a
    /// generated fallback).
    pub fn device_id(&self) -> &str {
        &self.anisette.device_id
    }

    /// Sign in with an Apple ID and password. The `two_factor` callback is
    /// invoked if the account has two-factor authentication enabled; it
    /// should prompt the user for the 6-digit code and return it. The
    /// callback is not called for accounts without 2FA.
    pub fn login(
        &self,
        apple_id: &str,
        password: &str,
        two_factor: impl FnMut() -> Result<String>,
    ) -> Result<Session> {
        grandslam::login(&self.agent, &self.anisette, apple_id, password, two_factor)
    }

    /// List teams associated with the authenticated Apple ID.
    pub fn list_teams(&self, session: &Session) -> Result<Vec<Team>> {
        dev_services::DevServicesClient::new(&self.agent, &self.anisette, session).list_teams()
    }

    /// List devices registered under the given team.
    pub fn list_devices(&self, session: &Session, team_id: &str) -> Result<Vec<Device>> {
        dev_services::DevServicesClient::new(&self.agent, &self.anisette, session)
            .list_devices(team_id)
    }

    /// Register a device with the given name and UDID under the team.
    pub fn add_device(
        &self,
        session: &Session,
        team_id: &str,
        name: &str,
        udid: &str,
    ) -> Result<()> {
        dev_services::DevServicesClient::new(&self.agent, &self.anisette, session)
            .add_device(team_id, name, udid)
    }

    /// Ensure an app ID for `bundle_id` exists under the team, creating it if
    /// needed.
    pub fn ensure_app_id(
        &self,
        session: &Session,
        team_id: &str,
        bundle_id: &str,
        name: &str,
    ) -> Result<()> {
        dev_services::DevServicesClient::new(&self.agent, &self.anisette, session)
            .ensure_app_id(team_id, bundle_id, name)?;
        Ok(())
    }

    /// Generate a new RSA keypair and CSR, submit to Apple, and download the
    /// 7-day development profile for `bundle_id`. The returned `DevProfile`
    /// contains the raw `.mobileprovision` bytes, the signed certificate in
    /// DER format, and the private key in PEM format.
    ///
    /// `cached_identity` is a previously issued `(cert_der, key_pem)` pair; it
    /// is reused when the certificate is still valid on the portal (not
    /// expired or revoked), avoiding a needless revoke+reissue on every
    /// profile refresh.
    ///
    /// Free accounts allow only one development certificate, so when a new cert
    /// is needed any existing cert must be revoked first. `confirm_revoke` is
    /// invoked with a human-readable description of each cert that would be
    /// revoked; returning `Ok(false)` aborts without revoking. It is not called
    /// when the cached cert is reused or when there are no existing certs.
    pub fn fetch_development_profile(
        &self,
        session: &Session,
        team_id: &str,
        bundle_id: &str,
        udids: &[&str],
        cached_identity: Option<(&[u8], &[u8])>,
        confirm_revoke: impl FnMut(&[String]) -> Result<bool>,
    ) -> Result<DevProfile> {
        dev_services::DevServicesClient::new(&self.agent, &self.anisette, session)
            .fetch_development_profile(team_id, bundle_id, udids, cached_identity, confirm_revoke)
    }
}

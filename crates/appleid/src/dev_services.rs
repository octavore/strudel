use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::anisette::AnisetteProvider;
use crate::{DevProfile, Device, Session, Team};

// Account-level actions (listTeams) live directly under the protocol path;
// resource actions (devices, app IDs, certs, profiles) are under `ios/`.
const BASE_URL: &str = "https://developerservices2.apple.com/services/QH65B2";
const CLIENT_ID: &str = "XABBG36SBA";
const PROTOCOL_VERSION: &str = "QH65B2";

pub struct DevServicesClient<'a> {
    agent: &'a ureq::Agent,
    anisette: &'a AnisetteProvider,
    session: &'a Session,
}

impl<'a> DevServicesClient<'a> {
    pub fn new(
        agent: &'a ureq::Agent,
        anisette: &'a AnisetteProvider,
        session: &'a Session,
    ) -> Self {
        DevServicesClient {
            agent,
            anisette,
            session,
        }
    }

    pub fn list_teams(&self) -> Result<Vec<Team>> {
        let resp = self.post("listTeams", plist::Dictionary::new())?;
        let teams = resp
            .get("teams")
            .and_then(|v| v.as_array())
            .context("missing teams array in listTeams response")?;

        teams
            .iter()
            .map(|t| {
                let d = t.as_dictionary().context("team entry is not a dict")?;
                Ok(Team {
                    id: plist_str(d, "teamId")?,
                    name: plist_str(d, "name")?,
                    status: plist_str(d, "status").unwrap_or_else(|_| "active".to_string()),
                })
            })
            .collect()
    }

    pub fn list_devices(&self, team_id: &str) -> Result<Vec<Device>> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );

        let resp = self.post("ios/listDevices", body)?;
        let devices = resp
            .get("devices")
            .and_then(|v| v.as_array())
            .context("missing devices array in listDevices response")?;

        devices
            .iter()
            .map(|d| {
                let d = d.as_dictionary().context("device entry is not a dict")?;
                Ok(Device {
                    id: plist_str(d, "deviceId")?,
                    name: plist_str(d, "name")?,
                    udid: plist_str(d, "deviceNumber")?,
                    model: plist_str(d, "model").unwrap_or_default(),
                    platform: plist_str(d, "deviceClass").unwrap_or_else(|_| "ios".to_string()),
                })
            })
            .collect()
    }

    pub fn add_device(&self, team_id: &str, name: &str, udid: &str) -> Result<()> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        body.insert("name".to_string(), plist::Value::String(name.to_string()));
        body.insert(
            "deviceNumber".to_string(),
            plist::Value::String(udid.to_string()),
        );
        self.post("ios/addDevice", body)?;
        Ok(())
    }

    pub fn ensure_app_id(&self, team_id: &str, bundle_id: &str, name: &str) -> Result<String> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );

        let resp = self.post("ios/listAppIds", body)?;
        let ids = resp
            .get("appIds")
            .and_then(|v| v.as_array())
            .context("missing appIds in listAppIds response")?;

        for entry in ids {
            let d = entry.as_dictionary().context("appId entry is not a dict")?;
            let identifier = plist_str(d, "identifier").unwrap_or_default();
            if identifier == bundle_id {
                return plist_str(d, "appIdId");
            }
        }

        // App ID not found - create it.
        let mut add_body = plist::Dictionary::new();
        add_body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        add_body.insert(
            "identifier".to_string(),
            plist::Value::String(bundle_id.to_string()),
        );
        add_body.insert("name".to_string(), plist::Value::String(name.to_string()));

        let add_resp = self.post("ios/addAppId", add_body)?;
        let created = add_resp
            .get("appId")
            .and_then(|v| v.as_dictionary())
            .context("missing appId in addAppId response")?;
        plist_str(created, "appIdId")
    }

    // Generates a keypair + CSR internally, submits to Apple, returns DER cert +
    // private key PEM.
    fn provision_cert(
        &self,
        team_id: &str,
        cached: Option<(&[u8], &[u8])>,
        confirm_revoke: &mut impl FnMut(&[String]) -> Result<bool>,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        // Reuse the cached cert+key when it's still valid on the portal, so we
        // don't needlessly revoke our own (and Xcode's) cert on every 7-day
        // profile refresh. The cert is valid ~1 year; only the profile rotates.
        if let Some((cert_der, key_pem)) = cached
            && self.cached_cert_usable(team_id, cert_der)?
        {
            return Ok((cert_der.to_vec(), key_pem.to_vec()));
        }

        let machine_id = &self.anisette.device_id;
        let machine_name = hostname();

        // Revoke any existing development cert so a fresh CSR is accepted
        // (free accounts allow only one).
        self.revoke_existing_certs(team_id, confirm_revoke)?;

        // Generate private key + CSR with openssl.
        let tmp = tempfile::Builder::new()
            .prefix("strudel-")
            .tempdir()
            .context("creating temp dir for CSR")?;
        let key_path = tmp.path().join("key.pem");
        let csr_path = tmp.path().join("csr.pem");

        // Capture output (rather than inheriting stdio) so openssl's key-gen
        // progress doesn't leak to the terminal; surface stderr only on failure.
        let output = Command::new("openssl")
            .args([
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                key_path.to_str().unwrap(),
                "-out",
                csr_path.to_str().unwrap(),
                "-subj",
                "/CN=Apple Development",
            ])
            .output()
            .context("running openssl to generate keypair + CSR")?;
        if !output.status.success() {
            bail!(
                "openssl req failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let csr_pem = std::fs::read_to_string(&csr_path).context("reading generated CSR")?;
        let key_pem = std::fs::read(&key_path).context("reading generated private key")?;

        // Submit CSR.
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        body.insert("csrContent".to_string(), plist::Value::String(csr_pem));
        body.insert(
            "machineId".to_string(),
            plist::Value::String(machine_id.clone()),
        );
        body.insert(
            "machineName".to_string(),
            plist::Value::String(machine_name),
        );

        let resp = self.post("ios/submitDevelopmentCSR", body)?;

        // submitDevelopmentCSR returns only metadata (no cert bytes), so read
        // the signed certificate back from listAllDevelopmentCerts by id.
        let certificate_id = resp
            .get("certRequest")
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("certificateId"))
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .context("missing certificateId in submitDevelopmentCSR response")?;
        let cert_der = self.download_development_cert(team_id, &certificate_id)?;

        Ok((cert_der, key_pem))
    }

    /// Read a freshly issued development certificate's DER bytes back from
    /// `listAllDevelopmentCerts`, matched by `certificate_id`.
    /// `submitDevelopmentCSR` only returns metadata, so the actual
    /// `certContent` must be fetched here.
    fn download_development_cert(&self, team_id: &str, certificate_id: &str) -> Result<Vec<u8>> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );

        let resp = self.post("ios/listAllDevelopmentCerts", body)?;
        let certs = resp
            .get("certificates")
            .and_then(|v| v.as_array())
            .context("missing certificates in listAllDevelopmentCerts response")?;

        let cert = certs
            .iter()
            .filter_map(|c| c.as_dictionary())
            .find(|d| d.get("certificateId").and_then(|v| v.as_string()) == Some(certificate_id))
            .with_context(|| {
                format!("certificate {certificate_id} not found in listAllDevelopmentCerts")
            })?;

        // certContent is the raw DER bytes (plist <data>).
        cert.get("certContent")
            .and_then(|v| v.as_data())
            .map(|b| b.to_vec())
            .with_context(|| format!("certificate {certificate_id} has no certContent"))
    }

    /// Whether a cached `cert_der` can still be reused: it isn't expiring soon
    /// and is still present on the portal (i.e. hasn't been revoked, including
    /// by Xcode or another machine).
    fn cached_cert_usable(&self, team_id: &str, cert_der: &[u8]) -> Result<bool> {
        if !cert_not_expiring(cert_der)? {
            return Ok(false);
        }

        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        let resp = self.post("ios/listAllDevelopmentCerts", body)?;
        let present = resp
            .get("certificates")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_dictionary())
            .any(|d| d.get("certContent").and_then(|v| v.as_data()) == Some(cert_der));
        Ok(present)
    }

    fn revoke_existing_certs(
        &self,
        team_id: &str,
        confirm_revoke: &mut impl FnMut(&[String]) -> Result<bool>,
    ) -> Result<()> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );

        let resp = self.post("ios/listAllDevelopmentCerts", body)?;
        let Some(certs) = resp.get("certificates").and_then(|v| v.as_array()) else {
            return Ok(());
        };
        if certs.is_empty() {
            return Ok(());
        }

        // Free accounts allow only a single development certificate, so any
        // existing cert (created by Xcode or an earlier run) blocks a new CSR
        // with error 7460. We can't reuse a cert whose private key we don't
        // hold, so revoke every development cert to make room for a fresh one.
        // Revoking can invalidate Xcode's signing, so confirm with the caller.
        let descriptions: Vec<String> = certs
            .iter()
            .filter_map(|c| c.as_dictionary())
            .map(|d| plist_str(d, "name").unwrap_or_else(|_| "development certificate".into()))
            .collect();

        if !confirm_revoke(&descriptions)? {
            bail!(
                "Declined to revoke the existing development certificate. Free \
                 provisioning allows only one, so a new profile cannot be issued \
                 without revoking it."
            );
        }

        for cert in certs {
            let Some(d) = cert.as_dictionary() else {
                continue;
            };
            let Ok(serial) = plist_str(d, "serialNumber") else {
                continue;
            };
            let mut revoke_body = plist::Dictionary::new();
            revoke_body.insert(
                "teamId".to_string(),
                plist::Value::String(team_id.to_string()),
            );
            revoke_body.insert("serialNumber".to_string(), plist::Value::String(serial));
            self.post("ios/revokeDevelopmentCert", revoke_body)?;
        }

        Ok(())
    }

    pub fn fetch_development_profile(
        &self,
        team_id: &str,
        bundle_id: &str,
        _udids: &[&str],
        cached_identity: Option<(&[u8], &[u8])>,
        mut confirm_revoke: impl FnMut(&[String]) -> Result<bool>,
    ) -> Result<DevProfile> {
        let (cert_der, key_pem) =
            self.provision_cert(team_id, cached_identity, &mut confirm_revoke)?;
        let app_id_id = self.ensure_app_id(team_id, bundle_id, &bundle_id_display(bundle_id))?;

        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        body.insert("appIdId".to_string(), plist::Value::String(app_id_id));

        let resp = self.post("ios/downloadTeamProvisioningProfile", body)?;
        let profile_dict = resp
            .get("provisioningProfile")
            .and_then(|v| v.as_dictionary())
            .context("missing provisioningProfile in downloadTeamProvisioningProfile response")?;

        // encodedProfile is the raw .mobileprovision bytes (plist <data>).
        let mobileprovision = profile_dict
            .get("encodedProfile")
            .and_then(|v| v.as_data())
            .context("missing encodedProfile in provisioningProfile")?
            .to_vec();

        Ok(DevProfile {
            mobileprovision,
            cert_der,
            key_pem,
        })
    }

    fn post(&self, action: &str, mut body: plist::Dictionary) -> Result<plist::Dictionary> {
        let url = format!("{BASE_URL}/{action}.action");

        body.insert(
            "clientId".to_string(),
            plist::Value::String(CLIENT_ID.to_string()),
        );
        body.insert(
            "protocolVersion".to_string(),
            plist::Value::String(PROTOCOL_VERSION.to_string()),
        );
        body.insert(
            "requestId".to_string(),
            plist::Value::String(Uuid::new_v4().to_string().to_uppercase()),
        );

        let mut buf = Vec::new();
        plist::Value::Dictionary(body)
            .to_writer_xml(&mut buf)
            .context("serializing dev services request plist")?;

        // "-2" yields the machine-level OTP; a real DSID returns empty OTP
        // headers unless the account is provisioned in AOSKit on this Mac.
        let anisette_hdrs = self.anisette.headers("-2")?;

        // The developer-services portal authenticates with the adsid in
        // X-Apple-I-Identity-Id and the app-specific Xcode token (obtained at
        // login via the `apptokens` exchange) in X-Apple-GS-Token.The raw GsIdmsToken
        // is rejected with resultCode 1100.
        let mut builder = self
            .agent
            .post(&url)
            .header("Content-Type", "text/x-xml-plist")
            .header("Accept", "text/x-xml-plist")
            .header("Accept-Language", "en-us")
            .header("User-Agent", "Xcode")
            .header("X-Apple-App-Info", "com.apple.gs.xcode.auth")
            .header("X-Xcode-Version", "11.2 (11B41)")
            .header("X-Apple-I-Identity-Id", &self.session.dsid)
            .header("X-Apple-GS-Token", &self.session.gs_token);
        for (k, v) in &anisette_hdrs {
            builder = builder.header(k, v);
        }

        let mut resp = builder
            .send(&buf[..])
            .map_err(|e| anyhow::anyhow!("dev services network error: {e}"))?;

        let status = resp.status();
        let body_str = resp
            .body_mut()
            .read_to_string()
            .context("reading dev services response body")?;

        if !status.is_success() {
            bail!("dev services {action} returned HTTP {status}: {body_str}");
        }

        let val = plist::Value::from_reader(Cursor::new(body_str.as_bytes()))
            .context("parsing dev services response plist")?;

        let dict = val
            .into_dictionary()
            .context("dev services response is not a dict")?;

        check_result_code(&dict, action)?;

        Ok(dict)
    }
}

/// True if the DER certificate is valid and not expiring within ~1 day, via
/// `openssl x509 -checkend` (exit 0 = not expiring).
fn cert_not_expiring(cert_der: &[u8]) -> Result<bool> {
    let mut child = Command::new("openssl")
        .args(["x509", "-inform", "DER", "-noout", "-checkend", "86400"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("running openssl x509 -checkend")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(cert_der).ok();
    }
    let status = child.wait().context("waiting for openssl x509 -checkend")?;
    Ok(status.success())
}

fn check_result_code(dict: &plist::Dictionary, action: &str) -> Result<()> {
    let code = dict
        .get("resultCode")
        .and_then(|v| v.as_signed_integer())
        .unwrap_or(0);
    if code != 0 {
        let msg = dict
            .get("userString")
            .or_else(|| dict.get("resultString"))
            .and_then(|v| v.as_string())
            .unwrap_or("unknown error");
        if code == 1100 {
            bail!("Apple session expired. Run `strudel login` to sign in again.");
        }
        bail!("dev services {action} failed (code {code}): {msg}");
    }
    Ok(())
}

fn plist_str(dict: &plist::Dictionary, key: &str) -> Result<String> {
    dict.get(key)
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
        .with_context(|| format!("missing or non-string field: {key}"))
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Mac".to_string())
}

fn bundle_id_display(bundle_id: &str) -> String {
    bundle_id
        .split('.')
        .next_back()
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .unwrap_or_else(|| bundle_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_id_display_extracts_last_component() {
        assert_eq!(bundle_id_display("com.example.myapp"), "Myapp");
        assert_eq!(bundle_id_display("MyApp"), "MyApp");
        assert_eq!(bundle_id_display("com.co.App"), "App");
    }

    #[test]
    fn check_result_code_passes_on_zero() {
        let mut d = plist::Dictionary::new();
        d.insert("resultCode".to_string(), plist::Value::Integer(0.into()));
        assert!(check_result_code(&d, "test").is_ok());
    }

    #[test]
    fn check_result_code_fails_on_nonzero() {
        let mut d = plist::Dictionary::new();
        d.insert(
            "resultCode".to_string(),
            plist::Value::Integer((-1001i64).into()),
        );
        d.insert(
            "userString".to_string(),
            plist::Value::String("Device limit reached".to_string()),
        );
        let err = check_result_code(&d, "addDevice").unwrap_err();
        assert!(format!("{err}").contains("Device limit reached"), "{err}");
    }
}

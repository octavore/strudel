use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::anisette::AnisetteProvider;
use crate::{DevProfile, Device, Session, Team};

// Account-level actions (listTeams) live directly under the protocol path;
// resource actions (devices, app IDs, certs, profiles) are under `ios/`.
const BASE_URL: &str = "https://developerservices2.apple.com/services/QH65B2";
const CLIENT_ID: &str = "XABBG36SBA";
const PROTOCOL_VERSION: &str = "QH65B2";

#[derive(Serialize)]
struct EmptyBody {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamIdBody {
    team_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddDeviceBody {
    team_id: String,
    name: String,
    device_number: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddAppIdBody {
    team_id: String,
    identifier: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitCsrBody {
    team_id: String,
    csr_content: String,
    machine_id: String,
    machine_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevokeCertBody {
    team_id: String,
    serial_number: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProfileBody {
    team_id: String,
    app_id_id: String,
}

#[derive(Deserialize)]
struct ListTeamsResponse {
    teams: Vec<TeamEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamEntry {
    team_id: String,
    name: String,
    #[serde(default = "default_status")]
    status: String,
}

#[derive(Deserialize)]
struct ListDevicesResponse {
    devices: Vec<DeviceEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceEntry {
    device_id: String,
    name: String,
    device_number: String,
    #[serde(default)]
    model: String,
    #[serde(default = "default_platform")]
    device_class: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAppIdsResponse {
    app_ids: Vec<AppIdEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppIdEntry {
    identifier: String,
    app_id_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddAppIdResponse {
    app_id: AppIdEntry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitCsrResponse {
    cert_request: CertRequestEntry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CertRequestEntry {
    certificate_id: String,
}

#[derive(Deserialize)]
struct ListCertsResponse {
    certificates: Vec<CertEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CertEntry {
    #[serde(default)]
    certificate_id: String,
    #[serde(default, with = "serde_bytes")]
    cert_content: Vec<u8>,
    #[serde(default = "default_cert_name")]
    name: String,
    #[serde(default)]
    serial_number: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProfileResponse {
    provisioning_profile: ProvisioningProfileEntry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisioningProfileEntry {
    #[serde(with = "serde_bytes")]
    encoded_profile: Vec<u8>,
}

fn default_status() -> String {
    "active".to_string()
}

fn default_platform() -> String {
    "ios".to_string()
}

fn default_cert_name() -> String {
    "development certificate".to_string()
}

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
        let resp: ListTeamsResponse = parse(self.post("listTeams", EmptyBody {})?)?;
        Ok(resp
            .teams
            .into_iter()
            .map(|t| Team {
                id: t.team_id,
                name: t.name,
                status: t.status,
            })
            .collect())
    }

    pub fn list_devices(&self, team_id: &str) -> Result<Vec<Device>> {
        let resp: ListDevicesResponse = parse(self.post(
            "ios/listDevices",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;
        Ok(resp
            .devices
            .into_iter()
            .map(|d| Device {
                id: d.device_id,
                name: d.name,
                udid: d.device_number,
                model: d.model,
                platform: d.device_class,
            })
            .collect())
    }

    pub fn add_device(&self, team_id: &str, name: &str, udid: &str) -> Result<()> {
        self.post(
            "ios/addDevice",
            AddDeviceBody {
                team_id: team_id.into(),
                name: name.into(),
                device_number: udid.into(),
            },
        )?;
        Ok(())
    }

    pub fn ensure_app_id(&self, team_id: &str, bundle_id: &str, name: &str) -> Result<String> {
        let resp: ListAppIdsResponse = parse(self.post(
            "ios/listAppIds",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;

        if let Some(existing) = resp.app_ids.iter().find(|a| a.identifier == bundle_id) {
            return Ok(existing.app_id_id.clone());
        }

        let resp: AddAppIdResponse = parse(self.post(
            "ios/addAppId",
            AddAppIdBody {
                team_id: team_id.into(),
                identifier: bundle_id.into(),
                name: name.into(),
            },
        )?)?;
        Ok(resp.app_id.app_id_id)
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
        let resp: SubmitCsrResponse = parse(self.post(
            "ios/submitDevelopmentCSR",
            SubmitCsrBody {
                team_id: team_id.into(),
                csr_content: csr_pem,
                machine_id: machine_id.clone(),
                machine_name,
            },
        )?)?;

        // submitDevelopmentCSR returns only metadata (no cert bytes), so read
        // the signed certificate back from listAllDevelopmentCerts by id.
        let cert_der =
            self.download_development_cert(team_id, &resp.cert_request.certificate_id)?;

        Ok((cert_der, key_pem))
    }

    /// Read a freshly issued development certificate's DER bytes back from
    /// `listAllDevelopmentCerts`, matched by `certificate_id`.
    /// `submitDevelopmentCSR` only returns metadata, so the actual
    /// `certContent` must be fetched here.
    fn download_development_cert(&self, team_id: &str, certificate_id: &str) -> Result<Vec<u8>> {
        let resp: ListCertsResponse = parse(self.post(
            "ios/listAllDevelopmentCerts",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;

        let cert = resp
            .certificates
            .into_iter()
            .find(|c| c.certificate_id == certificate_id)
            .with_context(|| {
                format!("certificate {certificate_id} not found in listAllDevelopmentCerts")
            })?;

        if cert.cert_content.is_empty() {
            bail!("certificate {certificate_id} has no certContent");
        }
        Ok(cert.cert_content)
    }

    /// Whether a cached `cert_der` can still be reused: it isn't expiring soon
    /// and is still present on the portal (i.e. hasn't been revoked, including
    /// by Xcode or another machine).
    fn cached_cert_usable(&self, team_id: &str, cert_der: &[u8]) -> Result<bool> {
        if !cert_not_expiring(cert_der)? {
            return Ok(false);
        }
        let resp: ListCertsResponse = parse(self.post(
            "ios/listAllDevelopmentCerts",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;
        let present = resp
            .certificates
            .iter()
            .any(|c| c.cert_content.as_slice() == cert_der);
        Ok(present)
    }

    fn revoke_existing_certs(
        &self,
        team_id: &str,
        confirm_revoke: &mut impl FnMut(&[String]) -> Result<bool>,
    ) -> Result<()> {
        let resp: ListCertsResponse = parse(self.post(
            "ios/listAllDevelopmentCerts",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;

        if resp.certificates.is_empty() {
            return Ok(());
        }

        // Free accounts allow only a single development certificate, so any
        // existing cert (created by Xcode or an earlier run) blocks a new CSR
        // with error 7460. We can't reuse a cert whose private key we don't
        // hold, so revoke every development cert to make room for a fresh one.
        // Revoking can invalidate Xcode's signing, so confirm with the caller.
        let descriptions: Vec<String> = resp.certificates.iter().map(|c| c.name.clone()).collect();

        if !confirm_revoke(&descriptions)? {
            bail!(
                "Declined to revoke the existing development certificate. Free \
                 provisioning allows only one, so a new profile cannot be issued \
                 without revoking it."
            );
        }

        for cert in &resp.certificates {
            if cert.serial_number.is_empty() {
                continue;
            }
            self.post(
                "ios/revokeDevelopmentCert",
                RevokeCertBody {
                    team_id: team_id.into(),
                    serial_number: cert.serial_number.clone(),
                },
            )?;
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

        let resp: DownloadProfileResponse = parse(self.post(
            "ios/downloadTeamProvisioningProfile",
            DownloadProfileBody {
                team_id: team_id.into(),
                app_id_id,
            },
        )?)?;

        Ok(DevProfile {
            mobileprovision: resp.provisioning_profile.encoded_profile,
            cert_der,
            key_pem,
        })
    }

    fn post(&self, action: &str, body: impl Serialize) -> Result<plist::Dictionary> {
        let url = format!("{BASE_URL}/{action}.action");

        let val = plist::to_value(&body).context("serializing dev services request body")?;
        let mut dict = val
            .into_dictionary()
            .context("request body must serialize to a plist dictionary")?;

        dict.insert("clientId".to_string(), CLIENT_ID.into());
        dict.insert("protocolVersion".to_string(), PROTOCOL_VERSION.into());
        dict.insert(
            "requestId".to_string(),
            Uuid::new_v4().to_string().to_uppercase().into(),
        );

        let mut buf = Vec::new();
        plist::Value::Dictionary(dict)
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

fn parse<T: serde::de::DeserializeOwned>(dict: plist::Dictionary) -> Result<T> {
    plist::from_value(&plist::Value::Dictionary(dict)).context("deserializing response")
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
        d.insert("resultCode".to_string(), (0i64).into());
        assert!(check_result_code(&d, "test").is_ok());
    }

    #[test]
    fn check_result_code_fails_on_nonzero() {
        let mut d = plist::Dictionary::new();
        d.insert("resultCode".to_string(), (-1001i64).into());
        d.insert("userString".to_string(), "Device limit reached".into());
        let err = check_result_code(&d, "addDevice").unwrap_err();
        assert!(format!("{err}").contains("Device limit reached"), "{err}");
    }
}

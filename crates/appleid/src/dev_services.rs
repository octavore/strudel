use std::io::Cursor;
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use uuid::Uuid;

use crate::anisette::AnisetteProvider;
use crate::{DevProfile, Device, Session, Team};

const BASE_URL: &str = "https://developerservices2.apple.com/services/QH65B2/ios";
// Fixed widget key Xcode sends for developer services.
const WIDGET_KEY: &str = "83545bf919730e51dbfba24e7e8a78d2";
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

        let resp = self.post("listDevices", body)?;
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
        self.post("addDevice", body)?;
        Ok(())
    }

    pub fn ensure_app_id(&self, team_id: &str, bundle_id: &str, name: &str) -> Result<String> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );

        let resp = self.post("listAppIds", body)?;
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

        let add_resp = self.post("addAppId", add_body)?;
        let created = add_resp
            .get("appId")
            .and_then(|v| v.as_dictionary())
            .context("missing appId in addAppId response")?;
        plist_str(created, "appIdId")
    }

    // Generates a keypair + CSR internally, submits to Apple, returns DER cert +
    // private key PEM.
    fn provision_cert(&self, team_id: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let machine_id = &self.anisette.device_id;
        let machine_name = hostname();

        // Revoke any existing certs for this machine to stay under the 2-cert limit.
        self.revoke_existing_certs(team_id, machine_id)?;

        // Generate private key + CSR with openssl.
        let tmp = tempfile::Builder::new()
            .prefix("strudel-")
            .tempdir()
            .context("creating temp dir for CSR")?;
        let key_path = tmp.path().join("key.pem");
        let csr_path = tmp.path().join("csr.pem");

        let status = Command::new("openssl")
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
            .status()
            .context("running openssl to generate keypair + CSR")?;
        if !status.success() {
            bail!("openssl req failed with status {status}");
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

        let resp = self.post("submitDevelopmentCSR", body)?;
        let cert_dict = resp
            .get("certRequest")
            .and_then(|v| v.as_dictionary())
            .context("missing certRequest in submitDevelopmentCSR response")?;

        let cert_b64 =
            plist_str(cert_dict, "certContent").context("missing certContent in certRequest")?;
        let cert_der = BASE64
            .decode(cert_b64.replace(['\n', '\r', ' '], ""))
            .context("decoding base64 cert content")?;

        Ok((cert_der, key_pem))
    }

    fn revoke_existing_certs(&self, team_id: &str, machine_id: &str) -> Result<()> {
        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        body.insert(
            "machineId".to_string(),
            plist::Value::String(machine_id.to_string()),
        );

        let resp = self.post("listAllDevelopmentCerts", body)?;
        let certs = match resp.get("certRequests").and_then(|v| v.as_array()) {
            Some(c) => c,
            None => return Ok(()),
        };

        for cert in certs {
            let d = match cert.as_dictionary() {
                Some(d) => d,
                None => continue,
            };
            let cert_id = match plist_str(d, "serialNumber") {
                Ok(id) => id,
                Err(_) => continue,
            };
            // Only revoke certs issued for this machine.
            let this_machine = plist_str(d, "machineId").unwrap_or_default();
            if this_machine != machine_id {
                continue;
            }
            let mut revoke_body = plist::Dictionary::new();
            revoke_body.insert(
                "teamId".to_string(),
                plist::Value::String(team_id.to_string()),
            );
            revoke_body.insert("serialNumber".to_string(), plist::Value::String(cert_id));
            self.post("revokeDevelopmentCert", revoke_body)?;
        }

        Ok(())
    }

    pub fn fetch_development_profile(
        &self,
        team_id: &str,
        bundle_id: &str,
        _udids: &[&str],
    ) -> Result<DevProfile> {
        let (cert_der, key_pem) = self.provision_cert(team_id)?;
        let app_id_id = self.ensure_app_id(team_id, bundle_id, &bundle_id_display(bundle_id))?;

        let mut body = plist::Dictionary::new();
        body.insert(
            "teamId".to_string(),
            plist::Value::String(team_id.to_string()),
        );
        body.insert("appIdId".to_string(), plist::Value::String(app_id_id));

        let resp = self.post("downloadTeamProvisioningProfile", body)?;
        let profile_dict = resp
            .get("provisioningProfile")
            .and_then(|v| v.as_dictionary())
            .context("missing provisioningProfile in downloadTeamProvisioningProfile response")?;

        let profile_b64 = plist_str(profile_dict, "encodedProfile")
            .context("missing encodedProfile in provisioningProfile")?;
        let mobileprovision = BASE64
            .decode(profile_b64.replace(['\n', '\r', ' '], ""))
            .context("decoding base64 mobileprovision")?;

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

        let anisette_hdrs = self.anisette.headers(&self.session.dsid)?;

        let mut builder = self
            .agent
            .post(&url)
            .header("Content-Type", "text/x-xml-plist")
            .header("Accept", "text/x-xml-plist")
            .header("User-Agent", "Xcode")
            .header("X-Apple-GS-Token", &self.session.gs_token)
            .header("X-Apple-Widget-Key", WIDGET_KEY);
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

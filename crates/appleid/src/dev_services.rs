use std::collections::BTreeMap;
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

/// Request body of `cloudContainer/listCloudContainers`. Confirmed by a
/// read-only probe, except for paging. `pageNumber` and `pageSize` come from
/// other clients. Whether `pageNumber` starts at 0 or 1 is unverified.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListCloudContainersBody {
    team_id: String,
    page_number: u32,
    page_size: u32,
    sort: &'static str,
}

/// Request body of `cloudContainer/addCloudContainer`. Verified by use on a
/// paid team. See [`DevServicesClient::create_cloud_container`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddCloudContainerBody {
    team_id: String,
    name: String,
    identifier: String,
}

/// Request body of `listCurrentContainersForAppId`. Confirmed by a read-only
/// probe.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListAppContainersBody {
    team_id: String,
    app_id_id: String,
}

/// Request body of `ios/identifiers/addContainersForAppId`. Verified by use on
/// a paid team. See [`DevServicesClient::associate_cloud_containers`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssociateContainersBody {
    team_id: String,
    app_id_id: String,
    cloud_containers: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListCloudContainersResponse {
    #[serde(default)]
    cloud_container_list: Vec<CloudContainerEntry>,
}

/// Container entry. Verified by use on a paid team. A missing field reads as an
/// empty string.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudContainerEntry {
    /// Container id passed to the associate action.
    #[serde(default)]
    cloud_container: String,
    #[serde(default)]
    identifier: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCloudContainerResponse {
    cloud_container: CloudContainerEntry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAppContainersResponse {
    #[serde(default)]
    cloud_container_for_app_list: Vec<AppContainerEntry>,
}

/// Entry of `cloudContainerForAppList`. The list holds a dict with only a
/// `bundle` key, then one dict per associated container. Its `cloudContainer`
/// is a container dict with `status`, `cloudContainer` (the id), `prefix`,
/// `name`, and `identifier`. A plain id string is also accepted.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppContainerEntry {
    #[serde(default)]
    cloud_container: Option<AppContainerRef>,
}

/// Value of `cloudContainer` in an [`AppContainerEntry`].
#[derive(Deserialize)]
#[serde(untagged)]
enum AppContainerRef {
    Id(String),
    Entry(CloudContainerEntry),
}

impl AppContainerRef {
    fn into_id(self) -> String {
        match self {
            Self::Id(id) => id,
            Self::Entry(entry) => entry.cloud_container,
        }
    }
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
    /// Enabled features by code. Values are booleans or strings.
    #[serde(default)]
    features: BTreeMap<String, plist::Value>,
}

/// Request body of `ios/updateAppId`. Feature codes are top-level keys.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAppIdBody {
    team_id: String,
    app_id_id: String,
    #[serde(flatten)]
    features: BTreeMap<String, String>,
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
        Ok(self
            .ensure_app_id_entry(team_id, bundle_id, name)?
            .app_id_id)
    }

    /// App ID of `bundle_id`, or `None` if the team has none.
    fn find_app_id_entry(&self, team_id: &str, bundle_id: &str) -> Result<Option<AppIdEntry>> {
        let resp: ListAppIdsResponse = parse(self.post(
            "ios/listAppIds",
            TeamIdBody {
                team_id: team_id.into(),
            },
        )?)?;
        Ok(resp.app_ids.into_iter().find(|a| a.identifier == bundle_id))
    }

    /// Codes of the features in `features` that are not enabled on the App ID
    /// of `bundle_id`. All of them if the App ID does not exist. Read-only.
    pub fn missing_app_id_features(
        &self,
        team_id: &str,
        bundle_id: &str,
        features: &[(&str, &str)],
    ) -> Result<Vec<String>> {
        let missing = match self.find_app_id_entry(team_id, bundle_id)? {
            Some(entry) => missing_features(&entry.features, features),
            None => features.to_vec(),
        };
        Ok(missing
            .into_iter()
            .map(|(code, _)| code.to_string())
            .collect())
    }

    fn ensure_app_id_entry(
        &self,
        team_id: &str,
        bundle_id: &str,
        name: &str,
    ) -> Result<AppIdEntry> {
        if let Some(existing) = self.find_app_id_entry(team_id, bundle_id)? {
            return Ok(existing);
        }

        let resp: AddAppIdResponse = parse(self.post(
            "ios/addAppId",
            AddAppIdBody {
                team_id: team_id.into(),
                identifier: bundle_id.into(),
                name: name.into(),
            },
        )?)?;
        Ok(resp.app_id)
    }

    /// Ensure the App ID exists, then enable features on it.
    ///
    /// Returns true if `updateAppId` was sent. It is skipped if every feature
    /// is already enabled.
    ///
    /// Experimental. The `updateAppId` action name and request shape are
    /// inferred from Xcode's network behavior. Apple does not document them.
    /// A wrong code or shape is reported by [`check_result_code`].
    ///
    /// `features` is `(feature code, enabled value)` pairs, such as
    /// `("APG3427HIY", "on")` for App Groups.
    pub fn ensure_capabilities(
        &self,
        team_id: &str,
        bundle_id: &str,
        name: &str,
        features: &[(&str, &str)],
    ) -> Result<bool> {
        if features.is_empty() {
            return Ok(false);
        }
        let entry = self.ensure_app_id_entry(team_id, bundle_id, name)?;
        let missing = missing_features(&entry.features, features);
        if missing.is_empty() {
            return Ok(false);
        }
        self.post(
            "ios/updateAppId",
            update_app_id_body(team_id, &entry.app_id_id, &missing),
        )?;
        Ok(true)
    }

    /// List every iCloud container on the team, visible to the session.
    ///
    /// Read-only. Pages are requested until a page adds no new identifier,
    /// because the default page size is 20 and the first page number is
    /// unverified. Fails with [`TeamNotEligible`] on a team without a paid
    /// Apple Developer Program membership.
    pub fn list_cloud_containers(&self, team_id: &str) -> Result<Vec<CloudContainer>> {
        let mut containers = Vec::new();
        let mut page_number = 0;
        loop {
            let resp: ListCloudContainersResponse = parse(self.post(
                "cloudContainer/listCloudContainers",
                ListCloudContainersBody {
                    team_id: team_id.into(),
                    page_number,
                    page_size: CONTAINER_PAGE_SIZE,
                    sort: CONTAINER_SORT,
                },
            )?)?;
            let page = resp
                .cloud_container_list
                .into_iter()
                .map(CloudContainer::from);
            if merge_containers(&mut containers, page) == 0 {
                return Ok(containers);
            }
            page_number += 1;
        }
    }

    /// Ids of the containers associated with an App ID. `platform` is `ios`
    /// or `mac`.
    ///
    /// Read-only. A dict with only a `bundle` key is treated as no
    /// association.
    pub fn list_app_containers(
        &self,
        team_id: &str,
        platform: &str,
        app_id_id: &str,
    ) -> Result<Vec<String>> {
        let resp: ListAppContainersResponse = parse(self.post(
            &format!("{platform}/listCurrentContainersForAppId"),
            ListAppContainersBody {
                team_id: team_id.into(),
                app_id_id: app_id_id.into(),
            },
        )?)?;
        Ok(associated_container_ids(resp.cloud_container_for_app_list))
    }

    /// Create an iCloud container on the team. The container cannot be
    /// deleted afterwards.
    ///
    /// Verified by use on a paid team. The response holds a `cloudContainer`
    /// dict. A wrong shape is reported by [`check_result_code`] or as a parse
    /// error.
    pub fn create_cloud_container(
        &self,
        team_id: &str,
        identifier: &str,
        name: &str,
    ) -> Result<CloudContainer> {
        validate_container_identifier(identifier)?;
        let resp: AddCloudContainerResponse = parse(self.post(
            "cloudContainer/addCloudContainer",
            AddCloudContainerBody {
                team_id: team_id.into(),
                name: name.into(),
                identifier: identifier.into(),
            },
        )?)?;
        Ok(resp.cloud_container.into())
    }

    /// Associate containers with an iOS App ID, keeping the containers that
    /// are already associated. `container_ids` are container ids from
    /// [`CloudContainer::id`].
    ///
    /// Returns true if the associate action was sent. It is skipped if every
    /// container is already associated.
    ///
    /// Verified by use on a paid team. Known limitations: it is not recorded
    /// whether Apple replaces or appends, so the union of current and
    /// requested ids is sent. It is also not recorded whether macOS App IDs
    /// accept this action.
    pub fn associate_cloud_containers(
        &self,
        team_id: &str,
        app_id_id: &str,
        container_ids: &[String],
    ) -> Result<bool> {
        let current = self.list_app_containers(team_id, "ios", app_id_id)?;
        let all = container_union(&current, container_ids);
        if all.len() == current.len() {
            return Ok(false);
        }
        self.post(
            "ios/identifiers/addContainersForAppId",
            AssociateContainersBody {
                team_id: team_id.into(),
                app_id_id: app_id_id.into(),
                cloud_containers: all,
            },
        )?;
        Ok(true)
    }

    /// Ensure the App ID for `bundle_id` exists, then associate containers
    /// with it. See [`Self::associate_cloud_containers`].
    pub fn ensure_cloud_containers(
        &self,
        team_id: &str,
        bundle_id: &str,
        name: &str,
        container_ids: &[String],
    ) -> Result<bool> {
        if container_ids.is_empty() {
            return Ok(false);
        }
        let app_id_id = self.ensure_app_id(team_id, bundle_id, name)?;
        self.associate_cloud_containers(team_id, &app_id_id, container_ids)
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

        // Error bodies can be JSON, such as resultCode 4100 for a team without
        // a paid membership. Parse them so `userString` is reported.
        if !status.is_success() && !body_str.trim_start().starts_with('{') {
            bail!("dev services {action} returned HTTP {status}: {body_str}");
        }

        let dict = parse_response_body(&body_str)?;

        check_result_code(&dict, action)?;

        Ok(dict)
    }
}

/// Container page size requested from `listCloudContainers`.
const CONTAINER_PAGE_SIZE: u32 = 500;

/// Sort order requested from `listCloudContainers`. The service rejects a
/// request without a sort value. The value comes from other clients.
const CONTAINER_SORT: &str = "name=asc";

/// Result code for a team without a program membership that is eligible for
/// the feature.
const RESULT_TEAM_NOT_ELIGIBLE: i64 = 4100;

/// The team has no paid Apple Developer Program membership, so iCloud
/// containers are unavailable.
#[derive(Debug)]
pub struct TeamNotEligible {
    action: String,
    message: String,
}

impl TeamNotEligible {
    pub fn new(action: impl Into<String>, message: impl Into<String>) -> Self {
        TeamNotEligible {
            action: action.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TeamNotEligible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dev services {} failed (code {RESULT_TEAM_NOT_ELIGIBLE}): {}",
            self.action, self.message
        )
    }
}

impl std::error::Error for TeamNotEligible {}

/// An iCloud container on a team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudContainer {
    /// Id passed to the associate action.
    pub id: String,
    /// Full identifier, such as `iCloud.com.example.app`.
    pub identifier: String,
    pub name: String,
}

impl From<CloudContainerEntry> for CloudContainer {
    fn from(entry: CloudContainerEntry) -> Self {
        CloudContainer {
            id: entry.cloud_container,
            identifier: entry.identifier,
            name: entry.name,
        }
    }
}

/// Parse a response body as a plist dict, or as a JSON object if it starts
/// with `{`.
fn parse_response_body(body: &str) -> Result<plist::Dictionary> {
    if body.trim_start().starts_with('{') {
        let json: serde_json::Value =
            serde_json::from_str(body).context("parsing dev services response JSON")?;
        return match json_to_plist(json) {
            Some(plist::Value::Dictionary(dict)) => Ok(dict),
            _ => bail!("dev services response is not a dict"),
        };
    }
    plist::Value::from_reader(Cursor::new(body.as_bytes()))
        .context("parsing dev services response plist")?
        .into_dictionary()
        .context("dev services response is not a dict")
}

/// Convert JSON to a plist value. Null values are dropped, since plist has no
/// null.
fn json_to_plist(json: serde_json::Value) -> Option<plist::Value> {
    use serde_json::Value;
    Some(match json {
        Value::Null => return None,
        Value::Bool(b) => b.into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64()?.into(),
        },
        Value::String(s) => s.into(),
        Value::Array(items) => {
            plist::Value::Array(items.into_iter().filter_map(json_to_plist).collect())
        },
        Value::Object(map) => plist::Value::Dictionary(
            map.into_iter()
                .filter_map(|(k, v)| Some((k, json_to_plist(v)?)))
                .collect(),
        ),
    })
}

/// Add containers whose identifier is not in `containers` yet. Returns the
/// number added.
fn merge_containers(
    containers: &mut Vec<CloudContainer>,
    page: impl IntoIterator<Item = CloudContainer>,
) -> usize {
    let before = containers.len();
    for container in page {
        if !containers
            .iter()
            .any(|c| c.identifier == container.identifier)
        {
            containers.push(container);
        }
    }
    containers.len() - before
}

/// Container ids of an associated-containers list. Entries without a
/// container id, such as a dict with only `bundle`, are dropped.
fn associated_container_ids(entries: Vec<AppContainerEntry>) -> Vec<String> {
    entries
        .into_iter()
        .filter_map(|e| e.cloud_container)
        .map(AppContainerRef::into_id)
        .filter(|id| !id.is_empty())
        .collect()
}

/// `current` followed by the ids in `wanted` that are not in `current`.
fn container_union(current: &[String], wanted: &[String]) -> Vec<String> {
    let mut all = current.to_vec();
    for id in wanted {
        if !all.contains(id) {
            all.push(id.clone());
        }
    }
    all
}

/// Check an iCloud container identifier. It must start with `iCloud.`, followed
/// by dot-separated segments of ASCII letters, digits, and hyphens.
pub fn validate_container_identifier(identifier: &str) -> Result<()> {
    let Some(suffix) = identifier.strip_prefix("iCloud.") else {
        bail!("invalid iCloud container identifier {identifier:?}: must start with `iCloud.`");
    };
    let valid = !suffix.is_empty()
        && suffix.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !valid {
        bail!(
            "invalid iCloud container identifier {identifier:?}: use reverse-DNS segments of \
             letters, digits, and hyphens after `iCloud.`, such as `iCloud.com.example.app`"
        );
    }
    Ok(())
}

fn update_app_id_body(
    team_id: &str,
    app_id_id: &str,
    features: &[(&str, &str)],
) -> UpdateAppIdBody {
    UpdateAppIdBody {
        team_id: team_id.into(),
        app_id_id: app_id_id.into(),
        features: features
            .iter()
            .map(|(code, value)| (code.to_string(), value.to_string()))
            .collect(),
    }
}

/// Features that are not enabled on the App ID yet.
fn missing_features<'a>(
    enabled: &BTreeMap<String, plist::Value>,
    wanted: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    wanted
        .iter()
        .filter(|(code, value)| {
            !enabled
                .get(*code)
                .is_some_and(|have| feature_matches(have, value))
        })
        .copied()
        .collect()
}

/// True if an enabled feature value equals the wanted value. Booleans match
/// `on` and `off`.
fn feature_matches(have: &plist::Value, want: &str) -> bool {
    match have {
        plist::Value::Boolean(b) => match want {
            "on" | "true" => *b,
            "off" | "false" => !*b,
            _ => false,
        },
        plist::Value::String(s) => s == want,
        _ => false,
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
        if code == RESULT_TEAM_NOT_ELIGIBLE {
            return Err(TeamNotEligible {
                action: action.to_string(),
                message: msg.to_string(),
            }
            .into());
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
    fn update_app_id_body_puts_features_at_top_level() {
        let body = update_app_id_body("TEAM1", "APPID1", &[("APG3427HIY", "on"), ("iCloud", "on")]);
        let dict = plist::to_value(&body).unwrap().into_dictionary().unwrap();
        assert_eq!(dict.get("teamId").unwrap().as_string(), Some("TEAM1"));
        assert_eq!(dict.get("appIdId").unwrap().as_string(), Some("APPID1"));
        assert_eq!(dict.get("APG3427HIY").unwrap().as_string(), Some("on"));
        assert_eq!(dict.get("iCloud").unwrap().as_string(), Some("on"));
        assert_eq!(dict.len(), 4);
    }

    const LIST_APP_IDS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>resultCode</key><integer>0</integer>
  <key>appIds</key><array>
    <dict>
      <key>identifier</key><string>com.example.other</string>
      <key>appIdId</key><string>OTHER</string>
    </dict>
    <dict>
      <key>identifier</key><string>com.example.app</string>
      <key>appIdId</key><string>APPID1</string>
      <key>features</key><dict>
        <key>APG3427HIY</key><true/>
        <key>iCloud</key><false/>
        <key>dataProtection</key><string>complete</string>
      </dict>
    </dict>
  </array>
</dict></plist>"#;

    fn fixture_entry(bundle_id: &str) -> AppIdEntry {
        let val = plist::Value::from_reader(Cursor::new(LIST_APP_IDS.as_bytes())).unwrap();
        let resp: ListAppIdsResponse = parse(val.into_dictionary().unwrap()).unwrap();
        resp.app_ids
            .into_iter()
            .find(|a| a.identifier == bundle_id)
            .unwrap()
    }

    #[test]
    fn missing_features_skips_enabled_features() {
        let entry = fixture_entry("com.example.app");
        let missing = missing_features(
            &entry.features,
            &[
                ("APG3427HIY", "on"),
                ("iCloud", "on"),
                ("dataProtection", "complete"),
            ],
        );
        assert_eq!(missing, vec![("iCloud", "on")]);
    }

    #[test]
    fn missing_features_detects_value_mismatch() {
        let entry = fixture_entry("com.example.app");
        let missing = missing_features(
            &entry.features,
            &[
                ("dataProtection", "completeUnlessOpen"),
                ("APG3427HIY", "off"),
            ],
        );
        assert_eq!(missing.len(), 2);
    }

    #[test]
    fn missing_features_is_everything_without_features() {
        let entry = fixture_entry("com.example.other");
        let missing = missing_features(&entry.features, &[("APG3427HIY", "on")]);
        assert_eq!(missing, vec![("APG3427HIY", "on")]);
    }

    #[test]
    fn check_result_code_passes_on_zero() {
        let mut d = plist::Dictionary::new();
        d.insert("resultCode".to_string(), (0i64).into());
        assert!(check_result_code(&d, "test").is_ok());
    }

    #[test]
    fn validate_container_identifier_accepts_reverse_dns() {
        assert!(validate_container_identifier("iCloud.com.example.app").is_ok());
        assert!(validate_container_identifier("iCloud.com.example-co.app2").is_ok());
        assert!(validate_container_identifier("iCloud.app").is_ok());
    }

    #[test]
    fn validate_container_identifier_rejects_bad_input() {
        for bad in [
            "",
            "com.example.app",
            "icloud.com.example.app",
            "iCloud",
            "iCloud.",
            "iCloud..app",
            "iCloud.com.",
            "iCloud.com example",
            "iCloud.com.example_app",
            "iCloud.com.exämple",
            " iCloud.com.example",
            "iCloud.com.example\n",
        ] {
            assert!(validate_container_identifier(bad).is_err(), "{bad:?}");
        }
    }

    fn container(id: &str, identifier: &str) -> CloudContainer {
        CloudContainer {
            id: id.into(),
            identifier: identifier.into(),
            name: String::new(),
        }
    }

    #[test]
    fn merge_containers_counts_only_new_identifiers() {
        let mut all = vec![container("1", "iCloud.a")];
        let added = merge_containers(
            &mut all,
            [container("1", "iCloud.a"), container("2", "iCloud.b")],
        );
        assert_eq!(added, 1);
        assert_eq!(all.len(), 2);
        assert_eq!(merge_containers(&mut all, [container("2", "iCloud.b")]), 0);
    }

    #[test]
    fn container_union_keeps_current_and_appends_new() {
        let current = vec!["A".to_string(), "B".to_string()];
        let wanted = vec!["B".to_string(), "C".to_string(), "C".to_string()];
        assert_eq!(container_union(&current, &wanted), ["A", "B", "C"]);
        assert_eq!(container_union(&current, &["A".to_string()]), current);
    }

    #[test]
    fn associated_container_ids_ignores_bundle_only_entry() {
        let resp: ListAppContainersResponse = parse(
            plist::Value::from_reader(Cursor::new(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>resultCode</key><integer>0</integer>
  <key>cloudContainerForAppList</key><array>
    <dict><key>bundle</key><string>68NBUYN8QA</string></dict>
  </array>
</dict></plist>"#,
            ))
            .unwrap()
            .into_dictionary()
            .unwrap(),
        )
        .unwrap();
        assert!(associated_container_ids(resp.cloud_container_for_app_list).is_empty());
    }

    #[test]
    fn associated_container_ids_reads_a_captured_response() {
        let dict = parse_response_body(
            r#"{"cloudContainerForAppList":[{"bundle":"FD5BCTX674"},{"cloudContainer":{"status":"current","cloudContainer":"C57Q6U4LZZ","prefix":"75J48AQ5LF","name":"com breakfastlabs dejavu","identifier":"iCloud.com.breakfastlabs.dejavu"}}],"resultCode":0}"#,
        )
        .unwrap();
        let resp: ListAppContainersResponse = parse(dict).unwrap();
        assert_eq!(
            associated_container_ids(resp.cloud_container_for_app_list),
            ["C57Q6U4LZZ"]
        );
    }

    #[test]
    fn associated_container_ids_reads_container_dicts_and_plain_ids() {
        let dict = parse_response_body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>resultCode</key><integer>0</integer>
  <key>cloudContainerForAppList</key><array>
    <dict>
      <key>bundle</key><string>B1</string>
      <key>cloudContainer</key><dict>
        <key>cloudContainer</key><string>CID1</string>
        <key>identifier</key><string>iCloud.com.example.app</string>
      </dict>
    </dict>
    <dict><key>cloudContainer</key><string>CID2</string></dict>
  </array>
</dict></plist>"#,
        )
        .unwrap();
        let resp: ListAppContainersResponse = parse(dict).unwrap();
        assert_eq!(
            associated_container_ids(resp.cloud_container_for_app_list),
            ["CID1", "CID2"]
        );
    }

    #[test]
    fn list_response_parses_containers() {
        let dict = parse_response_body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>resultCode</key><integer>0</integer>
  <key>cloudContainerList</key><array>
    <dict>
      <key>cloudContainer</key><string>CID1</string>
      <key>identifier</key><string>iCloud.com.example.app</string>
      <key>name</key><string>Example</string>
    </dict>
  </array>
</dict></plist>"#,
        )
        .unwrap();
        let resp: ListCloudContainersResponse = parse(dict).unwrap();
        let containers: Vec<CloudContainer> = resp
            .cloud_container_list
            .into_iter()
            .map(CloudContainer::from)
            .collect();
        let expected = CloudContainer {
            id: "CID1".into(),
            identifier: "iCloud.com.example.app".into(),
            name: "Example".into(),
        };
        assert_eq!(containers, [expected]);
    }

    #[test]
    fn json_error_body_surfaces_user_string() {
        let body = r#"{"resultCode":4100,"userString":"The selected team does not have a program membership that is eligible for this feature.","extra":null}"#;
        let dict = parse_response_body(body).unwrap();
        let err = check_result_code(&dict, "cloudContainer/listCloudContainers").unwrap_err();
        assert!(err.downcast_ref::<TeamNotEligible>().is_some(), "{err}");
        assert!(format!("{err}").contains("program membership"), "{err}");
    }

    #[test]
    fn json_error_body_with_other_code_surfaces_user_string() {
        let dict =
            parse_response_body(r#"{"resultCode":1003,"userString":"Service mapping"}"#).unwrap();
        let err = check_result_code(&dict, "test").unwrap_err();
        assert!(err.downcast_ref::<TeamNotEligible>().is_none());
        assert!(format!("{err}").contains("Service mapping"), "{err}");
    }

    #[test]
    fn associate_body_uses_expected_field_names() {
        let body = AssociateContainersBody {
            team_id: "T".into(),
            app_id_id: "A".into(),
            cloud_containers: vec!["C1".into(), "C2".into()],
        };
        let dict = plist::to_value(&body).unwrap().into_dictionary().unwrap();
        assert_eq!(dict.get("teamId").unwrap().as_string(), Some("T"));
        assert_eq!(dict.get("appIdId").unwrap().as_string(), Some("A"));
        assert_eq!(
            dict.get("cloudContainers")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
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

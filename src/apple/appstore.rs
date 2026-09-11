use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use clml::cprintln;
use indoc::indoc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::ResolvedConfig;

pub struct AppStoreClient {
    key_id: String,
    issuer: String,
    key_pem: Vec<u8>,
    agent: ureq::Agent,
}

#[derive(Serialize)]
struct Claims {
    iss: String,
    iat: u64,
    exp: u64,
    aud: String,
}

#[derive(Deserialize)]
struct ApiErrors {
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    title: String,
    detail: String,
}

pub struct Cert {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
}

pub struct PortalDevice {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
    pub udid: String,
}

fn api_error(code: u16, body: &str) -> anyhow::Error {
    if let Ok(errs) = serde_json::from_str::<ApiErrors>(body) {
        let msgs: Vec<String> = errs
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.title, e.detail))
            .collect();
        return anyhow::anyhow!("App Store Connect API error ({code}): {}", msgs.join("; "));
    }
    anyhow::anyhow!("App Store Connect API error ({code}): {body}")
}

impl AppStoreClient {
    pub fn from_config(cfg: &ResolvedConfig) -> Result<Self> {
        let key_path = cfg.apple_api_key_path.as_ref().context(indoc! {"
            App Store Connect API credentials required for `app_store_connect` provisioning profile management.
            Please set your API key id, API key path, and API issuer, either as environment variables:

              APPLE_API_KEY        key ID (e.g. \"2X9R4HXF34\")
              APPLE_API_KEY_PATH   path to your .p8 key file
              APPLE_API_ISSUER     issuer UUID from App Store Connect

            or under [apple] in strudel.toml. Run `strudel help notarize` for details.

            Alternatively, set [ios] provisioning = \"free\" in strudel.toml and run `strudel login`
            to use a plain Apple ID without a paid developer account.
         ",
        })?;
        if cfg.apple_api_key.is_empty() {
            bail!("APPLE_API_KEY (key ID) is required but not set.");
        }
        if cfg.apple_api_issuer.is_empty() {
            bail!("APPLE_API_ISSUER is required but not set.");
        }
        let key_pem = fs::read(key_path)
            .with_context(|| format!("Failed to read API key from {}", key_path.display()))?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(AppStoreClient {
            key_id: cfg.apple_api_key.clone(),
            issuer: cfg.apple_api_issuer.clone(),
            key_pem,
            agent,
        })
    }

    fn bearer_token(&self) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = Claims {
            iss: self.issuer.clone(),
            iat: now,
            exp: now + 900,
            aud: "appstoreconnect-v1".to_string(),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let key = EncodingKey::from_ec_pem(&self.key_pem)
            .context("Failed to parse App Store Connect API key as EC PEM (PKCS#8)")?;
        encode(&header, &claims, &key).context("Failed to sign JWT")
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("https://api.appstoreconnect.apple.com{path}");
        let token = self.bearer_token()?;
        let mut resp = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(|e| anyhow::anyhow!("Network error calling {url}: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(api_error(status.as_u16(), &body));
        }
        resp.body_mut()
            .read_json::<T>()
            .context("Failed to parse API response")
    }

    fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("https://api.appstoreconnect.apple.com{path}");
        let token = self.bearer_token()?;
        let mut resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(body)
            .map_err(|e| anyhow::anyhow!("Network error calling {url}: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(api_error(status.as_u16(), &body));
        }
        resp.body_mut()
            .read_json::<T>()
            .context("Failed to parse API response")
    }

    fn delete(&self, path: &str) -> Result<()> {
        let url = format!("https://api.appstoreconnect.apple.com{path}");
        let token = self.bearer_token()?;
        let mut resp = self
            .agent
            .delete(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(|e| anyhow::anyhow!("Network error calling {url}: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(api_error(status.as_u16(), &body));
        }
        Ok(())
    }

    /// Look up the bundle ID resource by identifier, without creating it.
    /// Returns `None` if it doesn't exist yet. Read-only: safe to call
    /// before asking the user for confirmation to make any changes.
    pub fn find_bundle_id(&self, bundle_id: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<BundleIdResource>,
        }
        #[derive(Deserialize)]
        struct BundleIdResource {
            id: String,
            attributes: BundleIdAttrs,
        }
        #[derive(Deserialize)]
        struct BundleIdAttrs {
            identifier: String,
        }

        cprintln!("<dim>Looking for bundle ID on App Store Connect: {bundle_id}</dim>");
        let path = format!("/v1/bundleIds?filter[identifier]={bundle_id}");
        let list: ListResp = self.get_json(&path)?;
        Ok(list
            .data
            .into_iter()
            .find(|r| r.attributes.identifier == bundle_id)
            .map(|r| r.id))
    }

    /// Find or create the bundle ID resource. Returns the resource ID.
    /// `platform` is the ASC `BundleIdPlatform` value (`"IOS"` or
    /// `"MAC_OS"`), used only when the bundle ID doesn't already exist.
    pub fn find_or_create_bundle_id(
        &self,
        bundle_id: &str,
        name: &str,
        platform: &str,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct SingleResp {
            data: BundleIdResource,
        }
        #[derive(Deserialize)]
        struct BundleIdResource {
            id: String,
        }

        if let Some(id) = self.find_bundle_id(bundle_id)? {
            return Ok(id);
        }
        let body = json!({
            "data": {
                "type": "bundleIds",
                "attributes": {"identifier": bundle_id, "name": name, "platform": platform}
            }
        });

        cprintln!("<dim>Bundle ID not found, creating on App Store Connect...</dim>");
        self.post_json::<_, SingleResp>("/v1/bundleIds", &body).map(|resp| resp.data.id).or_else(|e| {
            if format!("{e}").contains("403") {
                    bail!(
                        "Insufficient permissions to create bundle ID {bundle_id}. \
                         An API key with the Admin role is required to be able to manage bundle IDs on the App Store Connect portal. You can also create the bundle ID manually at https://developer.apple.com/account/resources/identifiers/list and run strudel again."
                    )
                } else {
                    Err(e)
                }
            })
    }

    /// Capability types already enabled on the bundle ID. Read-only: safe to
    /// call before asking the user for confirmation to make any changes.
    pub fn enabled_capability_types(
        &self,
        bundle_id_resource_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<CapResource>,
        }
        #[derive(Deserialize)]
        struct CapResource {
            attributes: CapAttrs,
        }
        #[derive(Deserialize)]
        struct CapAttrs {
            #[serde(rename = "capabilityType")]
            capability_type: String,
        }

        cprintln!("<dim>Checking enabled capabilities on bundle ID...</dim>");
        let list: ListResp = self.get_json(&format!(
            "/v1/bundleIds/{bundle_id_resource_id}/bundleIdCapabilities"
        ))?;
        Ok(list
            .data
            .into_iter()
            .map(|c| c.attributes.capability_type)
            .collect())
    }

    /// Ensure each of `capability_types` is enabled on the bundle ID.
    /// Capabilities already enabled are left alone.
    pub fn ensure_capabilities(
        &self,
        bundle_id_resource_id: &str,
        capability_types: &[&str],
    ) -> Result<()> {
        if capability_types.is_empty() {
            return Ok(());
        }

        let enabled = self.enabled_capability_types(bundle_id_resource_id)?;

        for cap_type in capability_types {
            if enabled.contains(*cap_type) {
                continue;
            }
            cprintln!("<dim>Enabling capability {cap_type} on bundle ID...</dim>");
            let body = json!({
                "data": {
                    "type": "bundleIdCapabilities",
                    "attributes": {"capabilityType": cap_type},
                    "relationships": {
                        "bundleId": {"data": {"type": "bundleIds", "id": bundle_id_resource_id}}
                    }
                }
            });
            self.post_json::<_, Value>("/v1/bundleIdCapabilities", &body)
                .map(|_| ())
                .or_else(|e| {
                    if format!("{e}").contains("403") {
                        bail!(
                            "Insufficient permissions to enable capability {cap_type}. \
                             An API key with the Admin role is required to manage capabilities \
                             on the App Store Connect portal. You can also enable it manually at \
                             https://developer.apple.com/account/resources/identifiers/list and \
                             run strudel again."
                        )
                    } else {
                        Err(e)
                    }
                })?;
        }
        Ok(())
    }

    /// List development certificates in the account. Errors if none exist.
    pub fn list_development_certificates(&self) -> Result<Vec<Cert>> {
        self.list_certificates("DEVELOPMENT")
    }

    /// List Developer ID Application certificates, covering both CA
    /// generations Apple issues under: the classic `DEVELOPER_ID_APPLICATION`
    /// chain, and the newer `DEVELOPER_ID_APPLICATION_G2` chain that a fresh
    /// CSR comes back on by default today. A profile that only embeds one
    /// generation silently fails to authorize a signing identity issued
    /// under the other - see the strudel README's "Signing & notarization"
    /// section. G2 certificates are listed first, since they're the ones a
    /// newly-issued identity is actually likely to use.
    pub fn list_developer_id_application_certificates(&self) -> Result<Vec<Cert>> {
        let mut certs = self.list_certificates_allow_empty("DEVELOPER_ID_APPLICATION_G2")?;
        certs.extend(self.list_certificates_allow_empty("DEVELOPER_ID_APPLICATION")?);
        if certs.is_empty() {
            bail!(
                "No Developer ID Application certificates found in your Apple Developer \
                 account.\n\
                 Create one at: https://developer.apple.com/account/resources/certificates/list"
            );
        }
        Ok(certs)
    }

    /// List certificates of `certificate_type` (an ASC `CertificateType`,
    /// e.g. `"DEVELOPMENT"` or `"DEVELOPER_ID_APPLICATION"`) in the account.
    /// Errors if none exist.
    pub fn list_certificates(&self, certificate_type: &str) -> Result<Vec<Cert>> {
        let certs = self.list_certificates_allow_empty(certificate_type)?;
        if certs.is_empty() {
            bail!(
                "No {certificate_type} certificates found in your Apple Developer account.\n\
                 Create one at: https://developer.apple.com/account/resources/certificates/list"
            );
        }
        Ok(certs)
    }

    /// Same as [`Self::list_certificates`] but returns an empty `Vec` instead
    /// of erroring when the account has none of `certificate_type` - for
    /// callers that check multiple types and only care whether the union is
    /// empty.
    fn list_certificates_allow_empty(&self, certificate_type: &str) -> Result<Vec<Cert>> {
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<CertResource>,
        }
        #[derive(Deserialize)]
        struct CertResource {
            id: String,
            attributes: CertAttrs,
        }
        #[derive(Deserialize)]
        struct CertAttrs {
            name: String,
        }

        cprintln!("<dim>Listing {certificate_type} certificates on App Store Connect...</dim>");
        let list: ListResp = self.get_json(&format!(
            "/v1/certificates?filter[certificateType]={certificate_type}&limit=200"
        ))?;
        Ok(list
            .data
            .into_iter()
            .map(|r| Cert {
                id: r.id,
                name: r.attributes.name,
            })
            .collect())
    }

    /// List registered iOS devices with ENABLED status.
    pub fn list_devices(&self) -> Result<Vec<PortalDevice>> {
        cprintln!("<dim>Listing registered iOS devices on App Store Connect...</dim>");
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<DeviceResource>,
        }
        #[derive(Deserialize)]
        struct DeviceResource {
            id: String,
            attributes: DeviceAttrs,
        }
        #[derive(Deserialize)]
        struct DeviceAttrs {
            name: String,
            udid: String,
        }

        let list: ListResp =
            self.get_json("/v1/devices?filter[platform]=IOS&filter[status]=ENABLED&limit=200")?;
        Ok(list
            .data
            .into_iter()
            .map(|r| PortalDevice {
                id: r.id,
                name: r.attributes.name,
                udid: r.attributes.udid,
            })
            .collect())
    }

    /// Register a device on the portal. Returns the resource ID.
    pub fn register_device(&self, name: &str, udid: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            data: DeviceResource,
        }
        #[derive(Deserialize)]
        struct DeviceResource {
            id: String,
        }

        let body = json!({
            "data": {
                "type": "devices",
                "attributes": {"name": name, "udid": udid, "platform": "IOS"}
            }
        });
        let resp: Resp = self.post_json("/v1/devices", &body)?;
        Ok(resp.data.id)
    }

    /// Create an `IOS_APP_DEVELOPMENT` provisioning profile embedding
    /// `device_ids`. Deletes any existing profile with `name` first so the
    /// device set is always current. Returns the raw `.mobileprovision`
    /// bytes.
    pub fn create_development_profile(
        &self,
        name: &str,
        bundle_id_resource_id: &str,
        cert_ids: &[String],
        device_ids: &[String],
    ) -> Result<Vec<u8>> {
        self.create_profile(
            name,
            "IOS_APP_DEVELOPMENT",
            bundle_id_resource_id,
            cert_ids,
            device_ids,
        )
    }

    /// Create a provisioning profile of `profile_type` (an ASC `ProfileType`,
    /// e.g. `"IOS_APP_DEVELOPMENT"` or `"MAC_APP_DIRECT"`). Deletes any
    /// existing profile with `name` first so it's always current. `device_ids`
    /// is empty for profile types with no device relationship (e.g. macOS
    /// Developer ID profiles). Returns the raw profile bytes.
    pub fn create_profile(
        &self,
        name: &str,
        profile_type: &str,
        bundle_id_resource_id: &str,
        cert_ids: &[String],
        device_ids: &[String],
    ) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<ProfileResource>,
        }
        #[derive(Deserialize)]
        struct ProfileResource {
            id: String,
            attributes: ProfileAttrs,
        }
        #[derive(Deserialize)]
        struct ProfileAttrs {
            name: String,
        }

        let list: ListResp = self.get_json(&format!(
            "/v1/profiles?filter[profileType]={profile_type}&limit=200"
        ))?;
        for p in list.data {
            if p.attributes.name == name {
                self.delete(&format!("/v1/profiles/{}", p.id))?;
                break;
            }
        }

        let cert_data: Vec<Value> = cert_ids
            .iter()
            .map(|id| json!({"type": "certificates", "id": id}))
            .collect();

        let mut relationships = json!({
            "bundleId": {
                "data": {"type": "bundleIds", "id": bundle_id_resource_id}
            },
            "certificates": {"data": cert_data}
        });
        if !device_ids.is_empty() {
            let device_data: Vec<Value> = device_ids
                .iter()
                .map(|id| json!({"type": "devices", "id": id}))
                .collect();
            relationships["devices"] = json!({"data": device_data});
        }

        let body = json!({
            "data": {
                "type": "profiles",
                "attributes": {
                    "name": name,
                    "profileType": profile_type
                },
                "relationships": relationships
            }
        });

        #[derive(Deserialize)]
        struct CreateResp {
            data: CreatedProfile,
        }
        #[derive(Deserialize)]
        struct CreatedProfile {
            attributes: CreatedProfileAttrs,
        }
        #[derive(Deserialize)]
        struct CreatedProfileAttrs {
            #[serde(rename = "profileContent")]
            profile_content: String,
        }

        let resp: CreateResp = self.post_json("/v1/profiles", &body).or_else(|e| {
            if format!("{e}").contains("403") {
                bail!(
                    "Insufficient permissions to create provisioning profile. \
                     An API key with the Admin role is required to manage profiles on the \
                     App Store Connect portal."
                )
            } else {
                Err(e)
            }
        })?;
        BASE64
            .decode(&resp.data.attributes.profile_content)
            .context("Failed to decode profile content (base64)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_serialize_correctly() {
        let claims = Claims {
            iss: "test-issuer".into(),
            iat: 1000,
            exp: 1900,
            aud: "appstoreconnect-v1".into(),
        };
        let json = serde_json::to_string(&claims).unwrap();
        assert!(json.contains("\"aud\":\"appstoreconnect-v1\""));
        assert!(json.contains("\"iss\":\"test-issuer\""));
        assert!(json.contains("\"iat\":1000"));
        assert!(json.contains("\"exp\":1900"));
    }

    #[test]
    fn api_error_with_apple_format() {
        let body = r#"{"errors":[{"status":"409","code":"ENTITY_ERROR","title":"Invalid attribute","detail":"The provided bundle ID is not available."}]}"#;
        let err = api_error(409, body);
        let msg = format!("{err}");
        assert!(msg.contains("(409)"), "got: {msg}");
        assert!(msg.contains("Invalid attribute"), "got: {msg}");
        assert!(msg.contains("bundle ID"), "got: {msg}");
    }

    #[test]
    fn api_error_fallback_on_non_json() {
        let err = api_error(500, "Internal Server Error");
        let msg = format!("{err}");
        assert!(msg.contains("(500)"), "got: {msg}");
        assert!(msg.contains("Internal Server Error"), "got: {msg}");
    }
}

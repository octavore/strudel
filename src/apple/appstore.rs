use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use color_print::cprintln;
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
            Please set your API key id, API key path, and API issuer in your environment or strudel.toml.

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

    /// Find or create the bundle ID resource. Returns the resource ID.
    pub fn find_or_create_bundle_id(&self, bundle_id: &str, name: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct ListResp {
            data: Vec<BundleIdResource>,
        }
        #[derive(Deserialize)]
        struct SingleResp {
            data: BundleIdResource,
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
        if let Some(r) = list
            .data
            .into_iter()
            .find(|r| r.attributes.identifier == bundle_id)
        {
            return Ok(r.id);
        }
        let body = json!({
            "data": {
                "type": "bundleIds",
                "attributes": {"identifier": bundle_id, "name": name, "platform": "IOS"}
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

    /// List development certificates in the account. Errors if none exist.
    pub fn list_development_certificates(&self) -> Result<Vec<Cert>> {
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

        cprintln!("<dim>Listing development certificates on App Store Connect...</dim>");
        let list: ListResp =
            self.get_json("/v1/certificates?filter[certificateType]=DEVELOPMENT&limit=200")?;
        if list.data.is_empty() {
            bail!(
                "No development certificates found in your Apple Developer account.\n\
                 Create one at: https://developer.apple.com/account/resources/certificates/list"
            );
        }
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

    /// Create a development provisioning profile. Deletes any existing profile
    /// with `name` first so the device set is always current. Returns the raw
    /// `.mobileprovision` bytes.
    pub fn create_development_profile(
        &self,
        name: &str,
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

        let list: ListResp =
            self.get_json("/v1/profiles?filter[profileType]=IOS_APP_DEVELOPMENT&limit=200")?;
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
        let device_data: Vec<Value> = device_ids
            .iter()
            .map(|id| json!({"type": "devices", "id": id}))
            .collect();

        let body = json!({
            "data": {
                "type": "profiles",
                "attributes": {
                    "name": name,
                    "profileType": "IOS_APP_DEVELOPMENT"
                },
                "relationships": {
                    "bundleId": {
                        "data": {"type": "bundleIds", "id": bundle_id_resource_id}
                    },
                    "certificates": {"data": cert_data},
                    "devices": {"data": device_data}
                }
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

    #[test]
    fn bundle_id_list_response_shape() {
        let json = r#"{"data":[{"id":"XYZ123","type":"bundleIds","attributes":{"name":"MyApp","identifier":"com.example.app","platform":"IOS","seedId":"TEAM1"}}]}"#;
        let val: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            val["data"][0]["attributes"]["identifier"],
            "com.example.app"
        );
        assert_eq!(val["data"][0]["id"], "XYZ123");
    }
}

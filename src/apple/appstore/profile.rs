use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::AppStoreClient;
use super::types::{ListEnvelope, Resource, SingleEnvelope};

impl AppStoreClient {
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
        struct ProfileAttrs {
            name: String,
        }

        let list: ListEnvelope<Resource<ProfileAttrs>> = self.get_json(&format!(
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
        struct CreatedProfileAttrs {
            #[serde(rename = "profileContent")]
            profile_content: String,
        }

        let resp: SingleEnvelope<Resource<CreatedProfileAttrs>> =
            self.post_json("/v1/profiles", &body).or_else(|e| {
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

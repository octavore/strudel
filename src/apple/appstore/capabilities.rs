use std::collections::HashSet;

use anyhow::{Result, bail};
use clml::cformat;
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::AppStoreClient;
use super::types::{Attrs, ListEnvelope};
use crate::apple::capabilities::CapabilityType;

impl AppStoreClient {
    /// Capability types already enabled on the bundle ID. Read-only: safe to
    /// call before asking the user for confirmation to make any changes.
    pub fn enabled_capability_types(
        &self,
        bundle_id_resource_id: &str,
    ) -> Result<HashSet<CapabilityType>> {
        #[derive(Deserialize)]
        struct CapAttrs {
            #[serde(rename = "capabilityType")]
            capability_type: String,
        }

        self.progress(cformat!(
            "<dim>Checking enabled capabilities on bundle ID...</dim>"
        ));
        let list: ListEnvelope<Attrs<CapAttrs>> = self.get_json(&format!(
            "/v1/bundleIds/{bundle_id_resource_id}/bundleIdCapabilities"
        ))?;
        // Values missing from `CapabilityType` are not managed by strudel.
        Ok(list
            .data
            .into_iter()
            .filter_map(|c| CapabilityType::from_api(&c.attributes.capability_type))
            .collect())
    }

    /// Enable `capability_type` on the bundle ID. Callers check
    /// [`Self::enabled_capability_types`] first.
    pub fn enable_capability(
        &self,
        bundle_id_resource_id: &str,
        capability_type: CapabilityType,
    ) -> Result<()> {
        let cap_type = capability_type.as_str();
        self.progress(cformat!(
            "<dim>Enabling capability {cap_type} on bundle ID...</dim>"
        ));
        let mut attributes = json!({"capabilityType": cap_type});
        if capability_type == CapabilityType::Icloud {
            // The API rejects iCloud without a CloudKit version. Xcode 6 is
            // the CloudKit variant.
            attributes["settings"] = json!([
                {"key": "ICLOUD_VERSION", "options": [{"key": "XCODE_6"}]}
            ]);
        }
        let body = json!({
            "data": {
                "type": "bundleIdCapabilities",
                "attributes": attributes,
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
            })
    }
}

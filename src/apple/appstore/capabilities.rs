use anyhow::{Result, bail};
use clml::cprintln;
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::AppStoreClient;
use super::types::{Attrs, ListEnvelope};

impl AppStoreClient {
    /// Capability types already enabled on the bundle ID. Read-only: safe to
    /// call before asking the user for confirmation to make any changes.
    pub fn enabled_capability_types(
        &self,
        bundle_id_resource_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        #[derive(Deserialize)]
        struct CapAttrs {
            #[serde(rename = "capabilityType")]
            capability_type: String,
        }

        cprintln!("<dim>Checking enabled capabilities on bundle ID...</dim>");
        let list: ListEnvelope<Attrs<CapAttrs>> = self.get_json(&format!(
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
}

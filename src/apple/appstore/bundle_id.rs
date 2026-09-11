use anyhow::{Result, bail};
use clml::cprintln;
use serde::Deserialize;
use serde_json::json;

use super::client::AppStoreClient;
use super::types::{IdOnly, ListEnvelope, Resource, SingleEnvelope};

impl AppStoreClient {
    /// Look up the bundle ID resource by identifier, without creating it.
    /// Returns `None` if it doesn't exist yet. Read-only: safe to call
    /// before asking the user for confirmation to make any changes.
    pub fn find_bundle_id(&self, bundle_id: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct BundleIdAttrs {
            identifier: String,
        }

        cprintln!("<dim>Looking for bundle ID on App Store Connect: {bundle_id}</dim>");
        let path = format!("/v1/bundleIds?filter[identifier]={bundle_id}");
        let list: ListEnvelope<Resource<BundleIdAttrs>> = self.get_json(&path)?;
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
        self.post_json::<_, SingleEnvelope<IdOnly>>("/v1/bundleIds", &body).map(|resp| resp.data.id).or_else(|e| {
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
}

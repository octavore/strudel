use anyhow::Result;
use clml::cprintln;
use serde::Deserialize;
use serde_json::json;

use super::client::AppStoreClient;
use super::types::{IdOnly, ListEnvelope, PortalDevice, Resource, SingleEnvelope};

impl AppStoreClient {
    /// List registered iOS devices with ENABLED status.
    pub fn list_devices(&self) -> Result<Vec<PortalDevice>> {
        cprintln!("<dim>Listing registered iOS devices on App Store Connect...</dim>");
        #[derive(Deserialize)]
        struct DeviceAttrs {
            name: String,
            udid: String,
        }

        let list: ListEnvelope<Resource<DeviceAttrs>> =
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
        let body = json!({
            "data": {
                "type": "devices",
                "attributes": {"name": name, "udid": udid, "platform": "IOS"}
            }
        });
        let resp: SingleEnvelope<IdOnly> = self.post_json("/v1/devices", &body)?;
        Ok(resp.data.id)
    }
}

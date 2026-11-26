use anyhow::Result;
use clml::cformat;
use serde::Deserialize;
use serde_json::json;

use super::client::AppStoreClient;
use super::types::{IdOnly, ListEnvelope, PortalDevice, Resource, SingleEnvelope};
use crate::config::Platform;

/// ASC device `platform` value.
fn device_platform(platform: Platform) -> &'static str {
    match platform {
        Platform::Ios => "IOS",
        Platform::Macos => "MAC_OS",
    }
}

impl AppStoreClient {
    /// List registered iOS devices with ENABLED status.
    pub fn list_devices(&self) -> Result<Vec<PortalDevice>> {
        self.list_devices_on(Platform::Ios)
    }

    /// List registered devices of `platform` with ENABLED status.
    pub fn list_devices_on(&self, platform: Platform) -> Result<Vec<PortalDevice>> {
        self.progress(cformat!(
            "<dim>Listing registered {} devices on App Store Connect...</dim>",
            platform.label()
        ));
        #[derive(Deserialize)]
        struct DeviceAttrs {
            name: String,
            udid: String,
        }

        let list: ListEnvelope<Resource<DeviceAttrs>> = self.get_json(&format!(
            "/v1/devices?filter[platform]={}&filter[status]=ENABLED&limit=200",
            device_platform(platform)
        ))?;
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

    /// Register a device of `platform` unless the portal lists it. Returns
    /// whether the device was registered. A conflict means another run
    /// registered it.
    pub fn ensure_device_registered(
        &self,
        name: &str,
        udid: &str,
        platform: Platform,
    ) -> Result<bool> {
        if self
            .list_devices_on(platform)?
            .iter()
            .any(|d| d.udid == udid)
        {
            return Ok(false);
        }
        match self.register_device_on(name, udid, platform) {
            Ok(_) => Ok(true),
            Err(e) if e.to_string().contains("409") => Ok(false),
            Err(e) if e.to_string().contains("403") => Err(e.context(format!(
                "Insufficient permissions to register device {name} ({udid}). Registering \
                 devices on App Store Connect needs the Admin role."
            ))),
            Err(e) => Err(e),
        }
    }

    /// Register an iOS device on the portal. Returns the resource ID.
    pub fn register_device(&self, name: &str, udid: &str) -> Result<String> {
        self.register_device_on(name, udid, Platform::Ios)
    }

    /// Register a device of `platform` on the portal. Returns the resource ID.
    pub fn register_device_on(&self, name: &str, udid: &str, platform: Platform) -> Result<String> {
        let body = json!({
            "data": {
                "type": "devices",
                "attributes": {
                    "name": name,
                    "udid": udid,
                    "platform": device_platform(platform)
                }
            }
        });
        let resp: SingleEnvelope<IdOnly> = self.post_json("/v1/devices", &body)?;
        Ok(resp.data.id)
    }
}

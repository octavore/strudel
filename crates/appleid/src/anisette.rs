use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSBundle, NSString};
use uuid::Uuid;

pub struct AnisetteProvider {
    pub device_id: String,
}

impl AnisetteProvider {
    pub fn new() -> Result<Self> {
        load_frameworks()?;
        let device_id = fetch_device_id_from_akdevice()
            .unwrap_or_else(|_| Uuid::new_v4().to_string().to_uppercase());
        Ok(AnisetteProvider { device_id })
    }

    pub fn headers(&self, dsid: &str) -> Result<HashMap<String, String>> {
        // SAFETY: we call ObjC class/instance methods on loaded private frameworks.
        unsafe {
            let mut h = HashMap::new();

            // OTP headers from AOSUtilities (class method, not instance method).
            let aos_cls = AnyClass::get(c"AOSUtilities").context("AOSUtilities class not found")?;
            let dsid_ns = NSString::from_str(dsid);
            let sel = objc2::sel!(retrieveOTPHeadersForDSID:);
            let responds: bool = objc2::msg_send![aos_cls, respondsToSelector: sel];
            if !responds {
                bail!(
                    "AOSUtilities does not respond to +retrieveOTPHeadersForDSID: on this macOS version"
                );
            }
            // AOSKit sometimes returns a partial OTP dict (missing X-Apple-MD) on
            // the first call right after the frameworks are loaded. We might need to retry
            // a few times if we see this causing an error (i.e. if sending GSA a request
            // with an incomplete identity is rejected)
            let otp_result: Option<Retained<AnyObject>> =
                objc2::msg_send![aos_cls, retrieveOTPHeadersForDSID: &*dsid_ns];
            let otp_dict = otp_result.context("retrieveOTPHeadersForDSID: returned nil")?;

            // OTP headers: only available when the account is registered with
            // AOSKit (i.e. signed in via System Settings > Apple Account).
            // Omit them silently rather than failing — Apple may accept the
            // request without them when the gs_token is otherwise valid.
            if let Ok(v) = nsobj_string(&otp_dict, "X-Apple-MD") {
                h.insert("X-Apple-I-MD".to_string(), v);
            }
            if let Ok(v) = nsobj_string(&otp_dict, "X-Apple-MD-M") {
                h.insert("X-Apple-I-MD-M".to_string(), v);
            }
            h.insert("X-Apple-I-MD-RINFO".to_string(), "17106176".to_string());

            // Machine serial number from AOSUtilities.
            let srl_sel = objc2::sel!(machineSerialNumber);
            let srl_responds: bool = objc2::msg_send![aos_cls, respondsToSelector: srl_sel];
            let serial = if srl_responds {
                let srl: Option<Retained<NSString>> =
                    objc2::msg_send![aos_cls, machineSerialNumber];
                srl.map(|s| s.to_string())
                    .unwrap_or_else(|| "0".to_string())
            } else {
                "0".to_string()
            };
            h.insert("X-Apple-I-SRL-NO".to_string(), serial);

            // Device info from AKDevice in AuthKit.
            h.insert("X-Mme-Device-Id".to_string(), self.device_id.clone());
            if let Some(ak_cls) = AnyClass::get(c"AKDevice") {
                let device: Option<Retained<AnyObject>> = objc2::msg_send![ak_cls, currentDevice];
                if let Some(device) = device {
                    let lu: Option<Retained<NSString>> = objc2::msg_send![&*device, localUserUUID];
                    if let Some(lu) = lu {
                        h.insert("X-Apple-I-MD-LU".to_string(), lu.to_string());
                    }
                    let sfd: Option<Retained<NSString>> =
                        objc2::msg_send![&*device, serverFriendlyDescription];
                    if let Some(sfd) = sfd {
                        h.insert("X-Mme-Client-Info".to_string(), sfd.to_string());
                    }
                }
            }

            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            h.insert("X-Apple-I-Client-Time".to_string(), now);
            h.insert("X-Apple-I-TimeZone".to_string(), "UTC".to_string());
            h.insert("X-Apple-Locale".to_string(), "en_US".to_string());
            h.insert("X-Apple-I-Locale".to_string(), "en_US".to_string());
            Ok(h)
        }
    }
}

fn load_frameworks() -> Result<()> {
    // SAFETY: loading real macOS private frameworks read-only.
    unsafe {
        for path in [
            "/System/Library/PrivateFrameworks/AOSKit.framework",
            "/System/Library/PrivateFrameworks/AuthKit.framework",
        ] {
            let path_ns = NSString::from_str(path);
            let bundle =
                NSBundle::bundleWithPath(&path_ns).with_context(|| format!("{path} not found"))?;
            if !bundle.isLoaded() && !bundle.load() {
                bail!("Failed to load {path}");
            }
        }
    }
    Ok(())
}

fn fetch_device_id_from_akdevice() -> Result<String> {
    // SAFETY: called after load_frameworks(); AuthKit is loaded.
    unsafe {
        let ak_cls = AnyClass::get(c"AKDevice").context("AKDevice not found")?;
        let device: Option<Retained<AnyObject>> = objc2::msg_send![ak_cls, currentDevice];
        let device = device.context("AKDevice.currentDevice returned nil")?;
        let val: Option<Retained<NSString>> = objc2::msg_send![&*device, uniqueDeviceIdentifier];
        val.map(|s| s.to_string())
            .context("AKDevice.uniqueDeviceIdentifier returned nil")
    }
}

fn nsobj_string(obj: &AnyObject, key: &str) -> Result<String> {
    // SAFETY: caller guarantees obj responds to objectForKey:.
    unsafe {
        let key_ns = NSString::from_str(key);
        let val: Option<Retained<NSString>> = objc2::msg_send![obj, objectForKey: &*key_ns];
        val.map(|s| s.to_string())
            .with_context(|| format!("missing anisette key: {key}"))
    }
}

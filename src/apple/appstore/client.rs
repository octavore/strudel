use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use indoc::indoc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::apple::appstore::types::{ApiErrors, Claims};
use crate::config::ResolvedConfig;

pub struct AppStoreClient {
    key_id: String,
    issuer: String,
    key_pem: Vec<u8>,
    agent: ureq::Agent,
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

    pub(super) fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
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

    pub(super) fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
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

    pub(super) fn delete(&self, path: &str) -> Result<()> {
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

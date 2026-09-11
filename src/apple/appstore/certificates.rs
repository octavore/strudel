use anyhow::{Result, bail};
use clml::cprintln;
use serde::Deserialize;

use super::client::AppStoreClient;
use super::types::{Cert, ListEnvelope, Resource};

impl AppStoreClient {
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
        struct CertAttrs {
            name: String,
        }

        cprintln!("<dim>Listing {certificate_type} certificates on App Store Connect...</dim>");
        let list: ListEnvelope<Resource<CertAttrs>> = self.get_json(&format!(
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
}

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(super) struct Claims {
    pub(super) iss: String,
    pub(super) iat: u64,
    pub(super) exp: u64,
    pub(super) aud: String,
}

#[derive(Deserialize)]
pub(super) struct ApiErrors {
    pub(super) errors: Vec<ApiError>,
}

#[derive(Deserialize)]
pub(super) struct ApiError {
    pub(super) title: String,
    pub(super) detail: String,
}

/// A JSON:API "many" response: `{"data": [...]}`.
#[derive(Deserialize)]
pub(super) struct ListEnvelope<T> {
    pub(super) data: Vec<T>,
}

/// A JSON:API "one" response: `{"data": {...}}`.
#[derive(Deserialize)]
pub(super) struct SingleEnvelope<T> {
    pub(super) data: T,
}

/// A JSON:API resource with an id and typed attributes.
#[derive(Deserialize)]
pub(super) struct Resource<A> {
    pub(super) id: String,
    pub(super) attributes: A,
}

/// A JSON:API resource where only the attributes are needed.
#[derive(Deserialize)]
pub(super) struct Attrs<A> {
    pub(super) attributes: A,
}

/// A JSON:API resource where only the id is needed.
#[derive(Deserialize)]
pub(super) struct IdOnly {
    pub(super) id: String,
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

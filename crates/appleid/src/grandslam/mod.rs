use std::collections::HashMap;
use std::io::{Cursor, Read};

use anyhow::{Context, Result, bail};
use num_bigint::BigUint;
use rand::Rng;

use crate::Session;
use crate::anisette::AnisetteProvider;

mod srp;
use srp::{compute_m1, decrypt_spd, derive_x, pad256, sha256, srp_n};

const GSA_URL: &str = "https://gsa.apple.com/grandslam/GsService2";
const AUTH_URL: &str = "https://gsa.apple.com/auth";

fn build_cpd(headers: &HashMap<String, String>) -> plist::Value {
    let mut cpd = plist::Dictionary::new();
    for (k, v) in headers {
        cpd.insert(k.clone(), plist::Value::String(v.clone()));
    }
    cpd.insert(
        "AppleIDClientIdentifier".to_string(),
        plist::Value::String("D4B7512F-E841-4AEA-A569-4F1E84738182".to_string()),
    );
    cpd.insert("bootstrap".to_string(), plist::Value::Boolean(true));
    cpd.insert(
        "capp".to_string(),
        plist::Value::String("AppStore".to_string()),
    );
    cpd.insert("ckgen".to_string(), plist::Value::Boolean(true));
    cpd.insert(
        "dc".to_string(),
        plist::Value::String("#d4c5b3".to_string()),
    );
    cpd.insert(
        "dec".to_string(),
        plist::Value::String("#e1e4e3".to_string()),
    );
    cpd.insert("loc".to_string(), plist::Value::String("en_US".to_string()));
    cpd.insert("pbe".to_string(), plist::Value::Boolean(false));
    cpd.insert(
        "prtn".to_string(),
        plist::Value::String("ME349".to_string()),
    );
    cpd.insert(
        "svct".to_string(),
        plist::Value::String("iTunes".to_string()),
    );
    plist::Value::Dictionary(cpd)
}

fn gsa_post(
    agent: &ureq::Agent,
    request: &plist::Value,
    anisette_headers: &HashMap<String, String>,
) -> Result<plist::Value> {
    // All GSA requests use a Header/Request envelope; responses come back under
    // Response.
    let mut header = plist::Dictionary::new();
    header.insert(
        "Version".to_string(),
        plist::Value::String("1.0.1".to_string()),
    );
    let mut outer = plist::Dictionary::new();
    outer.insert("Header".to_string(), plist::Value::Dictionary(header));
    outer.insert("Request".to_string(), request.clone());

    let mut buf = Vec::new();
    plist::Value::Dictionary(outer)
        .to_writer_xml(&mut buf)
        .context("serializing GSA request plist")?;

    let mut req = agent
        .post(GSA_URL)
        .header("Content-Type", "text/x-xml-plist")
        .header("Accept", "text/x-xml-plist")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("User-Agent", "akd/1.0 CFNetwork/1490.0.4 Darwin/24.6.0")
        .header(
            "X-MMe-Client-Info",
            "<MacBookPro18,3> <macOS;15.5;24F74> <com.apple.AuthKit/1 (com.apple.akd/1.0)>",
        );
    for (k, v) in anisette_headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req
        .send(&buf[..])
        .map_err(|e| anyhow::anyhow!("GSA network error: {e}"))?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut body_bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .read_to_end(&mut body_bytes)
        .context("reading GSA response body")?;

    if !status.is_success() {
        bail!(
            "GSA returned HTTP {status} (content-type: {content_type}): {}",
            String::from_utf8_lossy(&body_bytes)
        );
    }

    let top = plist::Value::from_reader(Cursor::new(&body_bytes))
        .with_context(|| {
            format!(
                "parsing GSA response plist (status: {status}, content-type: {content_type}, body: {:?})",
                String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(200)])
            )
        })?;

    let top_dict = top.as_dictionary().context("GSA response is not a dict")?;
    check_gsa_status(top_dict)?;
    let response = top_dict
        .get("Response")
        .and_then(|v| v.as_dictionary())
        .context("GSA response missing 'Response' key")?;
    check_gsa_status(response)?;
    Ok(plist::Value::Dictionary(response.clone()))
}

pub fn login(
    agent: &ureq::Agent,
    anisette: &AnisetteProvider,
    apple_id: &str,
    password: &str,
    mut two_factor: impl FnMut() -> Result<String>,
) -> Result<Session> {
    let n = srp_n();

    let mut a_bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut a_bytes);
    let a = BigUint::from_bytes_be(&a_bytes);
    let g = BigUint::from(2u32);
    let a_pub = g.modpow(&a, &n);

    let anisette_hdrs = anisette.headers("-2")?;
    let cpd = build_cpd(&anisette_hdrs);

    // --- SRP init ---
    let mut init_dict = plist::Dictionary::new();
    init_dict.insert("o".to_string(), plist::Value::String("init".to_string()));
    init_dict.insert("u".to_string(), plist::Value::String(apple_id.to_string()));
    // A2k is sent zero-padded to 256 bytes. The server re-parses this as a big
    // integer and tolerates the padded form, so we keep the fixed width here.
    init_dict.insert(
        "A2k".to_string(),
        plist::Value::Data(pad256(&a_pub).to_vec()),
    );
    init_dict.insert(
        "ps".to_string(),
        plist::Value::Array(vec![
            plist::Value::String("s2k".to_string()),
            plist::Value::String("s2k_fo".to_string()),
        ]),
    );
    init_dict.insert("cpd".to_string(), cpd.clone());

    let init_resp = gsa_post(agent, &plist::Value::Dictionary(init_dict), &anisette_hdrs)?;
    let init = init_resp
        .as_dictionary()
        .context("GSA init response is not a dict")?;

    let salt = init
        .get("s")
        .and_then(|v| v.as_data())
        .context("missing salt in GSA init response")?
        .to_vec();
    let b_bytes = init
        .get("B")
        .and_then(|v| v.as_data())
        .context("missing B in GSA init response")?
        .to_vec();
    let iterations = init
        .get("i")
        .and_then(|v| v.as_unsigned_integer())
        .context("missing iteration count in GSA init response")? as u32;
    let proto = init
        .get("sp")
        .and_then(|v| v.as_string())
        .unwrap_or("s2k")
        .to_string();
    let cookie = init
        .get("c")
        .and_then(|v| v.as_string())
        .context("missing cookie in GSA init response")?
        .to_string();

    let b_pub = BigUint::from_bytes_be(&b_bytes);
    let x_bytes = derive_x(password, &salt, iterations, &proto);
    let x = BigUint::from_bytes_be(&x_bytes);

    // k = H(N || g_padded_to_256)
    let mut g_padded = [0u8; 256];
    g_padded[255] = 2;
    let n_padded = pad256(&n);
    let mut k_input = n_padded.to_vec();
    k_input.extend_from_slice(&g_padded);
    let k = BigUint::from_bytes_be(&sha256(&k_input));

    // u = H(A_padded || B_padded)
    let a_padded = pad256(&a_pub);
    let b_padded = pad256(&b_pub);
    let mut u_input = a_padded.to_vec();
    u_input.extend_from_slice(&b_padded);
    let u = BigUint::from_bytes_be(&sha256(&u_input));

    // S = (B - k*g^x mod N)^(a + u*x) mod N
    let kgx = (&k * g.modpow(&x, &n)) % &n;
    let b_minus_kgx = if b_pub >= kgx {
        (&b_pub - &kgx) % &n
    } else {
        (&n - &kgx + &b_pub) % &n
    };
    let srp_s = b_minus_kgx.modpow(&(&a + &u * &x), &n);
    let k_srp = sha256(&srp_s.to_bytes_be());

    let m1 = compute_m1(&n, apple_id, &salt, &a_pub, &b_pub, &k_srp);

    // --- SRP complete ---
    let mut complete_dict = plist::Dictionary::new();
    complete_dict.insert(
        "o".to_string(),
        plist::Value::String("complete".to_string()),
    );
    complete_dict.insert("u".to_string(), plist::Value::String(apple_id.to_string()));
    complete_dict.insert("M1".to_string(), plist::Value::Data(m1.to_vec()));
    complete_dict.insert("c".to_string(), plist::Value::String(cookie));
    let complete_anisette_hdrs = anisette.headers("-2")?;
    complete_dict.insert("cpd".to_string(), build_cpd(&complete_anisette_hdrs));

    let complete_resp = gsa_post(
        agent,
        &plist::Value::Dictionary(complete_dict),
        &complete_anisette_hdrs,
    )?;
    let complete = complete_resp
        .as_dictionary()
        .context("GSA complete response is not a dict")?;

    let spd_bytes = complete
        .get("spd")
        .and_then(|v| v.as_data())
        .context("missing spd in GSA complete response")?
        .to_vec();

    let session_plist = decrypt_spd(&spd_bytes, &k_srp)?;
    let session = session_plist
        .as_dictionary()
        .context("decrypted spd is not a plist dict")?;

    let dsid = session
        .get("adsid")
        .or_else(|| session.get("dsid"))
        .and_then(|v| v.as_string())
        .context("missing dsid in decrypted spd")?
        .to_string();
    let gs_token = session
        .get("GsIdmsToken")
        .and_then(|v| v.as_string())
        .context("missing GsIdmsToken in decrypted spd")?
        .to_string();

    let auth_type = complete.get("au").and_then(|v| v.as_string()).unwrap_or("");

    if auth_type == "trustedDeviceSecondaryAuth" || auth_type == "secondaryAuth" {
        handle_two_factor(agent, anisette, &dsid, &gs_token, &mut two_factor)?;
    }

    Ok(Session { dsid, gs_token })
}

fn handle_two_factor(
    agent: &ureq::Agent,
    anisette: &AnisetteProvider,
    dsid: &str,
    gs_token: &str,
    two_factor: &mut impl FnMut() -> Result<String>,
) -> Result<()> {
    let anisette_hdrs = anisette.headers(dsid)?;

    // Trigger the push to the user's trusted device.
    let trigger_url = format!("{AUTH_URL}/verify/trusteddevice");
    let mut builder = agent
        .get(&trigger_url)
        .header("Accept", "application/json")
        .header("X-Apple-GS-Token", gs_token)
        .header("X-Apple-DS-ID", dsid);
    for (k, v) in &anisette_hdrs {
        builder = builder.header(k, v);
    }
    builder
        .call()
        .map_err(|e| anyhow::anyhow!("2FA trigger request failed: {e}"))?;

    let code = two_factor()?;
    let code = code.trim().to_string();

    let submit_url = format!("{AUTH_URL}/verify/trusteddevice/securitycode");
    let body = serde_json::json!({ "securityCode": { "code": code } });

    let mut builder = agent
        .post(&submit_url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("X-Apple-GS-Token", gs_token)
        .header("X-Apple-DS-ID", dsid);
    for (k, v) in &anisette_hdrs {
        builder = builder.header(k, v);
    }
    let mut resp = builder
        .send_json(&body)
        .map_err(|e| anyhow::anyhow!("2FA code submission failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        bail!("2FA verification failed ({status}): {body}");
    }

    Ok(())
}

fn check_gsa_status(dict: &plist::Dictionary) -> Result<()> {
    if let Some(status) = dict.get("Status").and_then(|v| v.as_dictionary()) {
        let code = status
            .get("ec")
            .and_then(|v| v.as_signed_integer())
            .unwrap_or(0);
        if code != 0 {
            let msg = status
                .get("em")
                .and_then(|v| v.as_string())
                .unwrap_or("unknown error");
            bail!("Apple ID authentication failed (code {code}): {msg}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_serde_roundtrip() {
        let s = Session {
            dsid: "12345678".to_string(),
            gs_token: "tok_abc123".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let s2: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.dsid, s.dsid);
        assert_eq!(s2.gs_token, s.gs_token);
    }
}

use std::collections::HashMap;
use std::io::{Cursor, Read};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use num_bigint::BigUint;
use rand::Rng;

use crate::Session;
use crate::anisette::AnisetteProvider;

mod srp;
use srp::{
    app_tokens_checksum, compute_m1, decrypt_app_token, decrypt_spd, derive_x, pad256, sha256,
    srp_n,
};

// App token requested from GSA; the developer-services portal authenticates
// with this token rather than the raw GsIdmsToken.
const XCODE_APP: &str = "com.apple.gs.xcode.auth";

const GSA_URL: &str = "https://gsa.apple.com/grandslam/GsService2";
const AUTH_URL: &str = "https://gsa.apple.com/auth";

fn build_cpd(headers: &HashMap<String, String>) -> plist::Value {
    let mut cpd = plist::Dictionary::new();
    for (k, v) in headers {
        cpd.insert(k.clone(), v.clone().into());
    }
    cpd.insert(
        "AppleIDClientIdentifier".to_string(),
        "D4B7512F-E841-4AEA-A569-4F1E84738182".into(),
    );
    cpd.insert("bootstrap".to_string(), true.into());
    cpd.insert("capp".to_string(), "AppStore".into());
    cpd.insert("ckgen".to_string(), true.into());
    cpd.insert("dc".to_string(), "#d4c5b3".into());
    cpd.insert("dec".to_string(), "#e1e4e3".into());
    cpd.insert("loc".to_string(), "en_US".into());
    cpd.insert("pbe".to_string(), false.into());
    cpd.insert("prtn".to_string(), "ME349".into());
    cpd.insert("svct".to_string(), "iTunes".into());
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
    header.insert("Version".to_string(), "1.0.1".into());
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
    // Apple requires two-factor to be completed, then the whole SRP handshake
    // re-run from a now-trusted device. Loop until we get a non-2FA result.
    for attempt in 0..2 {
        let spd = srp_exchange(agent, anisette, apple_id, password)?;
        let session = spd
            .as_dictionary()
            .context("decrypted spd is not a plist dict")?;

        let dsid = session
            .get("adsid")
            .or_else(|| session.get("dsid"))
            .and_then(|v| v.as_string())
            .context("missing dsid in decrypted spd")?
            .to_string();
        let idms_token = session
            .get("GsIdmsToken")
            .and_then(|v| v.as_string())
            .context("missing GsIdmsToken in decrypted spd")?
            .to_string();

        let auth_type = session
            .get("auth_type_marker")
            .and_then(|v| v.as_string())
            .unwrap_or("");

        if auth_type == "trustedDeviceSecondaryAuth" || auth_type == "secondaryAuth" {
            if attempt == 1 {
                bail!("Two-factor authentication did not complete; please try again.");
            }
            handle_two_factor(agent, anisette, &dsid, &idms_token, &mut two_factor)?;
            continue;
        }

        // Exchange the IDMS token for an Xcode app token. The developer-services
        // portal authenticates with this token, not the raw GsIdmsToken.
        let sk = session
            .get("sk")
            .and_then(|v| v.as_data())
            .context("missing sk in decrypted spd")?;
        let c = session
            .get("c")
            .and_then(|v| v.as_data())
            .context("missing c in decrypted spd")?;
        let gs_token = fetch_app_token(agent, anisette, &dsid, &idms_token, sk, c)?;

        return Ok(Session {
            apple_id: apple_id.to_string(),
            dsid,
            gs_token,
        });
    }
    unreachable!("login loop always returns or bails within 2 attempts")
}

/// Run the GSA SRP init/complete handshake and return the decrypted SPD as a
/// plist dictionary. The `au` (auth-type) field from the complete response is
/// folded into the SPD under the synthetic `auth_type_marker` key so the caller
/// can decide whether two-factor is required.
fn srp_exchange(
    agent: &ureq::Agent,
    anisette: &AnisetteProvider,
    apple_id: &str,
    password: &str,
) -> Result<plist::Value> {
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
    init_dict.insert("o".to_string(), "init".into());
    init_dict.insert("u".to_string(), apple_id.into());
    // A2k is sent zero-padded to 256 bytes. The server re-parses this as a big
    // integer and tolerates the padded form, so we keep the fixed width here.
    init_dict.insert(
        "A2k".to_string(),
        plist::Value::Data(pad256(&a_pub).to_vec()),
    );
    init_dict.insert(
        "ps".to_string(),
        plist::Value::Array(vec!["s2k".into(), "s2k_fo".into()]),
    );
    init_dict.insert("cpd".to_string(), cpd.clone());

    let init_resp = gsa_post(agent, &plist::Value::Dictionary(init_dict), &anisette_hdrs)
        .context("GSA SRP init")?;
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
    complete_dict.insert("o".to_string(), "complete".into());
    complete_dict.insert("u".to_string(), apple_id.into());
    complete_dict.insert("M1".to_string(), plist::Value::Data(m1.to_vec()));
    complete_dict.insert("c".to_string(), cookie.into());
    let complete_anisette_hdrs = anisette.headers("-2")?;
    complete_dict.insert("cpd".to_string(), build_cpd(&complete_anisette_hdrs));

    let complete_resp = gsa_post(
        agent,
        &plist::Value::Dictionary(complete_dict),
        &complete_anisette_hdrs,
    )
    .context("GSA SRP complete")?;
    let complete = complete_resp
        .as_dictionary()
        .context("GSA complete response is not a dict")?;

    let spd_bytes = complete
        .get("spd")
        .and_then(|v| v.as_data())
        .context("missing spd in GSA complete response")?
        .to_vec();

    let session_plist = decrypt_spd(&spd_bytes, &k_srp)?;
    let mut session = session_plist
        .into_dictionary()
        .context("decrypted spd is not a plist dict")?;

    // Fold the complete response's auth-type into the SPD so the caller can
    // detect a two-factor challenge without re-threading the response. Apple
    // returns it as `au` inside the `Status` sub-dictionary.
    let auth_type = complete
        .get("Status")
        .and_then(|v| v.as_dictionary())
        .and_then(|s| s.get("au"))
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    session.insert("auth_type_marker".to_string(), auth_type.into());

    Ok(plist::Value::Dictionary(session))
}

/// Exchange the IDMS token for an app-specific token scoped to
/// `com.apple.gs.xcode.auth` via the GSA `apptokens` request. The portal
/// authenticates developer-services calls with this token.
fn fetch_app_token(
    agent: &ureq::Agent,
    anisette: &AnisetteProvider,
    dsid: &str,
    idms_token: &str,
    sk: &[u8],
    c: &[u8],
) -> Result<String> {
    let checksum = app_tokens_checksum(sk, dsid, &[XCODE_APP]);
    // "-2" requests the machine-level OTP. A real DSID only returns valid OTP
    // headers when the account is provisioned in AOSKit on this Mac; otherwise
    // the machine headers come back empty and GSA rejects the request (-22410).
    let anisette_hdrs = anisette.headers("-2")?;

    let mut req = plist::Dictionary::new();
    req.insert("u".to_string(), dsid.into());
    req.insert(
        "app".to_string(),
        plist::Value::Array(vec![XCODE_APP.into()]),
    );
    req.insert("c".to_string(), plist::Value::Data(c.to_vec()));
    req.insert("t".to_string(), idms_token.into());
    req.insert(
        "checksum".to_string(),
        plist::Value::Data(checksum.to_vec()),
    );
    req.insert("cpd".to_string(), build_cpd(&anisette_hdrs));
    req.insert("o".to_string(), "apptokens".into());

    let resp = gsa_post(agent, &plist::Value::Dictionary(req), &anisette_hdrs)
        .context("GSA app-token exchange (apptokens)")?;
    let resp = resp
        .as_dictionary()
        .context("GSA apptokens response is not a dict")?;

    let et = resp
        .get("et")
        .and_then(|v| v.as_data())
        .context("missing encrypted token (et) in apptokens response")?;

    let decrypted = decrypt_app_token(sk, et)?;
    let token_plist = plist::Value::from_reader(Cursor::new(decrypted))
        .context("parsing decrypted app token plist")?;
    let tokens = token_plist
        .as_dictionary()
        .and_then(|d| d.get("t"))
        .and_then(|v| v.as_dictionary())
        .context("missing token map in app token response")?;
    let token = tokens
        .get(XCODE_APP)
        .and_then(|v| v.as_dictionary())
        .and_then(|d| d.get("token"))
        .and_then(|v| v.as_string())
        .context("missing Xcode app token in response")?;

    Ok(token.to_string())
}

fn handle_two_factor(
    agent: &ureq::Agent,
    anisette: &AnisetteProvider,
    dsid: &str,
    idms_token: &str,
    two_factor: &mut impl FnMut() -> Result<String>,
) -> Result<()> {
    // "-2" yields the machine-level OTP headers (see fetch_app_token).
    let anisette_hdrs = anisette.headers("-2")?;
    // These GSA endpoints authenticate with the identity token
    // base64("<adsid>:<GsIdmsToken>"), the same scheme Xcode uses.
    let identity_token = BASE64.encode(format!("{dsid}:{idms_token}"));

    // Headers shared by the trigger and validate calls. Anisette headers are
    // appended to these.
    let mut common: Vec<(String, String)> = vec![
        ("Content-Type".into(), "text/x-xml-plist".into()),
        ("User-Agent".into(), "Xcode".into()),
        ("Accept".into(), "text/x-xml-plist".into()),
        ("Accept-Language".into(), "en-us".into()),
        ("X-Apple-App-Info".into(), "com.apple.gs.xcode.auth".into()),
        ("X-Xcode-Version".into(), "11.2 (11B41)".into()),
        ("X-Apple-Identity-Token".into(), identity_token),
    ];
    common.extend(anisette_hdrs);

    // Trigger the push to the user's trusted devices.
    let trigger_url = format!("{AUTH_URL}/verify/trusteddevice");
    let mut builder = agent.get(&trigger_url);
    for (k, v) in &common {
        builder = builder.header(k, v);
    }
    builder
        .call()
        .map_err(|e| anyhow::anyhow!("2FA trigger request failed: {e}"))?;

    let code = two_factor()?;
    let code = code.trim().to_string();

    // Validate the code against GsService2/validate.
    let validate_url = format!("{GSA_URL}/validate");
    let mut builder = agent.get(&validate_url).header("security-code", &code);
    for (k, v) in &common {
        builder = builder.header(k, v);
    }
    let mut resp = builder
        .call()
        .map_err(|e| anyhow::anyhow!("2FA code validation failed: {e}"))?;

    let body = resp
        .body_mut()
        .read_to_string()
        .context("reading 2FA validation response")?;
    let val = plist::Value::from_reader(Cursor::new(body.as_bytes()))
        .with_context(|| format!("parsing 2FA validation response: {body:?}"))?;
    let dict = val
        .as_dictionary()
        .context("2FA validation response is not a dict")?;

    // `ec` may come back as an integer or a string; treat 0 as success.
    let ec = dict
        .get("ec")
        .and_then(|v| {
            v.as_signed_integer()
                .or_else(|| v.as_string().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0);
    if ec != 0 {
        if ec == -21669 {
            bail!("Incorrect verification code. Run `strudel login` to try again.");
        }
        let em = dict
            .get("em")
            .and_then(|v| v.as_string())
            .unwrap_or("unknown error");
        bail!("Two-factor verification failed (code {ec}): {em}");
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
            apple_id: "me@example.com".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let s2: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.dsid, s.dsid);
        assert_eq!(s2.gs_token, s.gs_token);
        assert_eq!(s2.apple_id, s.apple_id);
    }
}

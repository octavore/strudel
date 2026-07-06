//! Persistent keychain for a free-provisioning development identity. Unlike the
//! ephemeral [`super::temp`] keychain, this one is kept across builds (and left
//! in the user search list) so the self-generated cert can be reused until it
//! expires. Only [`crate::apple::provisioning`] calls into here.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

const DEV_KC_PASSWORD: &str = "strudel-dev";

/// Import a DER certificate + PEM private key into a persistent keychain at
/// `keychain_db`. The keychain is created if it does not exist.
/// `set-key-partition-list` is applied so codesign can use the key without an
/// interactive prompt.
pub fn import_dev_cert(cert_der: &[u8], key_pem: &[u8], keychain_db: &Path) -> Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix("strudel-cert-")
        .tempdir()
        .context("creating temp dir for cert import")?;
    let cert_der_path = tmp.path().join("cert.der");
    let cert_pem_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    let p12_path = tmp.path().join("dev.p12");

    fs::write(&cert_der_path, cert_der).context("writing temp cert")?;
    fs::write(&key_path, key_pem).context("writing temp key")?;

    // `openssl pkcs12 -export` expects a PEM certificate, so convert the DER first.
    let out = std::process::Command::new("openssl")
        .args([
            "x509",
            "-inform",
            "DER",
            "-in",
            cert_der_path.to_str().unwrap(),
            "-out",
            cert_pem_path.to_str().unwrap(),
        ])
        .output()
        .context("running openssl x509 to convert dev cert to PEM")?;
    if !out.status.success() {
        bail!(
            "openssl x509 (DER to PEM) failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let out = std::process::Command::new("openssl")
        .args([
            "pkcs12",
            "-export",
            "-in",
            cert_pem_path.to_str().unwrap(),
            "-inkey",
            key_path.to_str().unwrap(),
            "-out",
            p12_path.to_str().unwrap(),
            "-passout",
            &format!("pass:{DEV_KC_PASSWORD}"),
        ])
        .output()
        .context("running openssl pkcs12")?;
    if !out.status.success() {
        bail!(
            "openssl pkcs12 failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let kc_str = keychain_db.to_str().unwrap();

    // Create keychain (ignore error when it already exists).
    std::process::Command::new("security")
        .args(["create-keychain", "-p", DEV_KC_PASSWORD, kc_str])
        .output()
        .ok();

    let status = std::process::Command::new("security")
        .args(["unlock-keychain", "-p", DEV_KC_PASSWORD, kc_str])
        .status()
        .context("unlocking dev keychain")?;
    if !status.success() {
        bail!("security unlock-keychain failed for {kc_str}");
    }

    let p12_str = p12_path.to_str().unwrap();
    let status = std::process::Command::new("security")
        .args([
            "import",
            p12_str,
            "-P",
            DEV_KC_PASSWORD,
            "-A",
            "-t",
            "cert",
            "-f",
            "pkcs12",
            "-k",
            kc_str,
        ])
        .status()
        .context("importing PKCS#12 into dev keychain")?;
    if !status.success() {
        bail!("security import failed");
    }

    // Allow codesign to use the key without an interactive prompt.
    std::process::Command::new("security")
        .args([
            "set-key-partition-list",
            "-S",
            "apple-tool:,apple:",
            "-s",
            "-k",
            DEV_KC_PASSWORD,
            kc_str,
        ])
        .status()
        .context("setting key partition list on dev keychain")?;

    Ok(())
}

/// Ensure `keychain_db` is in the user's keychain search list, prepending it
/// if it isn't already present.
pub fn ensure_keychain_in_search_list(keychain_db: &Path) -> Result<()> {
    let kc_str = keychain_db.to_str().unwrap();
    let out = std::process::Command::new("security")
        .args(["list-keychains", "-d", "user"])
        .output()
        .context("listing user keychains")?;
    let existing = String::from_utf8_lossy(&out.stdout);
    if existing.contains(kc_str) {
        return Ok(());
    }
    let existing_keychains: Vec<&str> = existing
        .lines()
        .map(|l| l.trim().trim_matches('"'))
        .filter(|l| !l.is_empty())
        .collect();
    let mut args = vec!["list-keychains", "-d", "user", "-s", kc_str];
    args.extend(existing_keychains);
    std::process::Command::new("security")
        .args(&args)
        .status()
        .context("adding dev keychain to search list")?;
    Ok(())
}

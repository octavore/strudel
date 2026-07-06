//! `strudel login status` prints out global config, the Apple ID session stored
//! `~/.local/share/strudel/`, and the per-project `.strudel/` provisioning
//! artifacts.

use std::path::{Path, PathBuf};

use anyhow::Result;
use appleid::Session;
use color_print::cprintln;

use crate::config::{
    GlobalConfig, IosProvisioningBackend, ResolvedConfig, ResolvedTargetPlatform, load_config,
};
use crate::devices::DeviceSet;
use crate::paths::{Paths, StrudelData};

/// Print the global config, Apple ID session, and per-project provisioning
/// state. `config_path`/`target` mirror the other commands; the project
/// section is best-effort and omitted when no config loads.
pub fn run(config_path: &Path, target: Option<&str>) -> Result<()> {
    global_config_section()?;
    apple_id_section()?;
    project_section(config_path, target);
    Ok(())
}

fn global_config_section() -> Result<()> {
    let path = GlobalConfig::xdg_path()?;
    header("Global config", &path);
    if !path.exists() {
        cprintln!("  <dim>not found (using defaults; run `strudel config edit` to create)</dim>");
        return Ok(());
    }
    let g = GlobalConfig::load()?;
    field("signing identity", opt(&g.signing_identity));
    field("signing team id", opt(&g.signing_team_id));
    field("notarize issuer", opt(&g.notarize_api_issuer));
    field("notarize api key", mask_opt(&g.notarize_api_key));
    field("notarize key path", opt_path(&g.notarize_api_key_path));
    Ok(())
}

fn apple_id_section() -> Result<()> {
    let data = StrudelData::locate()?;
    let dir = data.session_json.parent().unwrap_or(Path::new("~"));
    header("Apple ID session", dir);

    match read_session(&data.session_json) {
        Some(session) => {
            cprintln!("  <green>●</green> signed in");
            field("apple id", session.apple_id);
            field("dsid", mask(&session.dsid));
        },
        None => {
            cprintln!("  <dim>○ not signed in (run `strudel login` for free provisioning)</dim>");
        },
    }

    field(
        "dev cert apple id",
        opt(&apple_id_from_cert(&data.cert_der)),
    );
    field("dev cert", describe_cert(&data.cert_der));
    field("dev key", present(&data.key_pem));
    field("dev keychain", present(&data.keychain_db));
    Ok(())
}

fn project_section(config_path: &Path, target: Option<&str>) {
    let project = match load_config(config_path) {
        Ok(p) => p,
        Err(_) => {
            // No usable strudel.toml here; the login state above is still
            // meaningful, so this is not an error.
            header("Project", config_path);
            cprintln!("  <dim>no strudel.toml found in this directory</dim>");
            return;
        },
    };

    header("Project", config_path);
    for cfg in &project.targets {
        if let Some(name) = target
            && cfg.app_name != name
        {
            continue;
        }
        target_block(cfg);
    }
}

fn target_block(cfg: &ResolvedConfig) {
    let platform = match &cfg.target_platform {
        ResolvedTargetPlatform::Mac(_) => "macos",
        ResolvedTargetPlatform::Ios(_) => "ios",
    };
    cprintln!(
        "\n  <bold,cyan>-- {} ({}) --</bold,cyan>",
        cfg.app_name,
        platform
    );
    field2("bundle id", cfg.bundle_id.clone());

    let ResolvedTargetPlatform::Ios(ios) = &cfg.target_platform else {
        // macOS provisioning is signing-identity based; report what will sign.
        field2(
            "sign identity",
            if_empty(&cfg.sign_identity, "ad-hoc / none configured"),
        );
        return;
    };

    let backend = match ios.provisioning {
        IosProvisioningBackend::Free => "free (Apple ID, 7-day profiles)",
        IosProvisioningBackend::AppStoreConnect => "app_store_connect",
    };
    field2("provisioning", backend.to_string());
    field2("apple id", opt(&ios.apple_id));

    let paths = Paths::new(cfg);
    profile_lines(&paths, cfg);
    device_lines(&paths);
}

fn profile_lines(paths: &Paths, cfg: &ResolvedConfig) {
    // Prefer the explicitly pinned profile; otherwise the cached one.
    let (label, path) = match &cfg.provisioning_profile {
        Some(p) => ("profile (pinned)", p.clone()),
        None => ("profile (cached)", paths.cached_profile.clone()),
    };
    if !path.exists() {
        field2(label, format!("{} (none)", shorten(&path)));
        return;
    }
    field2(label, shorten(&path));
    match crate::builder::decode_profile(&path) {
        Ok(value) => print_profile_details(value.as_dictionary()),
        Err(e) => cprintln!("      <yellow>could not decode: {}</yellow>", e),
    }
}

fn print_profile_details(dict: Option<&plist::Dictionary>) {
    let Some(dict) = dict else {
        cprintln!("      <yellow>unexpected profile format</yellow>");
        return;
    };
    if let Some(name) = dict.get("Name").and_then(|v| v.as_string()) {
        subfield("name", name.to_string());
    }
    if let Some(exp) = dict.get("ExpirationDate").and_then(|v| v.as_date()) {
        subfield("expires", format_expiry(exp));
    }
    let devices = dict
        .get("ProvisionedDevices")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    subfield("devices", devices.to_string());
    if let Some(app_id) = dict
        .get("Entitlements")
        .and_then(|v| v.as_dictionary())
        .and_then(|d| d.get("application-identifier"))
        .and_then(|v| v.as_string())
    {
        subfield("app id", app_id.to_string());
    }
}

fn device_lines(paths: &Paths) {
    let set = match DeviceSet::load(&paths.devices_toml) {
        Ok(s) => s,
        Err(_) => return,
    };
    if set.device.is_empty() {
        field2(
            "tracked devices",
            "none (run `strudel device register`)".to_string(),
        );
        return;
    }
    field2(
        "tracked devices",
        format!("{} ({})", set.device.len(), shorten(&paths.devices_toml)),
    );
    for d in &set.device {
        subfield("-", format!("{} ({})", d.name, d.udid));
    }
}

fn header(title: &str, path: &Path) {
    cprintln!(
        "\n<bold,cyan>{}</bold,cyan>  <dim>{}</dim>",
        title,
        shorten(path)
    );
}

fn field(label: &str, value: String) {
    cprintln!("  {:<18} <dim>{}</dim>", format!("{label}:"), value);
}

fn field2(label: &str, value: String) {
    cprintln!("    {:<16} <dim>{}</dim>", format!("{label}:"), value);
}

fn subfield(label: &str, value: String) {
    cprintln!("      {:<14} <dim>{}</dim>", format!("{label}"), value);
}

fn read_session(path: &Path) -> Option<Session> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// Extract the Apple ID email from a cached dev certificate's subject. Apple
/// Development certificates carry it in the common name, e.g.
/// `CN=iPhone Developer: you@example.com (XXXXXXXXXX)`.
fn apple_id_from_cert(cert_der: &Path) -> Option<String> {
    if !cert_der.exists() {
        return None;
    }
    // `sep_multiline` prints one RDN per line, so a comma inside the org name
    // can't be mistaken for an RDN separator when we pick out the CN.
    let out = std::process::Command::new("openssl")
        .args(["x509", "-inform", "DER", "-in"])
        .arg(cert_der)
        .args(["-noout", "-subject", "-nameopt", "sep_multiline"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let cn = text
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("CN="))?;
    email_from_cn(cn)
}

/// Pull the Apple ID email out of a development certificate common name, e.g.
/// `iPhone Developer: you@example.com (XXXXXXXXXX)` -> `you@example.com`.
/// Returns `None` when the CN carries a display name rather than an email.
fn email_from_cn(cn: &str) -> Option<String> {
    let after_colon = cn.split_once(": ").map(|(_, v)| v).unwrap_or(cn);
    let candidate = after_colon.split(" (").next().unwrap_or(after_colon).trim();
    candidate.contains('@').then(|| candidate.to_string())
}

/// Describe the cached dev certificate: expiry + short SHA-1 fingerprint.
fn describe_cert(cert_der: &Path) -> String {
    if !cert_der.exists() {
        return "not cached".to_string();
    }
    let mut parts = vec!["cached".to_string()];
    // "notAfter=Jul  5 12:00:00 2027 GMT" -> keep the value.
    if let Some(end) = openssl_field(cert_der, "-enddate")
        && let Some(v) = end.split_once('=').map(|(_, v)| v.trim())
    {
        parts.push(format!("expires {v}"));
    }
    if let Some(fp) = openssl_field(cert_der, "-fingerprint")
        && let Some(v) = fp.split_once('=').map(|(_, v)| v.trim())
    {
        let hex = v.replace(':', "");
        parts.push(format!("sha1 {}", &hex[..hex.len().min(12)]));
    }
    parts.join(", ")
}

/// Run `openssl x509 -inform DER -in <cert> -noout <flag>` and return the
/// single trimmed output line, or `None` on any failure.
fn openssl_field(cert_der: &Path, flag: &str) -> Option<String> {
    let out = std::process::Command::new("openssl")
        .args(["x509", "-inform", "DER", "-in"])
        .arg(cert_der)
        .args(["-noout", flag])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn format_expiry(exp: plist::Date) -> String {
    use std::time::SystemTime;
    let exp = SystemTime::from(exp);
    match exp.duration_since(SystemTime::now()) {
        Ok(d) => {
            let days = d.as_secs() / 86_400;
            format!("in {days} day(s)")
        },
        Err(_) => "expired".to_string(),
    }
}

fn present(path: &Path) -> String {
    if path.exists() {
        format!("present ({})", shorten(path))
    } else {
        "absent".to_string()
    }
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "(unset)".to_string())
}

fn opt_path(v: &Option<PathBuf>) -> String {
    v.as_ref()
        .map(|p| shorten(p))
        .unwrap_or_else(|| "(unset)".to_string())
}

fn if_empty(v: &str, fallback: &str) -> String {
    if v.is_empty() {
        fallback.to_string()
    } else {
        v.to_string()
    }
}

/// Mask a secret, keeping only the last 4 characters.
fn mask(v: &str) -> String {
    match v.len() {
        0 => "(empty)".to_string(),
        n if n <= 4 => "****".to_string(),
        n => format!("****{}", &v[n - 4..]),
    }
}

fn mask_opt(v: &Option<String>) -> String {
    v.as_deref()
        .map(mask)
        .unwrap_or_else(|| "(unset)".to_string())
}

/// Shorten a path by replacing the home-directory prefix with `~`.
fn shorten(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = s.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_keeps_last_four() {
        assert_eq!(mask("ABCD1234"), "****1234");
        assert_eq!(mask("XY"), "****");
        assert_eq!(mask(""), "(empty)");
    }

    #[test]
    fn email_from_cn_extracts_apple_id() {
        assert_eq!(
            email_from_cn("iPhone Developer: you@example.com (XXXXXXXXXX)").as_deref(),
            Some("you@example.com")
        );
        assert_eq!(
            email_from_cn("Apple Development: me@apple.com (ABCD123456)").as_deref(),
            Some("me@apple.com")
        );
        // Display-name CNs (paid teams) carry no email -> None.
        assert_eq!(
            email_from_cn("Apple Development: Jane Doe (ABCD123456)"),
            None
        );
    }

    #[test]
    fn shorten_replaces_home() {
        // SAFETY: single-threaded test; sets HOME for the shorten() lookup.
        unsafe { std::env::set_var("HOME", "/Users/tester") };
        assert_eq!(
            shorten(Path::new("/Users/tester/.config/strudel/config.toml")),
            "~/.config/strudel/config.toml"
        );
        assert_eq!(shorten(Path::new("/etc/hosts")), "/etc/hosts");
    }
}

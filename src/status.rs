//! `strudel status` prints the environment, global config, the Apple ID
//! session stored under `~/.local/share/strudel/`, and the per-project
//! `.strudel/` provisioning artifacts. `strudel login status` prints just the
//! Apple ID session block.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use appleid::Session;
use clml::cprintln;

use crate::apple::fingerprint::parse_fingerprint;
use crate::builder;
use crate::config::{
    GlobalConfig, IosProvisioningBackend, Platform, ResolvedConfig, ResolvedIosSection,
    ResolvedTargetPlatform, load_config,
};
use crate::devices::DeviceSet;
use crate::paths::{Paths, StrudelData};

/// `strudel status`: print the environment, global config, Apple ID session,
/// and per-project provisioning state. `config_path`/`target` mirror the other
/// commands; the project section is best-effort and omitted when no config
/// loads.
pub fn run(config_path: &Path, target: Option<&str>) -> Result<()> {
    environment_section();
    println!();
    global_config_section()?;
    println!();
    let session = apple_id_section()?;
    println!();
    project_section(config_path, target, session.as_ref());
    Ok(())
}

/// `strudel login status`: print just the Apple ID session block (no global
/// config or project state).
pub fn login_status() -> Result<()> {
    apple_id_section()?;
    Ok(())
}

/// Best-effort local toolchain versions; useful context when diagnosing a
/// failed build without requiring the caller to run `swift --version` etc.
/// themselves.
fn environment_section() {
    header("Environment", None);
    field("strudel version", env!("CARGO_PKG_VERSION").to_string());
    field("swift", tool_version("swift", &["--version"]));
    field("xcodebuild", tool_version("xcodebuild", &["-version"]));
}

fn tool_version(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::to_string))
        .unwrap_or_else(|| "not found".to_string())
}

/// `strudel profile` (no subcommand): print the current provisioning-profile
/// status for each selected target, without fetching or refreshing anything.
pub fn profile_info(config_path: &Path, target: Option<&str>) -> Result<()> {
    let project = load_config(config_path)?;
    let session = StrudelData::locate()
        .ok()
        .and_then(|d| read_session(&d.session_json));

    let targets = match target {
        Some(selector) => vec![project.resolve_target(selector)?],
        None => project.targets.iter().collect(),
    };

    for cfg in targets {
        cprintln!(
            "<bold,cyan>{}</bold,cyan>  <dim>{}</dim>",
            cfg.target_id,
            cfg.bundle_id
        );
        match &cfg.target_platform {
            ResolvedTargetPlatform::Ios(ios) => ios_provisioning_block(cfg, ios, session.as_ref()),
            ResolvedTargetPlatform::Mac(_) => macos_profile_block(cfg),
        }
    }
    Ok(())
}

/// `strudel devices` (no subcommand): list devices tracked in
/// `.strudel/devices.toml` for each selected iOS target.
pub fn devices_list(config_path: &Path, target: Option<&str>) -> Result<()> {
    let project = load_config(config_path)?;
    let targets = project.select(target, Platform::Ios, true)?;
    let multi = targets.len() > 1;

    for cfg in targets {
        if multi {
            cprintln!("<bold,cyan>{}</bold,cyan>", cfg.target_id);
        }
        let paths = Paths::new(cfg);
        let set = DeviceSet::load(&paths.devices_toml)?;
        if set.device.is_empty() {
            cprintln!("  <dim>No tracked devices. Run `strudel devices add`.</dim>");
            continue;
        }
        for d in &set.device {
            cprintln!("  {}  <dim>{}</dim>", d.name, d.udid);
        }
    }
    Ok(())
}

fn global_config_section() -> Result<()> {
    let path = GlobalConfig::xdg_path()?;
    header("Global config", Some(&path));
    if !path.exists() {
        cprintln!(
            "  <dim>not found (using defaults; run `strudel config global edit` to create)</dim>"
        );
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

fn apple_id_section() -> Result<Option<Session>> {
    let data = StrudelData::locate()?;
    let dir = data.session_json.parent().unwrap_or(Path::new("~"));
    header("Apple ID session", Some(dir));

    let session = read_session(&data.session_json);
    match &session {
        Some(session) => {
            cprintln!("  <green>●</green> signed in");
            field("apple id", session.apple_id.clone());
            field("dsid", mask(&session.dsid));
            account_lines(session);
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
    Ok(session)
}

/// Fetch and print developer teams + their registered devices from Apple's
/// developer-services portal. Best-effort: any network/auth failure is
/// reported inline rather than failing the whole `status` command, since
/// everything else it prints is purely local.
fn account_lines(session: &Session) {
    let details = (|| -> anyhow::Result<Vec<(appleid::Team, Vec<appleid::Device>)>> {
        let apple_id = appleid::AppleId::new()?;
        let teams = apple_id.list_teams(session)?;
        teams
            .into_iter()
            .map(|t| {
                let devices = apple_id.list_devices(session, &t.id)?;
                Ok((t, devices))
            })
            .collect()
    })();

    match details {
        Ok(teams) if teams.is_empty() => {
            field2("developer teams", "none found on this account".to_string());
        },
        Ok(teams) => {
            for (team, devices) in teams {
                field2(
                    "team",
                    format!("{} ({}) [{}]", team.name, team.id, team.status),
                );
                if devices.is_empty() {
                    subfield("devices", "none registered on the portal".to_string());
                    continue;
                }
                subfield("devices", devices.len().to_string());
                for d in devices {
                    subfield(
                        "-",
                        format!("{} ({}) - {} {}", d.name, d.udid, d.model, d.platform),
                    );
                }
            }
        },
        Err(e) => {
            cprintln!("  <yellow>could not reach Apple: {}</yellow>", e);
        },
    }
}

fn project_section(config_path: &Path, target: Option<&str>, session: Option<&Session>) {
    let project = match load_config(config_path) {
        Ok(p) => p,
        Err(_) => {
            // No usable strudel.toml here; the login state above is still
            // meaningful, so this is not an error.
            header("Project", Some(config_path));
            cprintln!("  <dim>no strudel.toml found in this directory</dim>");
            return;
        },
    };

    header("Project", Some(config_path));
    for cfg in &project.targets {
        if let Some(selector) = target
            && !cfg.target_id.contains(selector)
        {
            continue;
        }
        target_block(cfg, session);
    }
}

fn target_block(cfg: &ResolvedConfig, session: Option<&Session>) {
    let platform = match &cfg.target_platform {
        ResolvedTargetPlatform::Mac(_) => "macos",
        ResolvedTargetPlatform::Ios(_) => "ios",
    };
    cprintln!(
        "  target: <bold,cyan>{}</bold,cyan> <dim>{}</dim>",
        cfg.target_id,
        cfg.bundle_id
    );

    field2("platform", platform.to_string());
    match &cfg.target_platform {
        ResolvedTargetPlatform::Mac(_) => {
            field2(
                "sign identity",
                if_empty(&cfg.sign_identity, "ad-hoc / none configured"),
            );
            macos_profile_block(cfg);
        },
        ResolvedTargetPlatform::Ios(ios) => {
            ios_provisioning_block(cfg, ios, session);
        },
    }
}

/// Print the manually-pinned provisioning profile for a macOS target, if any.
/// Unlike iOS, macOS has no auto-fetch backend: a profile is only needed for
/// certain entitlements, and must be supplied via `build.provisioning_profile`
/// in strudel.toml.
fn macos_profile_block(cfg: &ResolvedConfig) {
    let Some(path) = &cfg.provisioning_profile else {
        field2(
            "provisioning profile",
            "not configured (only required for some entitlements)".to_string(),
        );
        return;
    };
    if !path.exists() {
        field2(
            "provisioning profile",
            format!("{} (missing)", shorten(path)),
        );
        return;
    }
    field2("provisioning profile", shorten(path));
    match builder::decode_profile(path) {
        Ok(value) => print_profile_details(value.as_dictionary(), None),
        Err(e) => cprintln!("      <yellow>could not decode: {}</yellow>", e),
    }
}

/// Print provisioning backend, cached profile, and tracked-device info for
/// one iOS target. Shared by `strudel login status` (as part of the full
/// project dump) and `strudel profile` (on its own), so the two commands
/// don't drift out of sync.
fn ios_provisioning_block(
    cfg: &ResolvedConfig,
    ios: &ResolvedIosSection,
    session: Option<&Session>,
) {
    let backend = match ios.provisioning {
        IosProvisioningBackend::Free => "free (Apple ID, 7-day profiles)",
        IosProvisioningBackend::AppStoreConnect => "app_store_connect",
    };
    field2("provisioning", backend.to_string());
    field2("apple id", opt(&ios.apple_id));

    // Free profiles are tied to whichever Apple ID requested them; only that
    // check makes sense for the free backend, and only when signed in.
    let expected_owner = matches!(ios.provisioning, IosProvisioningBackend::Free)
        .then_some(session)
        .flatten()
        .map(|s| s.apple_id.as_str());

    let paths = Paths::new(cfg);
    profile_lines(&paths, cfg, expected_owner);
    device_lines(&paths);
}

fn profile_lines(paths: &Paths, cfg: &ResolvedConfig, expected_owner: Option<&str>) {
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
    match builder::decode_profile(&path) {
        Ok(value) => print_profile_details(value.as_dictionary(), expected_owner),
        Err(e) => cprintln!("      <yellow>could not decode: {}</yellow>", e),
    }
}

fn print_profile_details(dict: Option<&plist::Dictionary>, expected_owner: Option<&str>) {
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
    if let Some(expected) = expected_owner {
        print_owner_check(dict, expected);
    }
}

/// For free provisioning, verify the profile's embedded developer
/// certificate(s) belong to the currently signed-in Apple ID. A stale profile
/// from a previously signed-in account still decodes and looks "current"
/// otherwise, but Apple will reject it (or the wrong identity) at install
/// time, so surface the mismatch here instead.
fn print_owner_check(dict: &plist::Dictionary, expected: &str) {
    let certs = dict.get("DeveloperCertificates").and_then(|v| v.as_array());
    let Some(certs) = certs else {
        subfield(
            "owner",
            "unknown (profile has no embedded certificate)".to_string(),
        );
        return;
    };

    let emails: Vec<String> = certs
        .iter()
        .filter_map(|v| v.as_data())
        .filter_map(email_from_der_bytes)
        .collect();

    if emails.iter().any(|e| e.eq_ignore_ascii_case(expected)) {
        subfield("owner", emails.join(", "));
    } else if emails.is_empty() {
        subfield(
            "owner",
            "unknown (could not read embedded certificate)".to_string(),
        );
    } else {
        cprintln!(
            "      <yellow>owner mismatch: profile belongs to {}, signed in as {}</yellow>",
            emails.join(", "),
            expected
        );
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
            "none (run `strudel devices add`)".to_string(),
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

fn header(title: &str, path: Option<&Path>) {
    match path {
        Some(p) => cprintln!(
            "<bold,cyan>{}</bold,cyan>  <dim>{}</dim>",
            title,
            shorten(p)
        ),
        None => cprintln!("<bold,cyan>{}</bold,cyan>", title),
    }
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
    email_from_der_bytes(&std::fs::read(cert_der).ok()?)
}

/// Extract the Apple ID email from a DER-encoded certificate's subject CN,
/// e.g. `CN=iPhone Developer: you@example.com (XXXXXXXXXX)`.
fn email_from_der_bytes(cert_der: &[u8]) -> Option<String> {
    // `sep_multiline` prints one RDN per line, so a comma inside the org name
    // can't be mistaken for an RDN separator when we pick out the CN.
    let mut child = std::process::Command::new("openssl")
        .args([
            "x509",
            "-inform",
            "DER",
            "-noout",
            "-subject",
            "-nameopt",
            "sep_multiline",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(cert_der).ok()?;
    let out = child.wait_with_output().ok()?;
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
        && let Some(hex) = parse_fingerprint(&fp)
    {
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
    shorten_under(path, std::env::var_os("HOME"))
}

/// [`shorten`] with `$HOME` passed in rather than read from the environment,
/// so tests do not have to mutate a process-global other threads are reading.
fn shorten_under(path: &Path, home: Option<OsString>) -> String {
    let s = path.to_string_lossy().to_string();
    if let Some(home) = home {
        let home = home.to_string_lossy();
        // Only on a directory boundary: `/Users/tester2` is not under
        // `/Users/tester`, and must not render as `~2`.
        if let Some(rest) = s.strip_prefix(home.as_ref())
            && (rest.is_empty() || rest.starts_with('/'))
        {
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

    fn home() -> Option<OsString> {
        Some(OsString::from("/Users/tester"))
    }

    #[test]
    fn shorten_replaces_home() {
        assert_eq!(
            shorten_under(
                Path::new("/Users/tester/.config/strudel/config.toml"),
                home()
            ),
            "~/.config/strudel/config.toml"
        );
    }

    #[test]
    fn shorten_replaces_a_bare_home_dir() {
        assert_eq!(shorten_under(Path::new("/Users/tester"), home()), "~");
    }

    #[test]
    fn shorten_leaves_paths_outside_home_alone() {
        assert_eq!(shorten_under(Path::new("/etc/hosts"), home()), "/etc/hosts");
        // A sibling whose name merely starts with $HOME's is not under $HOME:
        // the prefix has to land on a directory boundary, not any character.
        assert_eq!(
            shorten_under(Path::new("/Users/tester2/x"), home()),
            "/Users/tester2/x"
        );
    }

    #[test]
    fn shorten_without_home_returns_the_path_unchanged() {
        assert_eq!(shorten_under(Path::new("/etc/hosts"), None), "/etc/hosts");
    }
}

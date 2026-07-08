use anyhow::{Context, Result, bail};
use appleid::{AppleId, Session, Team};
use color_print::cprintln;

use crate::builder::keychain as kc;
use crate::config::ResolvedConfig;
use crate::devices::DeviceSet;
use crate::paths::{Paths, StrudelData, ensure_strudel_dir};

/// Interactive sign-in. Prompts for Apple ID + password + 2FA and persists
/// the resulting session under `~/.local/share/strudel/session.json`.
pub fn login(apple_id_email: Option<String>) -> Result<()> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id(&data)?;

    let email = match apple_id_email {
        Some(e) => e.to_string(),
        None => inquire::Text::new("Apple ID:")
            .with_help_message("Enter your Apple ID email address")
            .prompt()
            .context("reading Apple ID")?,
    };

    let password = inquire::Password::new("Password:")
        .without_confirmation()
        .prompt()
        .context("reading password")?;

    cprintln!("Signing in...");
    let session = apple_id
        .login(&email, &password, || {
            inquire::Text::new("Two-factor authentication code:")
                .prompt()
                .context("reading 2FA code")
        })
        .context("Apple ID sign-in failed")?;

    save_session(&data, &session)?;
    cprintln!("<green>✔</green> Signed in. Session saved.");
    cprintln!(
        "<dim>Free provisioning: 7-day profiles, max 3 devices, max 10 App IDs.\n\
         For unlimited, use a paid account + App Store Connect.</dim>"
    );
    nudge_device_registration(&apple_id, &session);
    Ok(())
}

/// After a successful sign-in, check whether the account's team has any
/// devices registered on the Apple developer portal, and if not, point at
/// `strudel devices add`. Best-effort: a network/auth error here
/// shouldn't fail `login` itself, since sign-in already succeeded.
fn nudge_device_registration(apple_id: &AppleId, session: &Session) {
    let result = pick_team(apple_id, session).and_then(|team| {
        apple_id
            .list_devices(session, &team.id)
            .with_context(|| format!("listing devices for team {}", team.name))
    });
    match result {
        Ok(devices) if devices.is_empty() => {
            cprintln!(
                "<dim>No devices registered on this account yet. Run \
                 `strudel devices add` to register your device(s).</dim>"
            );
        },
        Ok(_) => {},
        Err(e) => {
            cprintln!("<yellow>Could not check registered devices: {e}</yellow>");
        },
    }
}

/// Clear the persisted session and cached credentials.
pub fn logout() -> Result<()> {
    let data = StrudelData::locate()?;
    let targets: Vec<&std::path::Path> = [&data.session_json, &data.cert_der, &data.key_pem]
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.as_path())
        .collect();

    if targets.is_empty() {
        cprintln!("<dim>Nothing to clear; no session or cached credentials found.</dim>");
        return Ok(());
    }

    cprintln!("This will remove:");
    for path in &targets {
        cprintln!("  <dim>{}</dim>", path.display());
    }
    let confirmed = inquire::Confirm::new("Continue?")
        .with_default(false)
        .prompt()
        .context("reading confirmation")?;
    if !confirmed {
        cprintln!("<dim>Aborted; nothing was cleared.</dim>");
        return Ok(());
    }

    for path in &targets {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    cprintln!("<green>✔</green> Logged out. Session and cached credentials cleared.");
    Ok(())
}

/// Register a device on the Apple developer portal via Apple ID.
pub fn register_device(_cfg: &ResolvedConfig, name: &str, udid: &str) -> Result<()> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id(&data)?;
    let session = load_session_or_err(&data)?;
    let team = pick_team(&apple_id, &session)?;
    apple_id
        .add_device(&session, &team.id, name, udid)
        .with_context(|| format!("Failed to register device {name} ({udid})"))?;
    Ok(())
}

/// Fetch a 7-day development profile via Apple ID, cache it at
/// `paths.cached_profile`, and import the returned cert+key into the persistent
/// dev keychain so codesign can use the identity.
pub fn auto_fetch_profile(cfg: &ResolvedConfig, paths: &Paths) -> Result<()> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id(&data)?;
    let session = load_session_or_err(&data)?;
    let team = pick_team(&apple_id, &session)?;

    let device_set = DeviceSet::load(&paths.devices_toml)?;
    if device_set.device.is_empty() {
        bail!(
            "No devices tracked in .strudel/devices.toml.\n\
             Run `strudel devices add` to register your device(s)."
        );
    }
    let udids: Vec<&str> = device_set.device.iter().map(|d| d.udid.as_str()).collect();

    // Reuse a previously issued cert+key if it's still valid, so refreshing the
    // 7-day profile doesn't revoke and reissue the (year-long) cert each time.
    let cached_cert = std::fs::read(&data.cert_der).ok();
    let cached_key = std::fs::read(&data.key_pem).ok();
    let cached_identity = match (&cached_cert, &cached_key) {
        (Some(c), Some(k)) => Some((c.as_slice(), k.as_slice())),
        _ => None,
    };

    cprintln!("Fetching 7-day development profile via Apple ID...");
    let profile = apple_id
        .fetch_development_profile(
            &session,
            &team.id,
            &cfg.bundle_id,
            &udids,
            cached_identity,
            |certs| {
                cprintln!("<yellow>This account already has a development certificate:</yellow>");
                for c in certs {
                    cprintln!("  - {}", c);
                }
                inquire::Confirm::new("Revoke it to issue a new one?")
                    .with_default(false)
                    .with_help_message(
                        "Free accounts allow only one development certificate. \
                         Revoking may break Xcode's signing until it creates a new one.",
                    )
                    .prompt()
                    .context("reading revoke confirmation")
            },
        )
        .with_context(|| {
            format!(
                "Failed to fetch development profile for team \"{}\" ({})",
                team.name, team.id
            )
        })?;

    ensure_strudel_dir(&paths.strudel_dir)?;
    std::fs::write(&paths.cached_profile, &profile.mobileprovision)
        .context("writing provisioning profile")?;
    cprintln!(
        "<green>✔</green> Profile cached at {}",
        paths.cached_profile.display()
    );

    std::fs::write(&data.cert_der, &profile.cert_der).context("writing cached dev cert")?;
    std::fs::write(&data.key_pem, &profile.key_pem).context("writing cached dev key")?;

    kc::dev::import_dev_cert(&profile.cert_der, &profile.key_pem, &data.keychain_db)
        .context("importing dev cert into persistent keychain")?;
    kc::dev::ensure_keychain_in_search_list(&data.keychain_db)?;
    Ok(())
}

/// Ensure the persistent dev keychain is in the search list (and re-import the
/// cert if the cache exists). Called before codesign when the Free backend is
/// active and the profile is already current (so `auto_fetch_profile` was
/// skipped).
pub fn ensure_keychain_ready() -> Result<()> {
    let data = StrudelData::locate()?;
    if data.cert_der.exists() && data.key_pem.exists() {
        let cert = std::fs::read(&data.cert_der).context("reading cached cert")?;
        let key = std::fs::read(&data.key_pem).context("reading cached key")?;
        kc::dev::import_dev_cert(&cert, &key, &data.keychain_db)
            .context("re-importing cached dev cert")?;
    }
    kc::dev::ensure_keychain_in_search_list(&data.keychain_db)
}

/// SHA-1 fingerprint (uppercase hex, no separators) of the cached
/// free-provisioning development certificate, or `None` if it isn't cached.
///
/// Used to sign with that exact certificate rather than the ambiguous "Apple
/// Development" name: revoked/older "Apple Development" certs may still sit in
/// the login keychain, and signing by name would pick the wrong one (and
/// codesign would refuse an ambiguous match).
pub fn dev_cert_sha1() -> Result<Option<String>> {
    let data = StrudelData::locate()?;
    if !data.cert_der.exists() {
        return Ok(None);
    }
    let out = std::process::Command::new("openssl")
        .args([
            "x509",
            "-inform",
            "DER",
            "-in",
            data.cert_der.to_str().unwrap(),
            "-noout",
            "-fingerprint",
            "-sha1",
        ])
        .output()
        .context("computing dev cert fingerprint")?;
    if !out.status.success() {
        bail!(
            "openssl x509 fingerprint failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // Output: "SHA1 Fingerprint=AA:BB:CC:..."
    let stdout = String::from_utf8_lossy(&out.stdout);
    let fp = stdout
        .split('=')
        .nth(1)
        .map(|s| s.trim().replace(':', "").to_ascii_uppercase())
        .filter(|s| !s.is_empty());
    Ok(fp)
}

fn get_apple_id(_data: &StrudelData) -> Result<AppleId> {
    AppleId::new().context("initializing Apple ID client")
}

fn pick_team(apple_id: &AppleId, session: &Session) -> Result<Team> {
    let mut teams = apple_id
        .list_teams(session)
        .context("listing Apple Developer teams")?;
    if teams.is_empty() {
        bail!("No Apple Developer teams found for this Apple ID.");
    }
    if let Some(pos) = teams.iter().position(|t| t.name.contains("Personal Team")) {
        return Ok(teams.remove(pos));
    }
    Ok(teams.remove(0))
}

fn load_session_or_err(data: &StrudelData) -> Result<Session> {
    if !data.session_json.exists() {
        bail!(
            "Not signed in. Run `strudel login` to sign in with your Apple ID.\n\
             Or set [ios] provisioning = \"app_store_connect\" to use App Store Connect."
        );
    }
    let s = std::fs::read_to_string(&data.session_json).context("reading session file")?;
    serde_json::from_str(&s)
        .context("parsing session file (try `strudel login` to refresh the session)")
}

fn save_session(data: &StrudelData, session: &Session) -> Result<()> {
    let s = serde_json::to_string_pretty(session).context("serializing session")?;
    std::fs::write(&data.session_json, s).context("saving session")
}

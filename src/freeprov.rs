use anyhow::{Context, Result, bail};
use color_print::cprintln;

use appleid::{AppleId, Session, Team};

use crate::builder::keychain as kc;
use crate::config::ResolvedConfig;
use crate::devices::DeviceSet;
use crate::paths::{Paths, StrudelData, ensure_strudel_dir};

/// Interactive sign-in. Prompts for Apple ID + password + 2FA and persists
/// the resulting session under `~/.local/share/strudel/session.json`.
pub fn login(apple_id_hint: Option<&str>) -> Result<()> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id(&data)?;

    let email = match apple_id_hint {
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
    Ok(())
}

/// Clear the persisted session and cached credentials.
pub fn logout() -> Result<()> {
    let data = StrudelData::locate()?;
    if data.session_json.exists() {
        std::fs::remove_file(&data.session_json).context("removing session file")?;
    }
    if data.cert_der.exists() {
        std::fs::remove_file(&data.cert_der).ok();
    }
    if data.key_pem.exists() {
        std::fs::remove_file(&data.key_pem).ok();
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
             Run `strudel device register` to register your device(s)."
        );
    }
    let udids: Vec<&str> = device_set.device.iter().map(|d| d.udid.as_str()).collect();

    cprintln!("Fetching 7-day development profile via Apple ID...");
    let profile = apple_id
        .fetch_development_profile(&session, &team.id, &cfg.bundle_id, &udids)
        .context("Failed to fetch development profile")?;

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

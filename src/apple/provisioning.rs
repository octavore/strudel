use std::path::Path;

use anyhow::{Context, Result, bail};
use appleid::{AppleId, Session, Team};
use clml::cprintln;

use crate::apple::accounts::{self, Account};
use crate::apple::capabilities::{CapabilityDef, Known, Severity};
use crate::apple::fingerprint::parse_fingerprint;
use crate::builder::keychain as kc;
use crate::config::{Platform, ResolvedConfig};
use crate::devices::{DeviceSet, PortalEntry};
use crate::paths::{Paths, StrudelData, ensure_strudel_dir};

/// Interactive sign-in. Prompts for Apple ID + password + 2FA and persists
/// the resulting session under
/// `~/.local/share/strudel/accounts/<dsid>/session.json`.
pub fn login(apple_id_email: Option<String>) -> Result<()> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id()?;

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

    let account = Account::create(&data, &session)?;
    account.save_session(&session)?;
    cprintln!(
        "<green>✔</green> Signed in as {}. Session saved.",
        account.email
    );
    match account.refresh_teams(&apple_id) {
        Ok(teams) => {
            for team in &teams {
                cprintln!("<dim>  team {} ({})</dim>", team.name, team.id);
            }
        },
        Err(e) => cprintln!("<yellow>Could not list teams: {e:#}</yellow>"),
    }
    cprintln!(
        "<dim>Free provisioning: 7-day profiles, max 3 devices, max 10 App IDs.\n\
         For unlimited, use a paid account + App Store Connect.\n\
         Paid teams use the session only for `strudel icloud-containers`.</dim>"
    );
    nudge_device_registration(&apple_id, &session);
    Ok(())
}

/// After a successful sign-in, check whether the account's team has any
/// devices registered on the Apple developer portal, and if not, point at
/// `strudel devices add`. Best-effort: a network/auth error here
/// shouldn't fail `login` itself, since sign-in already succeeded.
fn nudge_device_registration(apple_id: &AppleId, session: &Session) {
    let result = pick_team(apple_id, session, "").and_then(|team| {
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

/// Clear the session and cached credentials of one account. A picker chooses
/// the account if several are signed in.
pub fn logout() -> Result<()> {
    let data = StrudelData::locate()?;
    let mut signed_in = accounts::list(&data)?;

    let account = match signed_in.len() {
        0 => {
            cprintln!("<dim>Nothing to clear; no session or cached credentials found.</dim>");
            return Ok(());
        },
        1 => signed_in.remove(0),
        _ => {
            let emails: Vec<String> = signed_in.iter().map(|a| a.email.clone()).collect();
            let choice = inquire::Select::new("Sign out which Apple ID?", emails)
                .raw_prompt()
                .context("reading Apple ID choice")?;
            signed_in.remove(choice.index)
        },
    };

    cprintln!("This will remove {}:", account.email);
    for path in account.files() {
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

    account.remove()?;
    cprintln!(
        "<green>✔</green> Logged out {}. Session and cached credentials cleared.",
        account.email
    );
    Ok(())
}

/// Apple ID client, account, session, and team of a free-provisioning flow.
struct FreeSession {
    apple_id: AppleId,
    account: Account,
    session: Session,
    team: Team,
}

/// Sign-in state for `team_id`. An empty `team_id` uses the only signed-in
/// account and its personal team.
fn free_session(team_id: &str) -> Result<FreeSession> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id()?;
    let account = accounts::for_config(&data, &apple_id, team_id)?;
    let session = account.load_session()?;
    let team = pick_team(&apple_id, &session, team_id)?;
    Ok(FreeSession {
        apple_id,
        account,
        session,
        team,
    })
}

/// Account whose dev certificate signs free-provisioned builds of `team_id`.
pub fn free_account(team_id: &str) -> Result<Account> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id()?;
    accounts::for_config(&data, &apple_id, team_id)
}

/// Register a device on the Apple developer portal via Apple ID.
pub fn register_device(cfg: &ResolvedConfig, name: &str, udid: &str) -> Result<()> {
    let FreeSession {
        apple_id,
        session,
        team,
        ..
    } = free_session(&cfg.team_id)?;
    apple_id
        .add_device(&session, &team.id, name, udid)
        .with_context(|| format!("Failed to register device {name} ({udid})"))?;
    Ok(())
}

/// List the devices registered on the team of the config's Apple ID.
pub fn registered_devices(cfg: &ResolvedConfig) -> Result<Vec<PortalEntry>> {
    let FreeSession {
        apple_id,
        session,
        team,
        ..
    } = free_session(&cfg.team_id)?;
    let devices = apple_id
        .list_devices(&session, &team.id)
        .with_context(|| format!("listing devices for team {}", team.name))?;
    Ok(devices
        .into_iter()
        .map(|d| PortalEntry {
            name: d.name,
            udid: d.udid,
        })
        .collect())
}

/// Bundle ID that a free development profile is fetched for. This is the host
/// app or one extension.
pub struct FreeProfileTarget<'a> {
    pub bundle_id: &'a str,
    /// App or extension name, used as the App ID name.
    pub label: &'a str,
    /// Where the fetched profile is cached.
    pub cache_path: &'a Path,
    /// Configured team ID. Empty if unset.
    pub team_id: &'a str,
}

/// Fetch a 7-day development profile via Apple ID, cache it at
/// `target.cache_path`, and import the returned cert+key into the persistent
/// dev keychain so codesign can use the identity. `caps` are enabled on the
/// App ID first where free provisioning supports them.
pub fn auto_fetch_profile(
    target: &FreeProfileTarget,
    caps: &[&'static CapabilityDef],
    paths: &Paths,
) -> Result<()> {
    let data = StrudelData::locate()?;
    let FreeSession {
        apple_id,
        account,
        session,
        team,
    } = free_session(target.team_id)?;

    let device_set = DeviceSet::load_for(&paths.devices_toml, Platform::Ios)?;
    if device_set.device.is_empty() {
        bail!(
            "No devices tracked in .strudel/devices.toml.\n\
             Run `strudel devices add` to register your device(s)."
        );
    }

    ensure_capabilities(&apple_id, &session, &team, target, caps)?;
    let udids: Vec<&str> = device_set.device.iter().map(|d| d.udid.as_str()).collect();

    // Reuse a previously issued cert+key if it's still valid, so refreshing the
    // 7-day profile doesn't revoke and reissue the (year-long) cert each time.
    let cached_cert = std::fs::read(account.cert_der()).ok();
    let cached_key = std::fs::read(account.key_pem()).ok();
    let cached_identity = match (&cached_cert, &cached_key) {
        (Some(c), Some(k)) => Some((c.as_slice(), k.as_slice())),
        _ => None,
    };

    cprintln!("Fetching 7-day development profile via Apple ID...");
    let profile = apple_id
        .fetch_development_profile(
            &session,
            &team.id,
            target.bundle_id,
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
    std::fs::write(target.cache_path, &profile.mobileprovision)
        .context("writing provisioning profile")?;
    cprintln!(
        "<green>✔</green> Profile cached at {}",
        target.cache_path.display()
    );

    std::fs::write(account.cert_der(), &profile.cert_der).context("writing cached dev cert")?;
    std::fs::write(account.key_pem(), &profile.key_pem).context("writing cached dev key")?;

    kc::dev::import_dev_cert(&profile.cert_der, &profile.key_pem, &data.keychain_db)
        .context("importing dev cert into persistent keychain")?;
    kc::dev::ensure_keychain_in_search_list(&data.keychain_db)?;
    Ok(())
}

/// Ensure the persistent dev keychain is in the search list (and re-import the
/// cert of `account` if the cache exists). Called before codesign when the
/// Free backend is active and the profile is already current (so
/// `auto_fetch_profile` was skipped).
pub fn ensure_keychain_ready(account: &Account) -> Result<()> {
    let data = StrudelData::locate()?;
    if account.cert_der().exists() && account.key_pem().exists() {
        let cert = std::fs::read(account.cert_der()).context("reading cached cert")?;
        let key = std::fs::read(account.key_pem()).context("reading cached key")?;
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
pub fn dev_cert_sha1(account: &Account) -> Result<Option<String>> {
    let cert_der = account.cert_der();
    if !cert_der.exists() {
        return Ok(None);
    }
    let out = std::process::Command::new("openssl")
        .args([
            "x509",
            "-inform",
            "DER",
            "-in",
            cert_der.to_str().unwrap(),
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(parse_fingerprint(&stdout))
}

/// Enable required capabilities that free accounts can use.
///
/// See [`CapabilityDef::free_feature`]. Capabilities in `caps` that free
/// provisioning cannot enable produce a warning. A failed update is an error
/// for `Verified` features and a warning for the rest.
fn ensure_capabilities(
    apple_id: &AppleId,
    session: &Session,
    team: &Team,
    target: &FreeProfileTarget,
    caps: &[&'static CapabilityDef],
) -> Result<()> {
    let plan = FreePlan::new(caps);

    if !plan.unsupported.is_empty() {
        let names: Vec<&str> = plan.unsupported.iter().map(|c| c.name).collect();
        cprintln!(
            "<yellow>warning:</yellow> These capabilities cannot be enabled under free \
             provisioning and need a paid Apple Developer Program account: {}",
            names.join(", ")
        );
    }

    for (severity, caps) in [
        (Severity::Error, &plan.verified),
        (Severity::Warning, &plan.unverified),
    ] {
        if caps.is_empty() {
            continue;
        }
        cprintln!("<dim>Enabling capabilities on the App ID (experimental)...</dim>");
        let result = apple_id.ensure_capabilities(
            session,
            &team.id,
            target.bundle_id,
            target.label,
            &feature_pairs(caps),
        );
        report_enable_result(severity, caps, target.bundle_id, result)?;
    }

    Ok(())
}

/// Required capabilities grouped by how free provisioning handles them.
struct FreePlan {
    /// Free feature is `Verified`. A failed update is an error.
    verified: Vec<&'static CapabilityDef>,
    /// Free feature is `Unverified`. A failed update is a warning.
    unverified: Vec<&'static CapabilityDef>,
    /// No known free feature.
    unsupported: Vec<&'static CapabilityDef>,
}

impl FreePlan {
    fn new(required: &[&'static CapabilityDef]) -> Self {
        let mut plan = FreePlan {
            verified: Vec::new(),
            unverified: Vec::new(),
            unsupported: Vec::new(),
        };
        for &cap in required {
            match (&cap.free_feature, cap.free_feature.severity()) {
                (Known::Unknown, _) => plan.unsupported.push(cap),
                (_, Severity::Error) => plan.verified.push(cap),
                (_, Severity::Warning) => plan.unverified.push(cap),
            }
        }
        plan
    }
}

/// Feature code and value of each capability that has a free feature.
fn feature_pairs(caps: &[&'static CapabilityDef]) -> Vec<(&'static str, &'static str)> {
    caps.iter()
        .filter_map(|c| c.free_feature.value().map(|f| (f.code, f.value)))
        .collect()
}

/// Report the outcome of an `updateAppId` call. A failure is an error if
/// `severity` is `Error` and a warning otherwise.
fn report_enable_result(
    severity: Severity,
    caps: &[&'static CapabilityDef],
    bundle_id: &str,
    result: Result<bool>,
) -> Result<()> {
    match result {
        Ok(true) => {
            let names: Vec<&str> = caps.iter().map(|c| c.name).collect();
            cprintln!("  Enabled {}", names.join(", "));
            Ok(())
        },
        Ok(false) => Ok(()),
        Err(err) => {
            let err = err.context(format!(
                "Failed to enable capabilities on App ID {bundle_id}"
            ));
            match severity {
                Severity::Error => Err(err),
                Severity::Warning => {
                    cprintln!(
                        "<yellow>warning:</yellow> {err:#}. Enable the capabilities by hand if \
                         the app needs them."
                    );
                    Ok(())
                },
            }
        },
    }
}

pub(crate) fn get_apple_id() -> Result<AppleId> {
    AppleId::new().context("initializing Apple ID client")
}

/// Team `team_id` of the session. An empty `team_id` picks the personal team,
/// or the first team.
fn pick_team(apple_id: &AppleId, session: &Session, team_id: &str) -> Result<Team> {
    let mut teams = apple_id
        .list_teams(session)
        .context("listing Apple Developer teams")?;
    if teams.is_empty() {
        bail!("No Apple Developer teams found for this Apple ID.");
    }
    if !team_id.is_empty() {
        let Some(pos) = teams.iter().position(|t| t.id == team_id) else {
            bail!("Team {team_id} is not available to {}.", session.apple_id);
        };
        return Ok(teams.remove(pos));
    }
    if let Some(pos) = teams.iter().position(|t| t.name.contains("Personal Team")) {
        return Ok(teams.remove(pos));
    }
    Ok(teams.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apple::capabilities::{self, CapabilityType, FreeFeature};

    fn named(name: &str) -> &'static CapabilityDef {
        capabilities::lookup_name(name).unwrap()
    }

    static VERIFIED: CapabilityDef = CapabilityDef {
        name: "Test Verified",
        entitlement_keys: &["com.apple.developer.test"],
        capability_type: CapabilityType::AppGroups,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Verified(FreeFeature {
            code: "TEST",
            value: "on",
        }),
        source: Some("test"),
    };

    #[test]
    fn free_plan_groups_by_free_feature() {
        let plan = FreePlan::new(&[named("App Groups"), named("HealthKit"), &VERIFIED]);
        assert_eq!(plan.verified.len(), 1);
        assert_eq!(plan.verified[0].name, "Test Verified");
        assert_eq!(plan.unverified.len(), 1);
        assert_eq!(plan.unverified[0].name, "App Groups");
        assert_eq!(plan.unsupported.len(), 1);
        assert_eq!(plan.unsupported[0].name, "HealthKit");
    }

    #[test]
    fn feature_pairs_use_table_codes() {
        let pairs = feature_pairs(&[named("App Groups"), named("Data Protection")]);
        assert_eq!(
            pairs,
            vec![("APG3427HIY", "on"), ("dataProtection", "complete")]
        );
    }

    #[test]
    fn unverified_failure_warns() {
        let caps = [named("App Groups")];
        let result = report_enable_result(
            Severity::Warning,
            &caps,
            "com.example.app",
            Err(anyhow::anyhow!("rejected")),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn verified_failure_errors() {
        let caps = [&VERIFIED];
        let err = report_enable_result(
            Severity::Error,
            &caps,
            "com.example.app",
            Err(anyhow::anyhow!("rejected")),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("com.example.app"), "{msg}");
        assert!(msg.contains("rejected"), "{msg}");
    }

    #[test]
    fn success_is_ok_for_either_severity() {
        let caps = [named("iCloud")];
        assert!(report_enable_result(Severity::Error, &caps, "a", Ok(true)).is_ok());
        assert!(report_enable_result(Severity::Warning, &caps, "a", Ok(false)).is_ok());
    }
}

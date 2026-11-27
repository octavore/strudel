//! iCloud containers on a developer team.
//!
//! Only `strudel icloud-containers create` creates containers, and only
//! `strudel icloud-containers link` links them to App IDs. Builds do not do
//! either. NOTE: Containers cannot be deleted, thus creating a container will
//! require user confirmation.
//!
//! Container commands use the Apple ID session of the account that owns the
//! team, because an App Store Connect API key cannot manage containers.
//!
//! Known limitation: Apple's create and associate actions are undocumented. See
//! `appleid::AppleId::create_cloud_container`.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};
use appleid::{AppleId, CloudContainer, Session, Team, TeamNotEligible};
use clml::cprintln;
use indoc::formatdoc;

use crate::apple::provisioning::get_apple_id;
use crate::apple::{accounts, capabilities};
use crate::cli::helpers::all_or_named;
use crate::config::{self, ResolvedConfig};
use crate::paths::StrudelData;

/// Name of the iCloud capability in strudel.toml.
const ICLOUD_CAPABILITY: &str = "iCloud";

/// Marker in Apple's error if the iCloud capability is off for the App ID.
const ICLOUD_NOT_ENABLED: &str = "iCloud feature should be enabled";

/// Validate identifier arguments and drop duplicates, keeping the first
/// occurrence. At least one identifier is required.
pub fn normalize_identifiers(identifiers: Vec<String>) -> Result<Vec<String>> {
    if identifiers.is_empty() {
        bail!("Give at least one container identifier, such as `iCloud.com.example.app`.");
    }
    let mut unique: Vec<String> = Vec::with_capacity(identifiers.len());
    for identifier in identifiers {
        appleid::validate_container_identifier(&identifier)?;
        if !unique.contains(&identifier) {
            unique.push(identifier);
        }
    }
    Ok(unique)
}

/// Requested identifiers that are not in `existing`, in request order.
pub fn missing_identifiers(requested: &[String], existing: &[CloudContainer]) -> Vec<String> {
    requested
        .iter()
        .filter(|id| !existing.iter().any(|c| &c.identifier == *id))
        .cloned()
        .collect()
}

/// Warning shown before any container is created. Lists every identifier in
/// `missing`, one per line.
pub fn creation_warning(missing: &[String], team: &Team) -> String {
    let identifiers: String = missing.iter().map(|id| format!("  {id}\n")).collect();
    formatdoc! {"
        WARNING: iCloud containers cannot be deleted.
        Once created, each identifier below stays reserved on your team permanently.
        It can be hidden and unlinked from App IDs, but never removed.

        Team: {name} ({id})

        {identifiers}",
        name = team.name,
        id = team.id,
    }
}

/// Display name sent when creating a container. Letters and digits of the
/// identifier suffix, separated by spaces.
///
/// Known limitation: the name format Apple accepts is unverified.
pub fn container_name(identifier: &str) -> String {
    identifier
        .strip_prefix("iCloud.")
        .unwrap_or(identifier)
        .split(['.', '-'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Team ID for a container command. A `--team` flag wins. Otherwise the team
/// ID of strudel.toml is used. Fails if neither is set, because creation is
/// permanent and a wrong team cannot be undone.
pub fn resolve_team_id(flag: Option<&str>, configured: Option<&str>) -> Result<String> {
    match flag.or(configured).filter(|id| !id.is_empty()) {
        Some(id) => Ok(id.to_string()),
        None => bail!(
            "No team ID. Pass `--team <id>`, or set `team_id` in the [apple] section of \
             strudel.toml."
        ),
    }
}

/// Team ID of the project in `config`, if the config loads and sets one.
fn configured_team_id(config: &Path) -> Option<String> {
    let project = config::load_config(config).ok()?;
    project
        .targets
        .iter()
        .map(|t| t.team_id.value.clone())
        .find(|id| !id.is_empty())
}

/// Team `team_id` from the teams of an account.
pub fn select_team(mut teams: Vec<Team>, team_id: &str) -> Result<Team> {
    match teams.iter().position(|t| t.id == team_id) {
        Some(pos) => Ok(teams.remove(pos)),
        None => bail!(
            "Team {team_id} is not available to this Apple ID. {}",
            team_list(&teams)
        ),
    }
}

fn team_list(teams: &[Team]) -> String {
    let items: Vec<String> = teams
        .iter()
        .map(|t| format!("{} ({})", t.name, t.id))
        .collect();
    format!("Available teams: {}.", items.join(", "))
}

fn team_label(team: &Team) -> String {
    format!("{} ({})", team.name, team.id)
}

/// Explain a failed container list. A team without a paid membership gets a
/// specific message.
fn explain_list_error(err: anyhow::Error, team: &Team) -> anyhow::Error {
    if err.downcast_ref::<TeamNotEligible>().is_some() {
        return err.context(format!(
            "iCloud containers need a paid Apple Developer Program team. Team {} ({}) is not eligible",
            team.name, team.id
        ));
    }
    err.context(format!(
        "listing iCloud containers for team {}",
        team_label(team)
    ))
}

/// Apple ID client, session, and team for container commands.
struct Connection {
    apple_id: AppleId,
    session: Session,
    team: Team,
}

/// Connect with the session of the account that owns `team_id`.
fn connect(team_id: &str) -> Result<Connection> {
    let data = StrudelData::locate()?;
    let apple_id = get_apple_id()?;
    let account = accounts::for_team(&data, &apple_id, team_id)?;
    let session = account.load_session()?;
    let teams = apple_id
        .list_teams(&session)
        .context("listing Apple Developer teams")?;
    let team = select_team(teams, team_id)?;
    Ok(Connection {
        apple_id,
        session,
        team,
    })
}

/// Print each container on the team. Read-only.
pub fn list(config: &Path, team_flag: Option<&str>) -> Result<()> {
    let team_id = resolve_team_id(team_flag, configured_team_id(config).as_deref())?;
    let ctx = connect(&team_id)?;
    let containers = ctx
        .apple_id
        .list_cloud_containers(&ctx.session, &ctx.team.id)
        .map_err(|e| explain_list_error(e, &ctx.team))?;
    cprintln!("<dim>Team: {} ({})</dim>", ctx.team.name, ctx.team.id);
    if containers.is_empty() {
        cprintln!("No iCloud containers on this team.");
    }
    for c in &containers {
        cprintln!("{}  <dim>{}</dim>", c.identifier, c.name);
    }
    Ok(())
}

/// Create the containers in `identifiers` that do not exist yet, after
/// confirmation.
pub fn create(identifiers: Vec<String>, config: &Path, team_flag: Option<&str>) -> Result<()> {
    let identifiers = normalize_identifiers(identifiers)?;
    if !std::io::stdin().is_terminal() {
        bail!("No terminal to confirm on. Run `strudel icloud-containers create` in a terminal.");
    }

    let team_id = resolve_team_id(team_flag, configured_team_id(config).as_deref())?;
    let ctx = connect(&team_id)?;
    let existing = ctx
        .apple_id
        .list_cloud_containers(&ctx.session, &ctx.team.id)
        .map_err(|e| explain_list_error(e, &ctx.team))?;
    let missing = missing_identifiers(&identifiers, &existing);
    for id in identifiers.iter().filter(|id| !missing.contains(id)) {
        cprintln!("<dim>{id} already exists, skipping.</dim>");
    }
    if missing.is_empty() {
        return Ok(());
    }

    let warning = creation_warning(&missing, &ctx.team);
    cprintln!("\n<yellow>{warning}</yellow>");
    let confirmed = inquire::Confirm::new("Create these containers?")
        .with_default(false)
        .prompt()
        .context("reading confirmation")?;
    if !confirmed {
        cprintln!("<dim>Aborted. No containers were created.</dim>");
        return Ok(());
    }

    let mut created = 0;
    for identifier in &missing {
        let result = ctx.apple_id.create_cloud_container(
            &ctx.session,
            &ctx.team.id,
            identifier,
            &container_name(identifier),
        );
        match result {
            Ok(_) => {
                created += 1;
                cprintln!("<green>✔</green> Created {identifier}");
            },
            Err(err) => {
                return Err(err.context(format!(
                    "Failed to create {identifier} ({created} of {} created)",
                    missing.len()
                )));
            },
        }
    }
    Ok(())
}

/// Exact command that creates `missing` on `team_id`.
pub fn create_command(missing: &[String], team_id: &str) -> String {
    format!(
        "strudel icloud-containers create --team {team_id} {}",
        missing.join(" ")
    )
}

/// Container ids of `configured`, in order. Fails with the `create` command
/// if a configured container does not exist on `team`.
pub fn configured_container_ids(
    configured: &[String],
    existing: &[CloudContainer],
    team: &Team,
) -> Result<Vec<String>> {
    let missing = missing_identifiers(configured, existing);
    if !missing.is_empty() {
        bail!(
            "iCloud containers missing on team {}: {}.\n\
             Containers cannot be deleted, so linking never creates them. Create them with:\n\n  {}\n",
            team_label(team),
            missing.join(", "),
            create_command(&missing, &team.id)
        );
    }
    Ok(configured
        .iter()
        .filter_map(|id| existing.iter().find(|c| &c.identifier == id))
        .map(|c| c.id.clone())
        .collect())
}

/// Add context to an association error. Appends the capability fix on its own
/// paragraph if the iCloud capability is not enabled.
fn explain_associate_error(err: anyhow::Error, bundle_id: &str) -> anyhow::Error {
    let context = format!("associating iCloud containers with {bundle_id}");
    let detail = format!("{err:#}");
    if !detail.contains(ICLOUD_NOT_ENABLED) {
        return err.context(context);
    }
    anyhow::anyhow!(
        "{context}: {detail}\n\n\
         Containers require the iCloud capability on the App ID. Enable iCloud for it in the \
         developer portal, then run `strudel icloud-containers link` again."
    )
}

/// Feature pair that turns iCloud on for an App ID.
fn icloud_feature() -> Result<(&'static str, &'static str)> {
    let def = capabilities::lookup_name(ICLOUD_CAPABILITY)?;
    let feature = def
        .free_feature
        .value()
        .context("The iCloud App ID feature code is not known.")?;
    Ok((feature.code, feature.value))
}

/// True if `names` lists the iCloud capability.
fn lists_icloud(names: &[String]) -> bool {
    names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(ICLOUD_CAPABILITY))
}

/// Link the configured containers of each selected target to its App ID.
/// Offers to enable the iCloud capability on the App ID and to list it in
/// strudel.toml.
pub fn link(config: &Path, target: Option<&str>, team_flag: Option<&str>) -> Result<()> {
    let project = config::load_config(config)?;
    let targets = all_or_named(&project, target)?;
    let with_containers: Vec<&ResolvedConfig> = targets
        .iter()
        .copied()
        .filter(|cfg| !cfg.target_platform.container_identifiers().is_empty())
        .collect();
    if with_containers.is_empty() {
        bail!(
            "No containers are configured. Add `containers = [\"iCloud.com.example.app\"]` to \
             strudel.toml."
        );
    }
    let interactive = std::io::stdin().is_terminal();
    for cfg in with_containers {
        cprintln!(
            "\n<bold,cyan>{}</bold,cyan>  <dim>{}</dim>",
            cfg.target_id,
            cfg.bundle_id
        );
        link_target(config, cfg, team_flag, interactive)?;
    }
    Ok(())
}

fn link_target(
    config: &Path,
    cfg: &ResolvedConfig,
    team_flag: Option<&str>,
    interactive: bool,
) -> Result<()> {
    let team_id = resolve_team_id(team_flag, Some(cfg.team_id.value.as_str()))?;
    let ctx = connect(&team_id)?;
    let bundle_id = cfg.bundle_id.as_str();

    let existing = ctx
        .apple_id
        .list_cloud_containers(&ctx.session, &ctx.team.id)
        .map_err(|e| explain_list_error(e, &ctx.team))?;
    let ids = configured_container_ids(
        cfg.target_platform.container_identifiers(),
        &existing,
        &ctx.team,
    )?;

    let (code, value) = icloud_feature()?;
    let missing = ctx
        .apple_id
        .missing_app_id_features(&ctx.session, &ctx.team.id, bundle_id, &[(code, value)])
        .with_context(|| format!("checking the App ID of {bundle_id}"))?;
    if !missing.is_empty() {
        enable_icloud(&ctx, cfg, interactive)?;
    }

    let changed = ctx
        .apple_id
        .ensure_cloud_containers(&ctx.session, &ctx.team.id, bundle_id, &cfg.app_name, &ids)
        .map_err(|err| explain_associate_error(err, bundle_id))?;
    let containers = cfg.target_platform.container_identifiers().join(", ");
    if changed {
        cprintln!("<green>✔</green> Linked {containers} to {bundle_id}");
    } else {
        cprintln!("<dim>{containers} already linked to {bundle_id}.</dim>");
    }

    offer_config_capability(config, cfg, interactive)
}

/// Enable the iCloud capability on the App ID after confirmation.
fn enable_icloud(ctx: &Connection, cfg: &ResolvedConfig, interactive: bool) -> Result<()> {
    let bundle_id = cfg.bundle_id.as_str();
    if !interactive {
        bail!(
            "The iCloud capability is off for {bundle_id}. Run `strudel icloud-containers link` \
             in a terminal to enable it."
        );
    }
    let prompt = format!("The iCloud capability is off for {bundle_id}. Enable it?");
    let confirmed = inquire::Confirm::new(&prompt)
        .with_default(true)
        .prompt()
        .context("reading confirmation")?;
    if !confirmed {
        bail!("Containers need the iCloud capability on {bundle_id}. Nothing was linked.");
    }
    let (code, value) = icloud_feature()?;
    ctx.apple_id
        .ensure_capabilities(
            &ctx.session,
            &ctx.team.id,
            bundle_id,
            &cfg.app_name,
            &[(code, value)],
        )
        .with_context(|| format!("enabling iCloud on {bundle_id}"))?;
    cprintln!("<green>✔</green> Enabled iCloud on {bundle_id}");
    Ok(())
}

/// Offer to list iCloud in the `capabilities` of strudel.toml, so builds keep
/// the capability enabled.
fn offer_config_capability(config: &Path, cfg: &ResolvedConfig, interactive: bool) -> Result<()> {
    if lists_icloud(cfg.target_platform.capability_names()) {
        return Ok(());
    }
    let platform = cfg.target_platform.platform();
    let hint = format!("Add \"{ICLOUD_CAPABILITY}\" to `capabilities` in strudel.toml.");
    if !interactive {
        cprintln!("<yellow>note:</yellow> {hint}");
        return Ok(());
    }
    let prompt = format!(
        "Add \"{ICLOUD_CAPABILITY}\" to `capabilities` of {} in {}?",
        cfg.target_id,
        config.display()
    );
    let confirmed = inquire::Confirm::new(&prompt)
        .with_default(true)
        .prompt()
        .context("reading confirmation")?;
    if !confirmed {
        cprintln!("<dim>{hint}</dim>");
        return Ok(());
    }
    config::edit::add_capability(config, platform, &cfg.app_name, ICLOUD_CAPABILITY)?;
    cprintln!(
        "<green>✔</green> Added \"{ICLOUD_CAPABILITY}\" to capabilities in {}",
        config.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(id: &str, name: &str) -> Team {
        Team {
            id: id.into(),
            name: name.into(),
            status: "active".into(),
        }
    }

    fn container(identifier: &str) -> CloudContainer {
        CloudContainer {
            id: format!("ID-{identifier}"),
            identifier: identifier.into(),
            name: String::new(),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_identifiers_is_an_error() {
        assert!(normalize_identifiers(Vec::new()).is_err());
    }

    #[test]
    fn duplicate_identifiers_collapse() {
        let got = normalize_identifiers(strings(&[
            "iCloud.com.example.a",
            "iCloud.com.example.b",
            "iCloud.com.example.a",
        ]))
        .unwrap();
        assert_eq!(got, ["iCloud.com.example.a", "iCloud.com.example.b"]);
    }

    #[test]
    fn invalid_identifier_is_an_error() {
        assert!(normalize_identifiers(strings(&["com.example.a"])).is_err());
    }

    #[test]
    fn missing_identifiers_skips_existing() {
        let existing = [container("iCloud.com.example.a")];
        let requested = strings(&["iCloud.com.example.a", "iCloud.com.example.b"]);
        assert_eq!(
            missing_identifiers(&requested, &existing),
            ["iCloud.com.example.b"]
        );
        assert!(missing_identifiers(&requested[..1], &existing).is_empty());
    }

    #[test]
    fn warning_states_permanence_and_lists_every_identifier() {
        let missing = strings(&["iCloud.com.example.a", "iCloud.com.example.b"]);
        let text = creation_warning(&missing, &team("T1", "Example LLC"));
        assert!(text.contains("cannot be deleted"), "{text}");
        assert!(text.contains("  iCloud.com.example.a\n"), "{text}");
        assert!(text.contains("  iCloud.com.example.b\n"), "{text}");
        assert!(text.contains("Example LLC (T1)"), "{text}");
    }

    #[test]
    fn container_name_uses_the_identifier_suffix() {
        assert_eq!(
            container_name("iCloud.com.example.my-app"),
            "com example my app"
        );
    }

    #[test]
    fn team_id_comes_from_the_flag_then_the_config() {
        assert_eq!(resolve_team_id(Some("T1"), Some("T2")).unwrap(), "T1");
        assert_eq!(resolve_team_id(None, Some("T2")).unwrap(), "T2");
        assert_eq!(resolve_team_id(Some("T1"), None).unwrap(), "T1");
    }

    #[test]
    fn missing_team_id_is_an_error() {
        for configured in [None, Some("")] {
            let err = resolve_team_id(None, configured).unwrap_err().to_string();
            assert!(err.contains("--team") && err.contains("team_id"), "{err}");
        }
    }

    #[test]
    fn select_team_finds_the_team() {
        let teams = vec![team("T1", "One"), team("T2", "Two")];
        assert_eq!(select_team(teams.clone(), "T2").unwrap().id, "T2");
        let err = select_team(teams, "T9").unwrap_err().to_string();
        assert!(err.contains("T9") && err.contains("T1"), "{err}");
        assert!(select_team(Vec::new(), "T1").is_err());
    }

    #[test]
    fn missing_container_stops_with_the_create_command() {
        let existing = [container("iCloud.com.example.a")];
        let wanted = strings(&["iCloud.com.example.a", "iCloud.com.example.b"]);
        let err = configured_container_ids(&wanted, &existing, &team("T1", "One"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("strudel icloud-containers create --team T1 iCloud.com.example.b"),
            "{err}"
        );
        assert!(
            !err.contains("create --team T1 iCloud.com.example.a"),
            "{err}"
        );
    }

    #[test]
    fn configured_containers_resolve_to_container_ids_in_order() {
        let existing = [
            container("iCloud.com.example.a"),
            container("iCloud.com.example.b"),
        ];
        let wanted = strings(&["iCloud.com.example.b", "iCloud.com.example.a"]);
        let got = configured_container_ids(&wanted, &existing, &team("T1", "One")).unwrap();
        assert_eq!(got, ["ID-iCloud.com.example.b", "ID-iCloud.com.example.a"]);
    }

    #[test]
    fn icloud_not_enabled_error_gets_a_hint() {
        let err = anyhow::anyhow!("code 1200: iCloud feature should be enabled before associating");
        let text = explain_associate_error(err, "com.example.app").to_string();
        assert!(text.contains("Enable iCloud"), "{text}");
        let other = explain_associate_error(anyhow::anyhow!("boom"), "com.example.app");
        assert!(!other.to_string().contains("Enable iCloud"));
    }

    #[test]
    fn lists_icloud_ignores_case() {
        assert!(lists_icloud(&strings(&["HealthKit", "icloud"])));
        assert!(!lists_icloud(&strings(&["HealthKit"])));
        assert!(!lists_icloud(&[]));
    }

    #[test]
    fn icloud_feature_is_known() {
        assert_eq!(icloud_feature().unwrap(), ("iCloud", "on"));
    }
}

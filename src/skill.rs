use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use color_print::cprintln;

use crate::help;

const RELEASE_ACTION_SKILL_MD: &str = include_str!("../claude/strudel-release-action/SKILL.md");
const RELEASE_ACTION_YML_TEMPLATE: &str =
    include_str!("../claude/strudel-release-action/assets/release.yml.template");
const RELEASE_ACTION_SH_TEMPLATE: &str =
    include_str!("../claude/strudel-release-action/assets/release.sh.template");

/// Resolves the base dir `skill install` writes into. If project is true, the
/// skill is installed into the current folder, otherwise it gets installed in
/// the home directory. `agents` picks the `.agents/skills` convention instead
/// of Claude Code's `.claude/skills` (the default)
pub(crate) fn resolve_skills_dir(project: bool, agents: bool) -> PathBuf {
    let name = if agents { "agents" } else { "claude" };
    let rel = format!(".{name}/skills");
    if project {
        PathBuf::from(rel)
    } else {
        PathBuf::from(shellexpand::tilde(&format!("~/{rel}")).as_ref())
    }
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum SkillKind {
    /// Points an AI agent at `strudel help` for this project's build/release
    /// docs
    Strudel,
    /// Sets up the `octavore/strudel-release-action` GitHub Action (signing,
    /// notarization, DMG packaging, release workflow)
    ReleaseAction,
}

impl SkillKind {
    /// All installable skills, in the order they're offered to the user.
    pub(crate) const ALL: &'static [SkillKind] = &[SkillKind::Strudel, SkillKind::ReleaseAction];

    fn dir_name(self) -> &'static str {
        match self {
            SkillKind::Strudel => "strudel",
            SkillKind::ReleaseAction => "strudel-release-action",
        }
    }

    /// (path relative to the skill's own directory, file content)
    fn files(self, app: &clap::Command) -> Vec<(&'static str, String)> {
        match self {
            SkillKind::Strudel => vec![("SKILL.md", generate_skill_md(app))],
            SkillKind::ReleaseAction => vec![
                ("SKILL.md", RELEASE_ACTION_SKILL_MD.to_string()),
                (
                    "assets/release.yml.template",
                    RELEASE_ACTION_YML_TEMPLATE.to_string(),
                ),
                (
                    "assets/release.sh.template",
                    RELEASE_ACTION_SH_TEMPLATE.to_string(),
                ),
            ],
        }
    }
}

impl Display for SkillKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let desc = match self {
            SkillKind::Strudel => {
                "points an AI agent at `strudel help` for this project's build/release docs"
            },
            SkillKind::ReleaseAction => {
                "sets up the `octavore/strudel-release-action` GitHub Actions release workflow"
            },
        };
        write!(f, "{} - {desc}", self.dir_name())
    }
}

/// Renders the `strudel` skill's SKILL.md content. Both the command list
/// and the topic list are pulled from the CLI's own definitions
/// (`help::commands`/`help::TOPICS`) rather than hand-copied, so this can't
/// drift out of sync as commands or topics are added, renamed, or removed.
pub(crate) fn generate_skill_md(app: &clap::Command) -> String {
    let mut commands = String::new();
    for (name, about) in help::commands(app) {
        commands.push_str(&format!("- **`strudel {name}`** - {about}\n"));
    }

    let mut topics = String::new();
    for (name, desc) in help::TOPICS {
        topics.push_str(&format!("- **`{name}`** - {desc}\n"));
    }

    indoc::formatdoc! {r#"
        ---
        name: strudel
        description: Build, sign, notarize, and package this project's macOS/iOS Swift app via the `strudel` CLI. Use this whenever the user wants to build, run, or release the app, asks about strudel.toml, code signing, notarization, DMG packaging, or CI setup for this project. Trigger on any mention of strudel, or of app bundles, codesigning, notarization, DMG packaging, iOS simulator, or device builds in this repo.
        ---

        # strudel

        This project is built with [strudel](https://github.com/octavore/strudel). The lists below are generated from the installed strudel's own command/topic definitions, so they can't go stale — but for anything not covered here, run `strudel help <topic>` or `strudel <command> --help` rather than assuming; the installed CLI's own docs are authoritative and stay in sync with the version actually in use here, which this file does not.

        ## Commands

        {commands}
        ## Topics

        Run `strudel help <topic>` for any of:

        {topics}
        ## Quick reference

        `strudel <command> --help` for a command's full usage. `strudel help` with no argument lists commands and topics together.
    "#}
}

/// `strudel skill install --preview`: print the chosen skill's SKILL.md to
/// stdout without writing anything, so the user can inspect it before it
/// gets installed. Bundled asset files (for skills that have them) are only
/// named, not dumped, to keep this readable.
pub(crate) fn print_preview(kind: SkillKind, app: &clap::Command) {
    let files = kind.files(app);
    let (_, skill_md) = &files[0];
    print!("{skill_md}");
    if files.len() > 1 {
        println!("\n(also installs {} additional file(s):", files.len() - 1);
        for (path, _) in &files[1..] {
            println!("  {path}");
        }
        println!(")");
    }
}

/// `strudel skill install [KIND]`: write the chosen skill's file(s) under
/// `<dir>/<kind-dir-name>/`. Each file is skipped (rather than overwritten)
/// if it already exists, unless `force` is set — matching the `strudel init`
/// convention for secondary scaffolded files — so a partial prior install is
/// topped up rather than blocked outright.
pub(crate) fn run_install(
    dir: &Path,
    kind: SkillKind,
    app: &clap::Command,
    force: bool,
) -> Result<()> {
    let skill_dir = dir.join(kind.dir_name());
    for (rel_path, content) in kind.files(app) {
        let path = skill_dir.join(rel_path);
        if path.exists() && !force {
            cprintln!(
                "Skipped <dim>{}</dim> (already exists; use --force to overwrite)",
                path.display()
            );
            continue;
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        cprintln!("Created <dim>{}</dim>", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;
    use crate::cli::Cli;

    fn test_app() -> clap::Command {
        Cli::command()
    }

    #[test]
    fn generated_skill_md_lists_every_topic_and_command() {
        let app = test_app();
        let content = generate_skill_md(&app);
        assert!(content.starts_with("---\nname: strudel\n"));
        for (name, desc) in help::TOPICS {
            assert!(
                content.contains(&format!("`{name}`")),
                "missing topic {name}"
            );
            assert!(content.contains(desc), "missing description for {name}");
        }
        for (name, _) in help::commands(&app) {
            assert!(
                content.contains(&format!("`strudel {name}`")),
                "missing command {name}"
            );
        }
    }

    #[test]
    fn project_flag_resolves_to_a_relative_dir() {
        assert_eq!(
            resolve_skills_dir(true, false),
            PathBuf::from(".claude/skills")
        );
        assert_eq!(
            resolve_skills_dir(true, true),
            PathBuf::from(".agents/skills")
        );
    }

    #[test]
    fn default_scope_is_global_and_expands_tilde() {
        for (agents, dir_name) in [(false, "claude"), (true, "agents")] {
            let path = resolve_skills_dir(false, agents);
            assert!(
                path.is_absolute(),
                "expected an absolute path, got {path:?}"
            );
            assert!(path.ends_with(format!(".{dir_name}/skills")));
        }
    }

    #[test]
    fn release_action_skill_md_is_embedded() {
        assert!(RELEASE_ACTION_SKILL_MD.starts_with("---\nname: strudel-release-action\n"));
        assert!(RELEASE_ACTION_YML_TEMPLATE.contains("strudel-release-action@v1"));
        assert!(RELEASE_ACTION_SH_TEMPLATE.contains("strudel config increment-version"));
    }

    #[test]
    fn run_install_creates_parent_dirs_and_writes_file() {
        let dir = tempfile::TempDir::new().unwrap();

        run_install(dir.path(), SkillKind::Strudel, &test_app(), false).unwrap();

        let path = dir.path().join("strudel/SKILL.md");
        assert!(path.exists());
        assert!(fs::read_to_string(&path).unwrap().contains("name: strudel"));
    }

    #[test]
    fn run_install_writes_all_files_for_a_multi_file_skill() {
        let dir = tempfile::TempDir::new().unwrap();

        run_install(dir.path(), SkillKind::ReleaseAction, &test_app(), false).unwrap();

        let skill_dir = dir.path().join("strudel-release-action");
        assert!(skill_dir.join("SKILL.md").exists());
        assert!(skill_dir.join("assets/release.yml.template").exists());
        assert!(skill_dir.join("assets/release.sh.template").exists());
    }

    #[test]
    fn run_install_skips_existing_file_without_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("strudel/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "existing").unwrap();

        run_install(dir.path(), SkillKind::Strudel, &test_app(), false).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "existing");
    }

    #[test]
    fn run_install_overwrites_when_forced() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("strudel/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "existing").unwrap();

        run_install(dir.path(), SkillKind::Strudel, &test_app(), true).unwrap();

        assert!(fs::read_to_string(&path).unwrap().contains("name: strudel"));
    }

    #[test]
    fn run_install_tops_up_a_partial_install_without_force() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_dir = dir.path().join("strudel-release-action");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "existing").unwrap();

        run_install(dir.path(), SkillKind::ReleaseAction, &test_app(), false).unwrap();

        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "existing"
        );
        assert!(skill_dir.join("assets/release.yml.template").exists());
    }
}

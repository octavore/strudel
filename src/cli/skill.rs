use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Subcommand};

use crate::cli::Cli;
use crate::skill::{self, SkillKind};

#[derive(clap::Args)]
pub(crate) struct SkillCmd {
    #[command(subcommand)]
    command: SkillAction,
}

#[derive(Subcommand)]
enum SkillAction {
    /// Write a skill (SKILL.md, plus any bundled files) that points an AI
    /// coding agent at strudel docs or tooling for this project. With no
    /// argument, prompts with a multi-select of every installable skill.
    Install {
        /// Which skill to install; omit to pick interactively
        kind: Option<SkillKind>,
        /// Directory to install into; a `<skill-name>/` subdirectory is
        /// created inside it (overrides --project / --agents)
        #[arg(long)]
        path: Option<PathBuf>,
        /// Overwrite files that already exist
        #[arg(long)]
        force: bool,
        /// Print the generated SKILL.md to stdout instead of writing it
        #[arg(long)]
        preview: bool,
        /// Install into this project instead of the user-global dir
        #[arg(long, conflicts_with = "path")]
        project: bool,
        /// Use the .agents/skills convention instead of .claude/skills
        #[arg(long, conflicts_with = "path")]
        agents: bool,
    },
}

impl SkillCmd {
    pub(crate) fn execute(self) -> Result<()> {
        match self.command {
            SkillAction::Install {
                kind,
                path,
                force,
                preview,
                project,
                agents,
            } => {
                let kinds = match kind {
                    Some(k) => vec![k],
                    None => select_kinds()?,
                };
                if kinds.is_empty() {
                    println!("Nothing selected.");
                    return Ok(());
                }

                let dir = path.unwrap_or_else(|| skill::resolve_skills_dir(project, agents));
                let app = Cli::command();

                for (i, kind) in kinds.into_iter().enumerate() {
                    if preview {
                        if i > 0 {
                            println!("\n---\n");
                        }
                        skill::print_preview(kind, &app);
                    } else {
                        skill::run_install(&dir, kind, &app, force)?;
                    }
                }
                Ok(())
            },
        }
    }
}

/// Prompts with a checkbox list of every installable skill, defaulting to
/// just `strudel` checked (space to toggle, enter to confirm).
fn select_kinds() -> Result<Vec<SkillKind>> {
    let selected = inquire::MultiSelect::new(
        "Which skill(s) do you want to install?",
        SkillKind::ALL.to_vec(),
    )
    .with_default(&[0])
    .prompt()?;
    Ok(selected)
}

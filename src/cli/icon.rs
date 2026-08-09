use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clml::cprintln;

use crate::cli::helpers::all_or_named;
use crate::config::{self, ResolvedIcon};
use crate::icon::render::render_to_png;

#[derive(clap::Args)]
pub(crate) struct IconCmd {
    /// Select a target by id
    #[arg(long)]
    target: Option<String>,

    /// Directory to write rendered icons into
    #[arg(long, default_value = ".")]
    out: PathBuf,
}

impl IconCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        let project = config::load_config(config)?;
        let targets = all_or_named(&project, self.target.as_deref())?;

        fs::create_dir_all(&self.out)
            .with_context(|| format!("Failed to create {}", self.out.display()))?;

        for cfg in targets {
            let Some(icon) = &cfg.icon else {
                cprintln!("<dim>{}: no icon configured, skipping</dim>", cfg.target_id);
                continue;
            };

            let ext = match icon {
                ResolvedIcon::Path { path, .. } => {
                    path.extension().and_then(|e| e.to_str()).unwrap_or("png")
                },
                ResolvedIcon::Generated { .. } => "png",
            };
            let dest = self
                .out
                .join(format!("{}-icon.{ext}", cfg.target_id.replace('/', "-")));

            render_to_png(icon, &dest)?;
            cprintln!("<green>wrote</green> <blue>{}</blue>", dest.display());
        }

        Ok(())
    }
}

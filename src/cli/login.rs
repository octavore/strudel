use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::apple::provisioning;
use crate::config::{self, ResolvedTargetPlatform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct LoginCmd {
    #[command(subcommand)]
    command: Option<LoginAction>,

    /// Apple ID email address (prompted if omitted)
    #[arg(long)]
    apple_id: Option<String>,
}

#[derive(Subcommand)]
enum LoginAction {
    /// Show just the Apple ID session: signed-in state, apple id, and dsid.
    /// Run `strudel status` for the full picture (config, project, etc.)
    Status,
    /// Sign out and clear the saved Apple ID session and cached dev
    /// credentials
    Clear,
}

impl LoginCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            None => {
                // If no --apple-id flag, try reading it from [ios] apple_id in
                // strudel.toml (best-effort; not required for login to work).
                let apple_id_email = match self.apple_id.as_deref() {
                    Some(id) => Some(id.to_string()),
                    None => config::load_config(config).ok().and_then(|p| {
                        p.targets.into_iter().find_map(|t| match t.target_platform {
                            ResolvedTargetPlatform::Ios(ref ios) => ios.apple_id.clone(),
                            _ => None,
                        })
                    }),
                };
                provisioning::login(apple_id_email)
            },
            Some(LoginAction::Status) => status::login_status(),
            Some(LoginAction::Clear) => provisioning::logout(),
        }
    }
}

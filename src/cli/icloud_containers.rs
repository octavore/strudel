use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::apple::containers;

#[derive(clap::Args)]
pub(crate) struct IcloudContainersCmd {
    #[command(subcommand)]
    command: IcloudContainersAction,
}

#[derive(Subcommand)]
enum IcloudContainersAction {
    /// Create iCloud containers on the team, after confirmation.
    ///
    /// Containers cannot be deleted. Each identifier stays reserved on the
    /// team permanently. Identifiers that already exist are skipped. Does not
    /// read `containers` from strudel.toml. Requires a terminal. Needs a paid
    /// Apple Developer Program team and an Apple ID session (`strudel login`)
    /// of the account that owns the team. Apple's container endpoints are
    /// undocumented.
    ///
    /// The usual identifier is `iCloud.` followed by the bundle ID, such as
    /// iCloud.com.example.app for bundle ID com.example.app.
    Create {
        /// Container identifiers, such as iCloud.com.example.app
        #[arg(required = true)]
        identifiers: Vec<String>,

        /// Team id. Defaults to `team_id` in strudel.toml. The team id picks
        /// the signed-in Apple ID.
        #[arg(long)]
        team: Option<String>,
    },
    /// List the iCloud containers on the team. Read-only.
    List {
        /// Team id. Defaults to `team_id` in strudel.toml. The team id picks
        /// the signed-in Apple ID.
        #[arg(long)]
        team: Option<String>,
    },
    /// Link the `containers` of strudel.toml to the App ID of each target.
    ///
    /// Builds never link containers. Containers must exist already, so run
    /// `create` first. If the iCloud capability is off for the App ID, offers
    /// to enable it. If strudel.toml does not list iCloud in `capabilities`,
    /// offers to add it. Needs a paid Apple Developer Program team and an
    /// Apple ID session (`strudel login`) of the account that owns the team.
    Link {
        /// Team id. Defaults to `team_id` in strudel.toml. The team id picks
        /// the signed-in Apple ID.
        #[arg(long)]
        team: Option<String>,

        /// Select a target by id
        #[arg(long)]
        target: Option<String>,
    },
}

impl IcloudContainersCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            IcloudContainersAction::Create { identifiers, team } => {
                containers::create(identifiers, config, team.as_deref())
            },
            IcloudContainersAction::List { team } => containers::list(config, team.as_deref()),
            IcloudContainersAction::Link { team, target } => {
                containers::link(config, target.as_deref(), team.as_deref())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        cmd: IcloudContainersCmd,
    }

    #[test]
    fn create_without_identifiers_is_rejected() {
        assert!(TestCli::try_parse_from(["test", "create"]).is_err());
        assert!(TestCli::try_parse_from(["test", "create", "--team", "T1"]).is_err());
    }

    #[test]
    fn create_rejects_yes_flag() {
        let args = ["test", "create", "--yes", "iCloud.com.example.a"];
        assert!(TestCli::try_parse_from(args).is_err());
    }

    #[test]
    fn link_accepts_team_and_target() {
        let cli = TestCli::try_parse_from(["test", "link", "--team", "T1", "--target", "ios/App"])
            .unwrap();
        let IcloudContainersAction::Link { team, target } = cli.cmd.command else {
            panic!("expected link");
        };
        assert_eq!(team.as_deref(), Some("T1"));
        assert_eq!(target.as_deref(), Some("ios/App"));
        assert!(TestCli::try_parse_from(["test", "link"]).is_ok());
    }

    #[test]
    fn create_accepts_identifiers_and_flags() {
        let cli = TestCli::try_parse_from([
            "test",
            "create",
            "--team",
            "T1",
            "iCloud.com.example.a",
            "iCloud.com.example.b",
        ])
        .unwrap();
        let IcloudContainersAction::Create { identifiers, team } = cli.cmd.command else {
            panic!("expected create");
        };
        assert_eq!(identifiers.len(), 2);
        assert_eq!(team.as_deref(), Some("T1"));
    }
}

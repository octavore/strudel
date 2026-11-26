//! Apple ID accounts signed in with `strudel login`.
//!
//! Each account has a directory under `accounts/` that holds its session, a
//! cache of its teams, and the free-provisioning dev credentials. A team ID
//! selects the account.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use appleid::{AppleId, Session, Team};
use serde::{Deserialize, Serialize};

use crate::paths::StrudelData;

const SESSION_FILE: &str = "session.json";
const TEAMS_FILE: &str = "teams.json";
const CERT_FILE: &str = "dev-cert.der";
const KEY_FILE: &str = "dev-key.pem";

/// Team of an account, as saved at sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedTeam {
    pub id: String,
    pub name: String,
}

/// One signed-in Apple ID.
#[derive(Debug, Clone)]
pub struct Account {
    pub email: String,
    dir: PathBuf,
}

/// Directory name of an account. The DSID does not change if the Apple ID
/// email does. Characters other than letters and digits are replaced, so the
/// name is path-safe.
fn dir_name(dsid: &str) -> String {
    dsid.trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

impl Account {
    /// Account of `session`. Creates its directory. A new sign-in with the
    /// same DSID uses the same directory, even if the Apple ID email changed.
    pub fn create(data: &StrudelData, session: &Session) -> Result<Self> {
        let dir = data.accounts_dir.join(dir_name(&session.dsid));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Account {
            email: session.apple_id.trim().to_string(),
            dir,
        })
    }

    pub fn session_json(&self) -> PathBuf {
        self.dir.join(SESSION_FILE)
    }

    /// Cached DER-encoded development certificate.
    pub fn cert_der(&self) -> PathBuf {
        self.dir.join(CERT_FILE)
    }

    /// Cached development private key PEM.
    pub fn key_pem(&self) -> PathBuf {
        self.dir.join(KEY_FILE)
    }

    fn teams_json(&self) -> PathBuf {
        self.dir.join(TEAMS_FILE)
    }

    pub fn load_session(&self) -> Result<Session> {
        let path = self.session_json();
        let s = std::fs::read_to_string(&path).context("reading session file")?;
        serde_json::from_str(&s).with_context(|| {
            format!(
                "parsing session file (try `strudel login --apple-id {}` to refresh the session)",
                self.email
            )
        })
    }

    pub fn save_session(&self, session: &Session) -> Result<()> {
        let s = serde_json::to_string_pretty(session).context("serializing session")?;
        std::fs::write(self.session_json(), s).context("saving session")
    }

    /// Teams saved by the last sign-in or refresh. Empty if none.
    pub fn cached_teams(&self) -> Vec<CachedTeam> {
        std::fs::read_to_string(self.teams_json())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn has_team(&self, team_id: &str) -> bool {
        self.cached_teams().iter().any(|t| t.id == team_id)
    }

    /// List the teams of this account and save them.
    pub fn refresh_teams(&self, apple_id: &AppleId) -> Result<Vec<Team>> {
        let session = self.load_session()?;
        let teams = apple_id
            .list_teams(&session)
            .context("listing Apple Developer teams")?;
        let cached: Vec<CachedTeam> = teams
            .iter()
            .map(|t| CachedTeam {
                id: t.id.clone(),
                name: t.name.clone(),
            })
            .collect();
        let s = serde_json::to_string_pretty(&cached).context("serializing teams")?;
        std::fs::write(self.teams_json(), s).context("saving teams")?;
        Ok(teams)
    }

    /// Files of this account that exist.
    pub fn files(&self) -> Vec<PathBuf> {
        [
            self.session_json(),
            self.teams_json(),
            self.cert_der(),
            self.key_pem(),
        ]
        .into_iter()
        .filter(|p| p.exists())
        .collect()
    }

    /// Delete the account directory.
    pub fn remove(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.dir)
            .with_context(|| format!("removing {}", self.dir.display()))
    }
}

/// Signed-in accounts, sorted by Apple ID.
pub fn list(data: &StrudelData) -> Result<Vec<Account>> {
    let mut accounts = Vec::new();
    let Ok(entries) = std::fs::read_dir(&data.accounts_dir) else {
        return Ok(accounts);
    };
    for entry in entries {
        let dir = entry.context("reading accounts directory")?.path();
        let session_json = dir.join(SESSION_FILE);
        if !session_json.exists() {
            continue;
        }
        let email = std::fs::read_to_string(&session_json)
            .ok()
            .and_then(|s| serde_json::from_str::<Session>(&s).ok())
            .map(|s| s.apple_id)
            .unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().into_owned());
        accounts.push(Account { email, dir });
    }
    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    Ok(accounts)
}

fn not_signed_in() -> anyhow::Error {
    anyhow::anyhow!("Not signed in. Run `strudel login` to sign in with your Apple ID.")
}

fn emails(accounts: &[Account]) -> String {
    accounts
        .iter()
        .map(|a| a.email.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Indexes of the accounts that have `team_id` in their team cache.
fn matching(accounts: &[Account], team_id: &str) -> Vec<usize> {
    accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.has_team(team_id))
        .map(|(i, _)| i)
        .collect()
}

/// Account that belongs to `team_id`. Team caches are refreshed from Apple if
/// no cache has the team. Fails if no account or several accounts have it.
pub fn for_team(data: &StrudelData, apple_id: &AppleId, team_id: &str) -> Result<Account> {
    let mut accounts = list(data)?;
    if accounts.is_empty() {
        return Err(not_signed_in());
    }
    let mut found = matching(&accounts, team_id);
    if found.is_empty() {
        for account in &accounts {
            // An expired session fails here. The account is then not a match.
            let _ = account.refresh_teams(apple_id);
        }
        found = matching(&accounts, team_id);
    }
    match found.as_slice() {
        [index] => Ok(accounts.swap_remove(*index)),
        [] => bail!(
            "No signed-in Apple ID has access to team {team_id}. Signed in: {}.\n\
             Run `strudel login` with the Apple ID that belongs to the team.",
            emails(&accounts)
        ),
        _ => {
            let several: Vec<Account> = found.iter().map(|&i| accounts[i].clone()).collect();
            bail!(
                "Team {team_id} is available to several signed-in Apple IDs: {}.\n\
                 Run `strudel login clear` to remove the ones you do not use.",
                emails(&several)
            )
        },
    }
}

/// Account for a config. A non-empty `team_id` selects the account that owns
/// the team. Without one, the only signed-in account is used.
pub fn for_config(data: &StrudelData, apple_id: &AppleId, team_id: &str) -> Result<Account> {
    if !team_id.is_empty() {
        return for_team(data, apple_id, team_id);
    }
    let mut accounts = list(data)?;
    match accounts.len() {
        0 => Err(not_signed_in()),
        1 => Ok(accounts.remove(0)),
        _ => bail!(
            "Several Apple IDs are signed in: {}.\n\
             Set `team_id` in the [apple] section of strudel.toml to pick one.",
            emails(&accounts)
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn data(dir: &Path) -> StrudelData {
        StrudelData {
            accounts_dir: dir.join("accounts"),
            keychain_db: dir.join("kc"),
        }
    }

    fn session(email: &str, dsid: &str) -> Session {
        Session {
            apple_id: email.into(),
            dsid: dsid.into(),
            gs_token: "t".into(),
        }
    }

    /// Sign in `email`, with a DSID derived from it.
    fn sign_in(data: &StrudelData, email: &str, teams: &[&str]) -> Account {
        let session = session(email, &format!("dsid-{email}"));
        let account = Account::create(data, &session).unwrap();
        account.save_session(&session).unwrap();
        let cached: Vec<CachedTeam> = teams
            .iter()
            .map(|id| CachedTeam {
                id: (*id).into(),
                name: format!("Team {id}"),
            })
            .collect();
        std::fs::write(
            account.teams_json(),
            serde_json::to_string(&cached).unwrap(),
        )
        .unwrap();
        account
    }

    #[test]
    fn dir_name_is_path_safe() {
        assert_eq!(dir_name(" 000123456789 "), "000123456789");
        assert_eq!(dir_name("../a/b"), "___a_b");
    }

    #[test]
    fn same_dsid_reuses_the_account_after_an_email_change() {
        let tmp = tempfile::tempdir().unwrap();
        let data = data(tmp.path());
        for email in ["old@example.com", "new@example.com"] {
            let session = session(email, "123");
            let account = Account::create(&data, &session).unwrap();
            account.save_session(&session).unwrap();
        }
        let accounts = list(&data).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "new@example.com");
    }

    #[test]
    fn list_returns_signed_in_accounts_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let data = data(tmp.path());
        assert!(list(&data).unwrap().is_empty());
        sign_in(&data, "b@example.com", &["T2"]);
        sign_in(&data, "a@example.com", &["T1"]);
        Account::create(&data, &session("nosession@example.com", "9")).unwrap();
        let got: Vec<String> = list(&data).unwrap().into_iter().map(|a| a.email).collect();
        assert_eq!(got, ["a@example.com", "b@example.com"]);
    }

    #[test]
    fn matching_uses_the_team_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let data = data(tmp.path());
        sign_in(&data, "a@example.com", &["T1", "T2"]);
        sign_in(&data, "b@example.com", &["T2"]);
        let accounts = list(&data).unwrap();
        assert_eq!(matching(&accounts, "T1"), [0]);
        assert_eq!(matching(&accounts, "T2"), [0, 1]);
        assert!(matching(&accounts, "T9").is_empty());
    }

    #[test]
    fn remove_deletes_the_account_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let data = data(tmp.path());
        let account = sign_in(&data, "a@example.com", &["T1"]);
        assert_eq!(account.files().len(), 2);
        account.remove().unwrap();
        assert!(list(&data).unwrap().is_empty());
    }
}

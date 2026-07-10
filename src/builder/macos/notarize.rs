use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{cmp, fs};

use anyhow::{Context, Result, bail};
use color_print::{cprint, cprintln};
use serde::{Deserialize, Serialize};

use crate::builder::{MacosBuilder, step};
use crate::paths::PendingSubmission;
use crate::shell::ShellArg;

#[derive(Serialize, Deserialize)]
pub struct NotarizationState {
    pub submitted_at: u64,
    pub dmg_dest: String,
}

fn format_elapsed(submitted_at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now.saturating_sub(submitted_at);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

impl MacosBuilder {
    pub fn notary_auth_args(&self) -> Result<Vec<ShellArg>> {
        if let Some(auth) = self.cfg.notary_auth() {
            let mut args = vec![
                "--key".into(),
                auth.key_path.into(),
                "--key-id".into(),
                auth.key_id.into(),
            ];
            if let Some(issuer) = auth.issuer {
                args.push("--issuer".into());
                args.push(issuer.into());
            }
            return Ok(args);
        }
        if self.dry_run {
            cprintln!("<red>Error: No notarization credentials configured.</red>");
            Ok(vec![
                "--key".into(),
                "MISSING!".into(),
                "--key-id".into(),
                "MISSING!".into(),
                "--issuer".into(),
                "MISSING!".into(),
            ])
        } else {
            bail!("No notarization credentials configured")
        }
    }

    pub fn poll_notarization(
        &self,
        uuid: &str,
        pending: &PendingSubmission,
        dmg_dest: &Path,
        auth_args: &[ShellArg],
    ) -> Result<()> {
        step("Waiting for notarization...");

        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Would poll notarytool info {uuid} until accepted");
            return self.finalize_notarization(pending, dmg_dest);
        }

        const POLL_SECS: u64 = 20;
        let started = Instant::now();
        let timeout = Duration::from_secs(self.cfg.notarize_timeout);

        loop {
            // Check status immediately on entry (and again after each wait) so
            // resuming a submission that already finished doesn't sit through
            // a needless countdown before finding out.
            let v = self.notarytool_info(uuid, auth_args)?;
            let status = v["status"].as_str().unwrap_or("unknown");

            let apple_status = match status {
                "Accepted" => {
                    println!();
                    cprintln!("  <green>Accepted!</green>");
                    return self.finalize_notarization(pending, dmg_dest);
                },
                "Invalid" | "Rejected" => {
                    println!();
                    let log = self.notarytool_log(uuid, auth_args);
                    bail!(
                        "Notarization {status}.\n\
                         Submission ID: {uuid}\n\
                         {}",
                        match &log {
                            Ok(s) => s.as_str(),
                            Err(_) => "Run `xcrun notarytool log` above for details.",
                        }
                    );
                },
                other => other.to_string(),
            };

            let elapsed = started.elapsed();
            if elapsed >= timeout {
                println!();
                bail!(
                    "Notarization timed out after {}s.\n\
                     Submission ID: {uuid}\n\
                     Run `strudel release --resume` to continue when Apple finishes processing.",
                    self.cfg.notarize_timeout
                );
            }

            // Tick every second so the elapsed time and countdown stay live.
            for remaining in (1..=POLL_SECS).rev() {
                let elapsed_s = started.elapsed().as_secs();
                let waited = if elapsed_s < 60 {
                    format!("{elapsed_s}s")
                } else {
                    format!("{}m{}s", elapsed_s / 60, elapsed_s % 60)
                };
                cprint!(
                    "\x1b[2K\r  <dim>{apple_status}: {waited} elapsed, next poll in {remaining}s</dim>"
                );
                std::io::stdout().flush().ok();
                sleep(Duration::from_secs(1));
            }
        }
    }

    fn finalize_notarization(&self, pending: &PendingSubmission, dmg_dest: &Path) -> Result<()> {
        let pending_dmg_str = pending.dmg.to_str().unwrap();
        let app_bundle_str = self.paths.app_bundle.to_str().unwrap();

        step("Stapling app bundle...");
        self.sh
            .run(&["xcrun", "stapler", "staple", app_bundle_str])?;

        step("Stapling DMG...");
        self.sh
            .run(&["xcrun", "stapler", "staple", pending_dmg_str])?;

        step("Moving DMG to output...");
        if !self.dry_run {
            if let Some(parent) = dmg_dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&pending.dmg, dmg_dest)?;
            fs::remove_dir_all(&pending.dir)?;
        } else {
            cprintln!(
                "<dim>[dry-run]</dim> mv {} {}",
                pending.dmg.display(),
                dmg_dest.display()
            );
            cprintln!("<dim>[dry-run]</dim> rm -rf {}", pending.dir.display());
        }

        Ok(())
    }

    fn notarytool_log(&self, uuid: &str, auth_args: &[ShellArg]) -> Result<String> {
        let output = Command::new("xcrun")
            .args(["notarytool", "log", uuid])
            .args(
                auth_args
                    .iter()
                    .flat_map(|arg| Into::<Vec<String>>::into(arg.clone())),
            )
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("notarytool log failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn notarytool_info(&self, uuid: &str, auth_args: &[ShellArg]) -> Result<serde_json::Value> {
        let output = Command::new("xcrun")
            .args(["notarytool", "info", uuid])
            .args(
                auth_args
                    .iter()
                    .flat_map(|arg| Into::<Vec<String>>::into(arg.clone())),
            )
            .args(["--output-format", "json"])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("notarytool info failed: {}", stderr.trim());
        }
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
            .context("Failed to parse notarytool info output")
    }

    fn find_pending_submissions(&self) -> Result<Vec<(String, NotarizationState)>> {
        let strudel_dir = &self.paths.strudel_dir;
        if !strudel_dir.exists() {
            return Ok(vec![]);
        }
        let mut results = vec![];
        for entry in fs::read_dir(strudel_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let state_path = entry.path().join("pending-notarization.toml");
            if !state_path.exists() {
                continue;
            }
            let uuid = entry.file_name().to_string_lossy().into_owned();
            let contents = fs::read_to_string(&state_path)
                .with_context(|| format!("Failed to read {}", state_path.display()))?;
            let state: NotarizationState = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse {}", state_path.display()))?;
            results.push((uuid, state));
        }
        results.sort_by_key(|b| cmp::Reverse(b.1.submitted_at));
        Ok(results)
    }

    pub fn resume_notarization(&self, uuid_hint: &str) -> Result<()> {
        let pending = self.find_pending_submissions()?;
        let (uuid, state) = if uuid_hint.is_empty() {
            match pending.len() {
                // no pending submissions
                0 => bail!("No pending notarization found in .strudel"),

                // exactly one: assume that's the one to resume
                1 => pending.into_iter().next().unwrap(),

                // too many to guess: require explicit UUID
                _ => {
                    cprintln!(
                        "<red>Multiple pending notarizations found. Specify which to resume:</red>"
                    );
                    for (uuid, state) in &pending {
                        cprintln!(
                            "  <cyan>{uuid}</cyan> (submitted {})",
                            format_elapsed(state.submitted_at)
                        );
                    }
                    let most_recent = &pending[0].0;
                    bail!("Run: strudel release --resume {most_recent}");
                },
            }
        } else {
            pending
                .into_iter()
                .find(|(u, _)| u == uuid_hint)
                .with_context(|| format!("No pending notarization found for UUID: {uuid_hint}"))?
        };

        let pending = self.paths.pending_submission(&uuid);
        let dmg_dest = PathBuf::from(&state.dmg_dest);
        let auth_args = self.notary_auth_args()?;

        step("Resuming notarization...");
        cprintln!("  <dim>Submission ID: {uuid}</dim>");

        self.poll_notarization(&uuid, &pending, &dmg_dest, &auth_args)?;

        println!();
        cprintln!(
            "<green>Done!</green> DMG: <cyan>{}</cyan>",
            dmg_dest.display()
        );
        Ok(())
    }
}

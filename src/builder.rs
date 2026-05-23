use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use color_print::cprintln;
use serde_json::Value;

use crate::config::ResolvedConfig;
use crate::paths::Paths;
use crate::shell::Shell;

pub struct Builder {
    cfg: ResolvedConfig,
    p: Paths,
    sh: Shell,
}

fn step(msg: &str) {
    cprintln!("\n<green>==>> {}</green>", msg);
}

impl Builder {
    pub fn new(cfg: ResolvedConfig, dry_run: bool) -> Self {
        let p = Paths::new(&cfg);
        Builder {
            cfg,
            p,
            sh: Shell::new(dry_run),
        }
    }

    pub fn clean(&self) -> Result<()> {
        step("Cleaning previous build...");
        if self.p.build_dir.exists() {
            fs::remove_dir_all(&self.p.build_dir)?;
        }
        fs::create_dir_all(&self.p.build_dir)?;
        Ok(())
    }

    pub fn build_binary(&self) -> Result<PathBuf> {
        step("Building release binary...");

        let source = self.cfg.source_dir.to_str().unwrap();
        let arch_flags: Vec<String> = self
            .cfg
            .archs
            .iter()
            .flat_map(|a| ["--arch".to_string(), a.clone()])
            .collect();

        // Build base args shared between both swift invocations
        let mut base: Vec<String> = vec![
            "build".to_string(),
            "-c".to_string(),
            "release".to_string(),
            "--package-path".to_string(),
            source.to_string(),
        ];
        base.extend(arch_flags);

        let build_refs: Vec<&str> = std::iter::once("swift")
            .chain(base.iter().map(String::as_str))
            .collect();
        self.sh.run_streamed(&build_refs)?;

        let mut show_base = base.clone();
        show_base.push("--show-bin-path".to_string());
        let show_refs: Vec<&str> = std::iter::once("swift")
            .chain(show_base.iter().map(String::as_str))
            .collect();
        let bin_dir = self.sh.run(&show_refs)?;
        let bin_dir = bin_dir.trim();

        let binary_path = if bin_dir.is_empty() {
            // dry-run: fall back to expected location
            self.cfg
                .source_dir
                .join(".build/release")
                .join(&self.cfg.target_name)
        } else {
            PathBuf::from(bin_dir).join(&self.cfg.target_name)
        };

        if !bin_dir.is_empty() && !binary_path.exists() {
            bail!("Could not locate built binary. Check the swift build output above.");
        }

        Ok(binary_path)
    }

    pub fn assemble_bundle(&self, binary_path: &Path) -> Result<PathBuf> {
        step("Assembling app bundle...");
        let app_bundle = &self.p.app_bundle;

        fs::create_dir_all(app_bundle.join("Contents/MacOS"))?;
        fs::create_dir_all(app_bundle.join("Contents/Resources"))?;

        fs::copy(
            binary_path,
            app_bundle.join("Contents/MacOS").join(&self.cfg.app_name),
        )?;

        // Read info JSON and override version/identity fields
        let info_str = fs::read_to_string(&self.cfg.info_json_path)?;
        let mut info: Value = serde_json::from_str(&info_str)?;
        let obj = info.as_object_mut().unwrap();
        obj.insert(
            "CFBundleShortVersionString".to_string(),
            Value::String(self.cfg.version.clone()),
        );
        obj.insert(
            "CFBundleVersion".to_string(),
            Value::String(self.cfg.build_number.clone()),
        );
        obj.insert(
            "CFBundleIdentifier".to_string(),
            Value::String(self.cfg.bundle_id.clone()),
        );

        if self.cfg.icon_path.exists() {
            fs::copy(
                &self.cfg.icon_path,
                app_bundle.join("Contents/Resources/AppIcon.icns"),
            )?;
            obj.insert(
                "CFBundleIconFile".to_string(),
                Value::String("AppIcon".to_string()),
            );
        }

        // Pipe JSON into plutil to produce Info.plist
        let json_bytes = serde_json::to_vec(&info)?;
        let plist_path = self.p.info_plist.to_str().unwrap();
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        fs::write(app_bundle.join("Contents/PkgInfo"), "APPL????")?;

        Ok(app_bundle.clone())
    }

    /// Build bundle only (clean → binary → assemble).
    pub fn bundle(&self) -> Result<()> {
        self.clean()?;
        let binary_path = self.build_binary()?;
        let app_bundle = self.assemble_bundle(&binary_path)?;
        cprintln!(
            "\n<green>Done! App bundle:</green>\n{}",
            app_bundle.display()
        );
        Ok(())
    }

    // ── Distribution steps ────────────────────────────────────────────────────

    pub fn sign(&self) -> Result<()> {
        step("Signing app bundle...");
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let ent_plist = self.p.entitlements_plist.to_str().unwrap();
        let ent_json = self.cfg.entitlements_json_path.to_str().unwrap();

        self.sh
            .run(&["plutil", "-convert", "xml1", "-o", ent_plist, ent_json])?;

        self.sh.run(&[
            "codesign",
            "--force",
            "--options",
            "runtime",
            "--entitlements",
            ent_plist,
            "--sign",
            &self.cfg.sign_identity,
            "--timestamp",
            app_bundle,
        ])?;

        step("Verifying signature...");
        self.sh.run(&[
            "codesign",
            "--verify",
            "--deep",
            "--strict",
            "--verbose=2",
            app_bundle,
        ])?;
        // spctl may return non-zero for unnotarized bundles
        self.sh.try_run(&[
            "spctl",
            "--assess",
            "--verbose=4",
            "--type",
            "exec",
            app_bundle,
        ]);

        Ok(())
    }

    pub fn notarize(&self) -> Result<()> {
        step("Creating zip for notarization...");
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let zip = self.p.zip.to_str().unwrap();

        self.sh
            .run(&["ditto", "-c", "-k", "--keepParent", app_bundle, zip])?;

        step("Stapling notarization ticket...");
        self.sh.run(&["xcrun", "stapler", "staple", app_bundle])?;
        self.sh.run(&["xcrun", "stapler", "validate", app_bundle])?;

        Ok(())
    }

    pub fn package_dmg(&self) -> Result<()> {
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let dmg = self.p.dmg.to_str().unwrap();
        let vol_name = format!("{} {}", self.cfg.app_name, self.cfg.version);
        let timeout_str = self.cfg.notarize_timeout.to_string();

        step("Creating DMG...");
        self.sh.run(&[
            "hdiutil",
            "create",
            "-volname",
            &vol_name,
            "-srcfolder",
            app_bundle,
            "-ov",
            "-format",
            "UDZO",
            dmg,
        ])?;

        self.sh.run(&[
            "codesign",
            "--force",
            "--sign",
            &self.cfg.sign_identity,
            "--timestamp",
            dmg,
        ])?;

        step("Submitting DMG for notarization...");
        self.sh.run(&[
            "xcrun",
            "notarytool",
            "submit",
            dmg,
            "--apple-id",
            &self.cfg.apple_id,
            "--team-id",
            &self.cfg.team_id,
            "--password",
            &self.cfg.apple_password,
            "--wait",
            "--timeout",
            &timeout_str,
        ])?;

        step("Stapling DMG...");
        self.sh.run(&["xcrun", "stapler", "staple", dmg])?;

        Ok(())
    }

    /// Full pipeline: clean → binary → assemble → sign → notarize → DMG.
    pub fn run(&self) -> Result<()> {
        self.clean()?;
        let binary_path = self.build_binary()?;
        self.assemble_bundle(&binary_path)?;
        self.sign()?;
        self.notarize()?;
        self.package_dmg()?;

        println!(
            "\nDone! Distribution artifacts:\n  App bundle: {}\n  DMG:        {}\n  Zip:        {}",
            self.p.app_bundle.display(),
            self.p.dmg.display(),
            self.p.zip.display(),
        );
        Ok(())
    }
}

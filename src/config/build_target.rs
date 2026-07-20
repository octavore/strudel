mod target_ios;
mod target_macos;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

pub use crate::config::build_target::target_ios::{IosProvisioningBackend, IosSection};
pub use crate::config::build_target::target_macos::DmgSection;
use crate::config::extension::ExtensionSection;
use crate::config::icon_section::IconSection;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BuildTarget {
    pub app: AppSection,

    #[serde(default)]
    pub build: BuildSection,

    #[serde(default)]
    pub extensions: Vec<ExtensionSection>,

    #[serde(flatten)]
    pub platform: TargetPlatform,
}

impl BuildTarget {
    pub fn platform(&self) -> Platform {
        match self.platform {
            TargetPlatform::Macos { .. } => Platform::Macos,
            TargetPlatform::Ios { .. } => Platform::Ios,
        }
    }
}

/// AppSection `[app]` contains required application metadata.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AppSection {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: Option<String>,
}

/// BuildSection `[build]` contains inputs and outputs. All optional, with
/// defaults.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct BuildSection {
    pub source_dir: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,

    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: Option<PathBuf>,
    pub icon: Option<IconSection>,

    pub archs: Option<Vec<String>>,

    /// Swift executable target name. Defaults to the app name.
    pub target_name: Option<String>,
    /// Extra environment variables forwarded to `swift build`.
    pub build_env: Option<HashMap<String, String>>,
    /// Dynamic libraries and `.framework` bundles (e.g. Sparkle) to embed in
    /// `Contents/Frameworks` and sign.
    pub embed_libs: Option<Vec<PathBuf>>,
    /// Provisioning profile to embed as `Contents/embedded.provisionprofile`.
    pub provisioning_profile: Option<PathBuf>,

    /// Directory whose contents are merged into `Contents/Resources/`.
    pub resources_dir: Option<PathBuf>,
    /// Individual files or folders to copy into `Contents/Resources/`.
    pub resources: Option<Vec<PathBuf>>,

    /// Arbitrary files or directories copied to a specific destination inside
    /// the bundle, optionally signed (e.g. a helper binary placed outside
    /// `Contents/Resources`/`Contents/Frameworks`).
    pub copy: Option<Vec<CopySection>>,
}

/// One `[[build.copy]]` entry: an arbitrary file or directory copied into the
/// bundle at a caller-chosen destination.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CopySection {
    /// Source file or directory, resolved relative to the config file's
    /// directory. Copied in under its own file name (matching `resources`),
    /// not renamed.
    pub src: PathBuf,
    /// Destination directory relative to the bundle root (e.g.
    /// `"Contents/MacOS"` or `"Contents/Resources/tool"`), created if missing.
    pub dest_dir: String,
    /// Codesign the copied item before the outer bundle is sealed. Directories
    /// are signed with `--deep` (they may contain nested code); files are
    /// signed directly. Needed for executables or nested bundles placed
    /// outside `embed_libs`/`resources`.
    #[serde(default)]
    pub sign: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum TargetPlatform {
    Macos {
        dmg: Option<DmgSection>,
        /// Path to a `.xcassets` directory to compile into
        /// `Contents/Resources/Assets.car` with `xcrun actool`.
        assets_dir: Option<PathBuf>,
    },
    Ios {
        #[serde(default)]
        ios: IosSection,
    },
}

/// The target platform for a build target.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Ios,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Platform::Macos => "macOS",
            Platform::Ios => "iOS",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Ios => "ios",
        }
    }
}

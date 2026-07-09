use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::resolved::ResolvedIcon;
use crate::config::utils::resolve_to;

/// `[build.icon]`: either a png or icns file copied into the bundle as-is,
/// or generate an icon from a png or svg source image at build time.
/// Untagged, since the two forms are distinguished by their field names
/// (`path` vs `src`) rather than an explicit tag:
///
/// ```toml
/// icon.path = "AppIcon.icns" # or "AppIcon.png"
/// ```
/// ```toml
/// [build.icon]
/// src = "art.png"
/// scale = 1.2               # optional
/// background = "#fefefe"  # optional
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields, untagged)]
pub enum IconSection {
    Path {
        /// File copied into the bundle. Supports png and icns.
        /// Set `icns = true` to convert a png into a multi-resolution `.icns`
        path: PathBuf,
        icns: Option<bool>,
    },
    Generated {
        /// Source image (png/svg) to composite into an icon.
        src: PathBuf,

        /// Multiplier to scale the source image.
        scale: Option<f32>,

        /// Icon background color, in hex. Default: white.
        background: Option<String>,

        /// Set `icns = true` to convert the generated icon into a
        /// multi-resolution `.icns`.
        icns: Option<bool>,
    },
}

impl IconSection {
    pub fn resolve(self, config_dir: &Path) -> ResolvedIcon {
        match self {
            IconSection::Path { path, icns } => ResolvedIcon::Path {
                path: resolve_to(config_dir, path),
                icns: icns.unwrap_or(false),
            },
            IconSection::Generated {
                src,
                scale,
                background,
                icns,
            } => ResolvedIcon::Generated {
                src: resolve_to(config_dir, src),
                scale: scale.unwrap_or(1.0),
                background,
                icns: icns.unwrap_or(false),
            },
        }
    }
}

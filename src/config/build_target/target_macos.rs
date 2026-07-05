use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::resolved::ResolvedDmg;
use crate::config::utils::resolve_to;

/// `[dmg]` — DMG window customization for `strudel release`.
///
/// The styled Finder window (a generated `.DS_Store`, applied headlessly by the
/// `dmg` crate) is the default even when this section is absent. Add the
/// section to override individual fields or opt out with `plain = true`.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DmgSection {
    /// Set to `true` to skip the styled window and produce a plain compressed
    /// DMG directly (no Finder window configuration). Default: `false`.
    #[serde(default)]
    pub plain: bool,
    /// Path to a PNG or JPEG background image for the DMG Finder window.
    pub background: Option<PathBuf>,
    /// Finder window width in pixels. Default: 660.
    pub window_width: Option<u32>,
    /// Finder window height in pixels. Default: 400.
    pub window_height: Option<u32>,
    /// Icon size in pixels. Default: 128.
    pub icon_size: Option<u32>,
    /// Horizontal position of the .app icon. Default: 192.
    pub app_x: Option<u32>,
    /// Vertical position of the .app icon. Default: 192.
    pub app_y: Option<u32>,
    /// Horizontal position of the Applications symlink. Default: 468.
    pub applications_x: Option<u32>,
    /// Vertical position of the Applications symlink. Default: 192.
    pub applications_y: Option<u32>,
}

impl DmgSection {
    /// Returns `None` when `plain = true` (simple UDZO path); otherwise
    /// returns the resolved config with defaults filled in.
    pub fn resolve(self, config_dir: &Path) -> Option<ResolvedDmg> {
        if self.plain {
            return None;
        }
        Some(ResolvedDmg {
            background: self.background.map(|p| resolve_to(config_dir, p)),
            window_width: self.window_width.unwrap_or(660),
            window_height: self.window_height.unwrap_or(400),
            icon_size: self.icon_size.unwrap_or(128),
            app_x: self.app_x.unwrap_or(192),
            app_y: self.app_y.unwrap_or(192),
            applications_x: self.applications_x.unwrap_or(468),
            applications_y: self.applications_y.unwrap_or(192),
        })
    }
}

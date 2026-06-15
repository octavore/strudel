use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::ResolvedExtension;
use crate::config::utils::resolve_to;

/// The kind of app extension after resolution. A flat discriminator used by
/// downstream build steps; the kind-specific data has already been unpacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionKind {
    SafariWebExtension,
    AppExtension,
}

/// One entry per app extension embedded in the host bundle. In toml, this is
/// `[[extensions]]`. Fields shared by all extension kinds live at the top
/// level; kind-specific fields are carried by the flattened, `kind`-tagged
/// [`ExtensionKindConfig`].
// note: not deny_unknown_fields because enum is internally tagged and deny_unknown_fields would
// reject the tag
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ExtensionSection {
    /// Swift executable target name in `Package.swift`. The compiled binary
    /// is placed at `<name>.appex/Contents/MacOS/<target_name>`.
    pub target_name: String,

    /// `CFBundleIdentifier` of the extension — typically a child of the host
    /// app's bundle id (e.g. `com.example.myapp.Extension`).
    pub bundle_id: String,

    /// Display/bundle name. Defaults to `target_name`. Used for both the
    /// `.appex` directory name and `CFBundleName` / `CFBundleDisplayName`.
    pub name: Option<String>,

    /// JSON describing extra `Info.plist` keys for the extension. strudel
    /// always injects `CFBundle*` identity keys and the kind-specific
    /// `NSExtension` dict on top of this. Optional; defaults to an empty
    /// object.
    pub info_json_path: Option<PathBuf>,

    /// JSON entitlements for the extension — required, since extensions are
    /// sandboxed independently of the host app.
    pub entitlements_json_path: Option<PathBuf>,

    /// Discriminator (`kind = "..."`) plus the kind-specific fields. Internally
    /// tagged so the variant's fields sit flat alongside the common ones.
    #[serde(flatten)]
    pub kind: ExtensionKindConfig,
}

impl ExtensionSection {
    /// Resolve a parsed extension entry against the config directory,
    /// applying defaults and validating kind-specific required
    /// fields.
    pub fn resolve(self, config_dir: &Path) -> Result<ResolvedExtension> {
        let ExtensionSection {
            target_name,
            bundle_id,
            name,
            info_json_path,
            entitlements_json_path,
            kind,
        } = self;

        let resolved_name = name.unwrap_or_else(|| target_name.clone());

        let resolve = |p: PathBuf| resolve_to(config_dir, p);
        let entitlements_json_path = entitlements_json_path.map(resolve).with_context(|| {
            format!(
                "extension `{target_name}` is missing required field `entitlements_json_path` \
             (extensions are sandboxed independently of the host app)"
            )
        })?;

        let info_json_path = info_json_path.map(resolve);

        let (kind, resources_dir, principal_class, extension_point_identifier) = match kind {
            ExtensionKindConfig::SafariWebExtension {
                resources_dir,
                principal_class,
            } => {
                let principal_class = principal_class
                    .unwrap_or_else(|| format!("{target_name}.SafariWebExtensionHandler"));
                (
                    ExtensionKind::SafariWebExtension,
                    Some(resolve(resources_dir)),
                    Some(principal_class),
                    None,
                )
            },
            ExtensionKindConfig::AppExtension {
                extension_point_identifier,
                principal_class,
            } => (
                ExtensionKind::AppExtension,
                None,
                principal_class,
                Some(extension_point_identifier),
            ),
        };

        Ok(ResolvedExtension {
            kind,
            target_name,
            bundle_id,
            name: resolved_name,
            info_json_path,
            entitlements_json_path,
            resources_dir,
            principal_class,
            extension_point_identifier,
        })
    }
}

/// Kind-specific deserialized fields for an [`ExtensionSection`]. Internally
/// tagged on `kind`: each variant's fields appear at the same level as the
/// common extension fields in TOML.
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionKindConfig {
    SafariWebExtension {
        /// Directory whose contents (manifest.json, JS, HTML, icons, …) are
        /// copied wholesale into `<name>.appex/Contents/Resources/`.
        resources_dir: PathBuf,
        /// `NSExtensionPrincipalClass`. Defaults to
        /// `"<target_name>.SafariWebExtensionHandler"` — the Apple Xcode
        /// template convention.
        principal_class: Option<String>,
    },
    AppExtension {
        /// `NSExtensionPointIdentifier` — identifies the extension point this
        /// extension targets (e.g. `"com.apple.share-services"` for a Share
        /// Extension, `"com.apple.FinderSync"` for a Finder Sync Extension).
        extension_point_identifier: String,
        /// `NSExtensionPrincipalClass`. Optional — some extension points
        /// require it, others do not.
        principal_class: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indoc::indoc;

    use super::*;
    use crate::config::fixtures::*;

    #[test]
    fn parses_and_resolves_safari_web_extension() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [[extensions]]
            kind = "safari_web_extension"
            target_name = "MyAppExtension"
            bundle_id = "com.example.myapp.Extension"
            resources_dir = "ext/dist"
            entitlements_json_path = "ext/entitlements.json"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(r.extensions.len(), 1);
        let ext = &r.extensions[0];
        assert_eq!(ext.kind, ExtensionKind::SafariWebExtension);
        assert_eq!(ext.target_name, "MyAppExtension");
        // name defaults to target_name
        assert_eq!(ext.name, "MyAppExtension");
        assert_eq!(ext.bundle_id, "com.example.myapp.Extension");
        assert_eq!(ext.resources_dir, Some(PathBuf::from("/cfg/ext/dist")));
        assert_eq!(
            ext.entitlements_json_path,
            PathBuf::from("/cfg/ext/entitlements.json")
        );
        // principal_class defaults to "<target_name>.SafariWebExtensionHandler"
        assert_eq!(
            ext.principal_class.as_deref(),
            Some("MyAppExtension.SafariWebExtensionHandler")
        );
    }

    #[test]
    fn safari_web_extension_requires_resources_dir() {
        let err = parse_build_config(indoc! {r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#})
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("resources_dir"), "got: {msg}");
    }

    #[test]
    fn extension_requires_entitlements() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            resources_dir = "ext"
        "#})
        .unwrap();
        let err = cfg.resolve(Path::new("/cfg"), None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("entitlements_json_path"), "got: {msg}");
    }

    #[test]
    fn extension_unknown_field_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            resources_dir = "ext"
            entitlements_json_path = "e.json"
            unknown_field = "boom"
        "#});
        assert!(err.is_err(), "typo'd extension key should be rejected");
    }

    #[test]
    fn unknown_extension_kind_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "share_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#});
        assert!(err.is_err(), "unknown extension kind should be rejected");
    }

    #[test]
    fn parses_and_resolves_app_extension() {
        let cfg = parse_build_config(indoc! {r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [[extensions]]
            kind = "app_extension"
            target_name = "MyShareExtension"
            bundle_id = "com.example.myapp.Share"
            extension_point_identifier = "com.apple.share-services"
            entitlements_json_path = "share/entitlements.json"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(r.extensions.len(), 1);
        let ext = &r.extensions[0];
        assert_eq!(ext.kind, ExtensionKind::AppExtension);
        assert_eq!(ext.target_name, "MyShareExtension");
        assert_eq!(ext.name, "MyShareExtension");
        assert_eq!(ext.bundle_id, "com.example.myapp.Share");
        assert_eq!(
            ext.extension_point_identifier.as_deref(),
            Some("com.apple.share-services")
        );
        assert!(ext.principal_class.is_none());
        assert!(ext.resources_dir.is_none());
        assert_eq!(
            ext.entitlements_json_path,
            PathBuf::from("/cfg/share/entitlements.json")
        );
    }

    #[test]
    fn app_extension_accepts_optional_principal_class() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "app_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            extension_point_identifier = "com.apple.FinderSync"
            principal_class = "Ext.FinderSyncController"
            entitlements_json_path = "e.json"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ext = &r.extensions[0];
        assert_eq!(
            ext.principal_class.as_deref(),
            Some("Ext.FinderSyncController")
        );
    }

    #[test]
    fn app_extension_requires_extension_point_identifier() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[extensions]]
            kind = "app_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#})
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("extension_point_identifier"), "got: {msg}");
    }
}

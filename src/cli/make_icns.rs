use std::path::PathBuf;

use anyhow::Result;

use crate::icns;

/// Make an .icns file from a PNG image. The PNG should be at least 1024x1024,
/// and the resulting .icns file will contain all the required icon sizes for
/// macOS.
#[derive(clap::Args)]
pub(crate) struct MakeIcnsCmd {
    /// Source PNG path (should be at least 1024x1024)
    png: PathBuf,

    /// Destination .icns path
    icns: PathBuf,
}

impl MakeIcnsCmd {
    pub(crate) fn execute(self) -> Result<()> {
        icns::make_icns(&self.png, &self.icns, false)
    }
}

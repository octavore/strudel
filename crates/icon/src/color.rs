use image::Rgba;

use crate::IconError;

/// Parses an opaque `#RRGGBB` hex string into a color.
pub fn parse_hex_color(hex: &str) -> Result<Rgba<u8>, IconError> {
    let stripped = hex.trim_start_matches('#');
    let channel =
        |s: &str| u8::from_str_radix(s, 16).map_err(|_| IconError::InvalidColor(hex.to_string()));
    match stripped.len() {
        6 => Ok(Rgba([
            channel(&stripped[0..2])?,
            channel(&stripped[2..4])?,
            channel(&stripped[4..6])?,
            255,
        ])),
        _ => Err(IconError::InvalidColor(hex.to_string())),
    }
}

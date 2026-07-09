use image::Rgba;

use crate::IconError;

/// Parses an opaque `#RRGGBB` hex string into a color. Shorthand (`#RGB`) and
/// alpha (`#RRGGBBAA`) are not accepted.
pub fn parse_hex_color(hex: &str) -> Result<Rgba<u8>, IconError> {
    let stripped = hex.trim_start_matches('#');
    // Byte-index the digits below, so reject anything that isn't six ASCII hex
    // digits up front: a multi-byte char would otherwise make `stripped[0..2]`
    // split a char boundary and panic. This also rejects the signs and
    // underscores `u8::from_str_radix` would otherwise accept.
    if stripped.len() != 6 || !stripped.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(IconError::InvalidColor(hex.to_string()));
    }
    let channel =
        |s: &str| u8::from_str_radix(s, 16).map_err(|_| IconError::InvalidColor(hex.to_string()));
    Ok(Rgba([
        channel(&stripped[0..2])?,
        channel(&stripped[2..4])?,
        channel(&stripped[4..6])?,
        255,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex_with_and_without_hash() {
        assert_eq!(parse_hex_color("#1a2B3c").unwrap(), Rgba([26, 43, 60, 255]));
        assert_eq!(parse_hex_color("1a2b3c").unwrap(), Rgba([26, 43, 60, 255]));
        assert_eq!(parse_hex_color("#ffffff").unwrap(), Rgba([255; 4]));
        assert_eq!(parse_hex_color("#000000").unwrap(), Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn wrong_length_is_rejected() {
        // Shorthand and alpha forms are documented as unsupported, so they must
        // report the color as invalid rather than silently picking 6 of the digits.
        for hex in ["#fff", "#ffffffff", "#fffff", "", "#"] {
            assert!(parse_hex_color(hex).is_err(), "{hex:?} should be rejected");
        }
    }

    #[test]
    fn non_hex_digits_are_rejected() {
        // "+1" and "1_" parse as integers under from_str_radix; a color must not.
        for hex in ["#gggggg", "#+1+1+1", "#1_1_1_", "#12 456"] {
            assert!(parse_hex_color(hex).is_err(), "{hex:?} should be rejected");
        }
    }

    #[test]
    fn multibyte_char_is_rejected_and_does_not_panic() {
        // "aÿbcd" is six bytes but byte 2 is inside the two-byte 'ÿ', so a
        // naive `stripped[0..2]` would panic instead of erroring. Reachable
        // from `[build.icon] background` in a user's strudel.toml.
        assert_eq!("aÿbcd".len(), 6);
        assert!(parse_hex_color("#aÿbcd").is_err());
        assert!(parse_hex_color("#日本語").is_err());
    }

    #[test]
    fn error_names_the_original_input() {
        let err = parse_hex_color("#nope").unwrap_err();
        assert!(format!("{err}").contains("#nope"), "got: {err}");
    }
}

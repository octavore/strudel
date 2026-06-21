//! SRP-6a primitives for Apple's GrandSlam authentication. Apple uses the
//! RFC 5054 2048-bit group with SHA-256 and a non-standard password pre-hash
//! (s2k / s2k_fo).

use std::io::Cursor;

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use num_bigint::BigUint;
use pbkdf2::pbkdf2_hmac;
use sha2::{Digest, Sha256};

// https://datatracker.ietf.org/doc/html/rfc5054#appendix-A
// RFC 5054 2048-bit group. Apple's SRP-6a uses SHA-256 instead of SHA-1.
const SRP_N_HEX: &str = concat!(
    "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC319294",
    "3DB56050A37329CBB4A099ED8193E0757767A13DD52312AB4B03310D",
    "CD7F48A9DA04FD50E8083969EDB767B0CF6095179A163AB3661A05FB",
    "D5FAAAE82918A9962F0B93B855F97993EC975EEAA80D740ADBF4FF74",
    "7359D041D5C33EA71D281E446B14773BCA97B43A23FB801676BD207A",
    "436C6481F1D2B9078717461A5B9D32E688F87748544523B524B0D57D",
    "5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6AF874E73",
    "03CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6",
    "94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F",
    "9E4AFF73",
);

type HmacSha256 = Hmac<Sha256>;

pub(super) fn srp_n() -> BigUint {
    BigUint::parse_bytes(SRP_N_HEX.as_bytes(), 16).expect("constant SRP_N_HEX is valid hex")
}

pub(super) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

pub(super) fn pad256(n: &BigUint) -> [u8; 256] {
    let bytes = n.to_bytes_be();
    let mut out = [0u8; 256];
    let start = 256usize.saturating_sub(bytes.len());
    out[start..].copy_from_slice(&bytes[bytes.len().saturating_sub(256)..]);
    out
}

// Derive the SRP x value. Apple's password pre-hashing differs from standard
// SRP. First compute the SRP "password" p via Apple's s2k scheme:
//   s2k:    p = PBKDF2-HMAC-SHA256(SHA256(password), salt, i, 32)
//   s2k_fo: p = PBKDF2-HMAC-SHA256(hex_upper(SHA256(password)), salt, i, 32)
// Then x = SHA256(salt || SHA256(b":" || p)).
pub(super) fn derive_x(password: &str, salt: &[u8], iterations: u32, method: &str) -> [u8; 32] {
    let pw_hash = sha256(password.as_bytes());
    let pw_input: Vec<u8> = if method == "s2k_fo" {
        pw_hash
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
            .into_bytes()
    } else {
        pw_hash.to_vec()
    };
    let mut p = [0u8; 32];
    pbkdf2_hmac::<Sha256>(&pw_input, salt, iterations, &mut p);

    let mut inner = Vec::with_capacity(1 + p.len());
    inner.push(b':');
    inner.extend_from_slice(&p);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(salt.len() + inner_hash.len());
    outer.extend_from_slice(salt);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

// M1 = H(H(N) XOR H(g) || H(username) || salt || A || B || K)
pub(super) fn compute_m1(
    n: &BigUint,
    username: &str,
    salt: &[u8],
    a_pub: &BigUint,
    b_pub: &BigUint,
    k_srp: &[u8; 32],
) -> [u8; 32] {
    let n_padded = pad256(n);

    let h_n = sha256(&n_padded);
    // H(g) = SHA256(g padded to 256 bytes)
    let mut g_padded = [0u8; 256];
    g_padded[255] = 2;
    let h_g = sha256(&g_padded);
    let mut xor_ng = [0u8; 32];
    for i in 0..32 {
        xor_ng[i] = h_n[i] ^ h_g[i];
    }

    let a_bytes = a_pub.to_bytes_be();
    let b_bytes = b_pub.to_bytes_be();
    let mut input = Vec::with_capacity(32 + 32 + salt.len() + a_bytes.len() + b_bytes.len() + 32);
    input.extend_from_slice(&xor_ng);
    input.extend_from_slice(&sha256(username.as_bytes()));
    input.extend_from_slice(salt);
    input.extend_from_slice(&a_bytes);
    input.extend_from_slice(&b_bytes);
    input.extend_from_slice(k_srp);
    sha256(&input)
}

// Decrypt the `spd` session data blob from the GSA complete response.
//
// The AES key and IV are both derived from the SRP session key K:
//   key = HMAC-SHA256(K, "extra data key:")
//   iv  = HMAC-SHA256(K, "extra data iv:")[..16]
// The entire spd blob is the ciphertext (no IV prefix).
pub(super) fn decrypt_spd(spd_bytes: &[u8], k_srp: &[u8; 32]) -> Result<plist::Value> {
    use aes::Aes256;
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockModeDecrypt, KeyIvInit};

    type Aes256CbcDec = cbc::Decryptor<Aes256>;

    let session_key = hmac_sha256(k_srp, b"extra data key:");
    let iv_full = hmac_sha256(k_srp, b"extra data iv:");
    let iv: &[u8; 16] = iv_full[..16].try_into().unwrap();

    let plaintext = Aes256CbcDec::new((&session_key).into(), iv.into())
        .decrypt_padded_vec::<Pkcs7>(spd_bytes)
        .context("AES-CBC decryption of spd failed")?;

    plist::Value::from_reader(Cursor::new(plaintext)).context("spd plaintext is not a valid plist")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srp_n_parses_to_2048_bits() {
        let n = srp_n();
        assert_eq!(n.bits(), 2048);
    }

    #[test]
    fn pad256_short_value() {
        let n = BigUint::from(1u32);
        let padded = pad256(&n);
        assert_eq!(padded[..255], [0u8; 255]);
        assert_eq!(padded[255], 1);
    }

    #[test]
    fn pad256_n_starts_with_ac() {
        // The RFC 5054 2048-bit group N begins with AC6BDB41...
        let n = srp_n();
        let padded = pad256(&n);
        assert_eq!(padded[0], 0xAC);
    }

    #[test]
    fn derive_x_produces_32_bytes() {
        let salt = [0u8; 32];
        let result = derive_x("password", &salt, 1, "s2k");
        assert_eq!(result.len(), 32);
        assert!(result.iter().any(|&b| b != 0));
    }

    #[test]
    fn derive_x_s2k_vs_s2k_fo_differ() {
        let salt = [1u8; 32];
        let r1 = derive_x("testpassword", &salt, 1000, "s2k");
        let r2 = derive_x("testpassword", &salt, 1000, "s2k_fo");
        assert_ne!(r1, r2);
    }

    #[test]
    fn m1_computation_is_deterministic() {
        let n = srp_n();
        let a_pub = BigUint::from(42u32);
        let b_pub = BigUint::from(99u32);
        let k = [7u8; 32];
        let m1a = compute_m1(&n, "user@example.com", b"saltsalt", &a_pub, &b_pub, &k);
        let m1b = compute_m1(&n, "user@example.com", b"saltsalt", &a_pub, &b_pub, &k);
        assert_eq!(m1a, m1b);
    }
}

//! Small cryptographic helpers shared across modules.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Cryptographically secure random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("the OS random number generator failed");
    bytes
}

fn keyed(key: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        mac.update(part);
    }
    mac
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    keyed(key, parts).finalize().into_bytes().into()
}

/// Constant-time check of an HMAC-SHA256 tag.
pub fn hmac_matches(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    keyed(key, parts).verify_slice(tag).is_ok()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_verifies() {
        let tag = hmac(b"key", &[b"a", b"b"]);
        assert!(hmac_matches(b"key", &[b"ab"], &tag));
        assert!(!hmac_matches(b"other", &[b"ab"], &tag));
        assert_ne!(random::<16>(), random::<16>());
    }
}

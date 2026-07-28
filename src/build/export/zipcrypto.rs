//! The legacy PKWARE ("ZipCrypto") stream cipher.
//!
//! BrickLink Studio's `.io` files are ZIP archives encrypted with this cipher
//! and the fixed password `soho0909`. The `zip` crate can *read* ZipCrypto but
//! cannot write it, so the writer is hand-rolled here.
//!
//! Note this is 1990s obfuscation, not security: the password is public and
//! the cipher is trivially broken. It is implemented only because Studio's
//! file format requires it.
//!
//! The CRC-32 table is generated here rather than taken from `crc32fast`
//! because the cipher needs the raw, non-inverted per-byte update
//! (`crc = (crc >> 8) ^ TABLE[(crc ^ b) & 0xff]`), which that crate's hasher
//! does not expose. The same table serves the archive's entry checksums.

#![allow(clippy::doc_markdown, clippy::unreadable_literal)]

/// The password BrickLink Studio uses for every `.io` file.
pub const STUDIO_PASSWORD: &[u8] = b"soho0909";

/// The standard CRC-32 (IEEE) lookup table.
const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = build_table();

/// One byte of the raw, non-inverted CRC-32 update.
#[must_use]
pub const fn crc32_byte(crc: u32, byte: u8) -> u32 {
    (crc >> 8) ^ TABLE[((crc ^ byte as u32) & 0xff) as usize]
}

/// The standard CRC-32 of a buffer, with the usual pre- and post-inversion.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;
    for byte in data {
        crc = crc32_byte(crc, *byte);
    }
    crc ^ 0xFFFF_FFFF
}

/// The cipher's three-word key state.
#[derive(Debug, Clone, Copy)]
pub struct Keys {
    k0: u32,
    k1: u32,
    k2: u32,
}

impl Keys {
    /// Initialises the keys from a password.
    #[must_use]
    pub fn new(password: &[u8]) -> Self {
        let mut keys = Self {
            k0: 0x1234_5678,
            k1: 0x2345_6789,
            k2: 0x3456_7890,
        };
        for byte in password {
            keys.update(*byte);
        }
        keys
    }

    /// Mixes one plaintext byte into the key state.
    const fn update(&mut self, byte: u8) {
        self.k0 = crc32_byte(self.k0, byte);
        self.k1 = self
            .k1
            .wrapping_add(self.k0 & 0xff)
            .wrapping_mul(0x0808_8405)
            .wrapping_add(1);
        #[allow(clippy::cast_possible_truncation)]
        let high = (self.k1 >> 24) as u8;
        self.k2 = crc32_byte(self.k2, high);
    }

    /// The next keystream byte.
    #[allow(clippy::cast_possible_truncation)]
    const fn stream_byte(&self) -> u8 {
        let temp = (self.k2 | 2) as u16;
        ((temp.wrapping_mul(temp ^ 1)) >> 8) as u8
    }

    /// Encrypts one plaintext byte.
    pub const fn encrypt_byte(&mut self, plain: u8) -> u8 {
        let cipher = plain ^ self.stream_byte();
        self.update(plain);
        cipher
    }

    /// Decrypts one ciphertext byte.
    pub const fn decrypt_byte(&mut self, cipher: u8) -> u8 {
        let plain = cipher ^ self.stream_byte();
        self.update(plain);
        plain
    }
}

/// Encrypts a buffer, prepending the 12-byte header ZIP requires.
///
/// The header's last byte carries the high byte of the entry's CRC, which is
/// how a reader checks the password before decompressing. The filler is
/// derived deterministically from the plaintext rather than randomly, so an
/// export of the same model is byte-for-byte reproducible and can be
/// golden-tested.
#[must_use]
pub fn encrypt(plaintext: &[u8], password: &[u8], crc: u32) -> Vec<u8> {
    let mut keys = Keys::new(password);
    let mut out = Vec::with_capacity(plaintext.len() + 12);

    let mut header = [0u8; 12];
    // A tiny deterministic PRNG seeded from the content, so the filler varies
    // between entries but never between runs.
    let mut seed = crc ^ 0x5DEE_CE66;
    for slot in header.iter_mut().take(11) {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        #[allow(clippy::cast_possible_truncation)]
        {
            *slot = (seed >> 16) as u8;
        }
    }
    // Readers check this byte against the entry CRC.
    #[allow(clippy::cast_possible_truncation)]
    {
        header[11] = (crc >> 24) as u8;
    }

    for byte in header {
        out.push(keys.encrypt_byte(byte));
    }
    for byte in plaintext {
        out.push(keys.encrypt_byte(*byte));
    }
    out
}

/// Decrypts a buffer produced by [`encrypt`], dropping the 12-byte header.
///
/// Only used by tests; readers in the wild are the `zip` crate and Studio.
#[must_use]
pub fn decrypt(ciphertext: &[u8], password: &[u8]) -> Vec<u8> {
    let mut keys = Keys::new(password);
    ciphertext
        .iter()
        .map(|byte| keys.decrypt_byte(*byte))
        .skip(12)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_vector() {
        // The check value every CRC-32/IEEE implementation agrees on.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc32_is_sensitive_to_every_byte() {
        assert_ne!(crc32(b"hello"), crc32(b"hellp"));
        assert_ne!(crc32(b"hello"), crc32(b"olleh"));
    }

    #[test]
    fn keys_depend_on_the_password() {
        let a = Keys::new(b"soho0909");
        let b = Keys::new(b"different");
        assert_ne!((a.k0, a.k1, a.k2), (b.k0, b.k1, b.k2));
    }

    #[test]
    fn encrypting_then_decrypting_round_trips() {
        let plaintext = b"0 FILE model.ldr\n1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat\n";
        let crc = crc32(plaintext);

        let cipher = encrypt(plaintext, STUDIO_PASSWORD, crc);
        assert_eq!(cipher.len(), plaintext.len() + 12);
        assert_ne!(&cipher[12..], plaintext.as_slice(), "it must actually encrypt");

        assert_eq!(decrypt(&cipher, STUDIO_PASSWORD), plaintext);
    }

    #[test]
    fn the_wrong_password_does_not_recover_the_plaintext() {
        let plaintext = b"secret contents";
        let cipher = encrypt(plaintext, STUDIO_PASSWORD, crc32(plaintext));
        assert_ne!(decrypt(&cipher, b"wrong"), plaintext);
    }

    #[test]
    fn the_header_carries_the_crc_check_byte() {
        let plaintext = b"anything";
        let crc = crc32(plaintext);
        let cipher = encrypt(plaintext, STUDIO_PASSWORD, crc);

        // Decrypt just the header and inspect its last byte.
        let mut keys = Keys::new(STUDIO_PASSWORD);
        let header: Vec<u8> = cipher[..12]
            .iter()
            .map(|byte| keys.decrypt_byte(*byte))
            .collect();
        #[allow(clippy::cast_possible_truncation)]
        let expected = (crc >> 24) as u8;
        assert_eq!(header[11], expected);
    }

    #[test]
    fn encryption_is_reproducible() {
        let plaintext = b"the same model twice";
        let crc = crc32(plaintext);
        assert_eq!(
            encrypt(plaintext, STUDIO_PASSWORD, crc),
            encrypt(plaintext, STUDIO_PASSWORD, crc),
            "exports must be byte-for-byte stable"
        );
    }

    #[test]
    fn an_empty_payload_still_produces_a_header() {
        let cipher = encrypt(b"", STUDIO_PASSWORD, crc32(b""));
        assert_eq!(cipher.len(), 12);
        assert!(decrypt(&cipher, STUDIO_PASSWORD).is_empty());
    }
}

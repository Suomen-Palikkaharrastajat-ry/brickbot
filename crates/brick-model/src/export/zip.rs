//! A minimal ZIP writer that can produce ZipCrypto-encrypted entries.
//!
//! The `zip` crate reads such archives but cannot write them, and Studio's
//! `.io` format requires them, so the handful of records we need are written
//! by hand.
//!
//! Every entry is **stored**, not deflated. An LDraw file for five hundred
//! bricks is around 40 KB, so compression buys nothing worth the extra
//! surface, and storing keeps the writer to header arithmetic.

#![allow(clippy::doc_markdown)]

use super::zipcrypto::{crc32, encrypt};

const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

/// Version 2.0, which is what stored-and-encrypted entries require.
const VERSION: u16 = 20;
/// General-purpose bit 0: the entry is encrypted.
const FLAG_ENCRYPTED: u16 = 0x0001;
/// Compression method 0: stored.
const METHOD_STORE: u16 = 0;

/// One file to put in the archive.
pub struct Entry {
    pub name: String,
    pub contents: Vec<u8>,
}

impl Entry {
    pub fn new(name: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            contents: contents.into(),
        }
    }
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Builds an archive whose entries are all ZipCrypto-encrypted with
/// `password`.
///
/// Timestamps are fixed rather than taken from the clock, so exporting the
/// same model twice produces identical bytes.
#[must_use]
pub fn write_encrypted_archive(entries: &[Entry], password: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // (name, crc, stored size, uncompressed size, local header offset)
    let mut directory: Vec<(String, u32, u32, u32, u32)> = Vec::new();

    for entry in entries {
        #[allow(clippy::cast_possible_truncation)]
        let offset = out.len() as u32;
        let crc = crc32(&entry.contents);
        let payload = encrypt(&entry.contents, password, crc);

        #[allow(clippy::cast_possible_truncation)]
        let stored_size = payload.len() as u32;
        #[allow(clippy::cast_possible_truncation)]
        let plain_size = entry.contents.len() as u32;
        #[allow(clippy::cast_possible_truncation)]
        let name_len = entry.name.len() as u16;

        push_u32(&mut out, LOCAL_HEADER_SIG);
        push_u16(&mut out, VERSION);
        push_u16(&mut out, FLAG_ENCRYPTED);
        push_u16(&mut out, METHOD_STORE);
        push_u16(&mut out, 0); // modification time
        push_u16(&mut out, 0); // modification date
        push_u32(&mut out, crc);
        push_u32(&mut out, stored_size);
        push_u32(&mut out, plain_size);
        push_u16(&mut out, name_len);
        push_u16(&mut out, 0); // extra field length
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&payload);

        directory.push((entry.name.clone(), crc, stored_size, plain_size, offset));
    }

    #[allow(clippy::cast_possible_truncation)]
    let directory_offset = out.len() as u32;
    for (name, crc, stored_size, plain_size, offset) in &directory {
        #[allow(clippy::cast_possible_truncation)]
        let name_len = name.len() as u16;

        push_u32(&mut out, CENTRAL_HEADER_SIG);
        push_u16(&mut out, VERSION); // version made by
        push_u16(&mut out, VERSION); // version needed
        push_u16(&mut out, FLAG_ENCRYPTED);
        push_u16(&mut out, METHOD_STORE);
        push_u16(&mut out, 0); // modification time
        push_u16(&mut out, 0); // modification date
        push_u32(&mut out, *crc);
        push_u32(&mut out, *stored_size);
        push_u32(&mut out, *plain_size);
        push_u16(&mut out, name_len);
        push_u16(&mut out, 0); // extra field length
        push_u16(&mut out, 0); // comment length
        push_u16(&mut out, 0); // disk number
        push_u16(&mut out, 0); // internal attributes
        push_u32(&mut out, 0); // external attributes
        push_u32(&mut out, *offset);
        out.extend_from_slice(name.as_bytes());
    }

    #[allow(clippy::cast_possible_truncation)]
    let directory_size = out.len() as u32 - directory_offset;
    #[allow(clippy::cast_possible_truncation)]
    let count = directory.len() as u16;

    push_u32(&mut out, EOCD_SIG);
    push_u16(&mut out, 0); // this disk
    push_u16(&mut out, 0); // disk with the central directory
    push_u16(&mut out, count);
    push_u16(&mut out, count);
    push_u32(&mut out, directory_size);
    push_u32(&mut out, directory_offset);
    push_u16(&mut out, 0); // comment length

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::zipcrypto::STUDIO_PASSWORD;
    use std::io::Read;

    fn sample() -> Vec<Entry> {
        vec![
            Entry::new("model.ldr", "1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat\n"),
            Entry::new(".INFO", "Studio 2.0\n"),
        ]
    }

    #[test]
    fn writes_the_expected_signatures() {
        let archive = write_encrypted_archive(&sample(), STUDIO_PASSWORD);
        assert_eq!(
            u32::from_le_bytes(archive[0..4].try_into().unwrap()),
            LOCAL_HEADER_SIG
        );
        // The end-of-central-directory record is the last 22 bytes.
        let eocd = archive.len() - 22;
        assert_eq!(
            u32::from_le_bytes(archive[eocd..eocd + 4].try_into().unwrap()),
            EOCD_SIG
        );
        assert_eq!(
            u16::from_le_bytes(archive[eocd + 8..eocd + 10].try_into().unwrap()),
            2,
            "two entries"
        );
    }

    #[test]
    fn marks_entries_as_encrypted_and_stored() {
        let archive = write_encrypted_archive(&sample(), STUDIO_PASSWORD);
        let flags = u16::from_le_bytes(archive[6..8].try_into().unwrap());
        let method = u16::from_le_bytes(archive[8..10].try_into().unwrap());
        assert_eq!(flags & FLAG_ENCRYPTED, FLAG_ENCRYPTED);
        assert_eq!(method, METHOD_STORE);
    }

    #[test]
    fn the_stored_size_includes_the_encryption_header() {
        let entries = sample();
        let archive = write_encrypted_archive(&entries, STUDIO_PASSWORD);
        let stored = u32::from_le_bytes(archive[18..22].try_into().unwrap()) as usize;
        let plain = u32::from_le_bytes(archive[22..26].try_into().unwrap()) as usize;
        assert_eq!(plain, entries[0].contents.len());
        assert_eq!(stored, plain + 12);
    }

    #[test]
    fn the_archive_can_be_read_back_by_the_zip_crate() {
        // The important test: our bytes are checked against an independent
        // implementation, not just against our own reader.
        let entries = sample();
        let archive = write_encrypted_archive(&entries, STUDIO_PASSWORD);

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        assert_eq!(zip.len(), 2);

        let mut recovered = String::new();
        zip.by_name_decrypt("model.ldr", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut recovered)
            .unwrap();
        assert_eq!(recovered, "1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat\n");

        let mut info = String::new();
        zip.by_name_decrypt(".INFO", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut info)
            .unwrap();
        assert_eq!(info, "Studio 2.0\n");
    }

    #[test]
    fn a_large_entry_round_trips() {
        let big = "1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat\n".repeat(2000);
        let archive =
            write_encrypted_archive(&[Entry::new("model.ldr", big.clone())], STUDIO_PASSWORD);

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        let mut recovered = String::new();
        zip.by_name_decrypt("model.ldr", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut recovered)
            .unwrap();
        assert_eq!(recovered, big);
    }

    #[test]
    fn an_empty_archive_is_still_valid() {
        let archive = write_encrypted_archive(&[], STUDIO_PASSWORD);
        let zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        assert_eq!(zip.len(), 0);
    }

    #[test]
    fn archives_are_reproducible() {
        assert_eq!(
            write_encrypted_archive(&sample(), STUDIO_PASSWORD),
            write_encrypted_archive(&sample(), STUDIO_PASSWORD)
        );
    }
}

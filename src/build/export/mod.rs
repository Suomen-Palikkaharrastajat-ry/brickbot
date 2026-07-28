//! Exporting a model to LDraw text and to BrickLink Studio's `.io` container.
//!
//! Because the model is stored natively in LDraw coordinates, writing the
//! `.ldr` is a direct dump — integer coordinates, no transform, no float
//! formatting ambiguity.
//!
//! An exported file contains only part *references* (`1 4 0 0 0 … 3001.dat`),
//! never part geometry, so nothing from the LDraw library is redistributed by
//! it.

#![allow(clippy::doc_markdown)]

pub mod zip;
pub mod zipcrypto;

use super::model::Model;
use zip::Entry;
use zipcrypto::STUDIO_PASSWORD;

/// Writes the model as LDraw text.
#[must_use]
pub fn to_ldr(model: &Model) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    out.push_str("0 FILE model.ldr\n");
    let _ = writeln!(out, "0 {}", model.name);
    out.push_str("0 Name: model.ldr\n");
    out.push_str("0 Author: Brickbot\n");
    out.push_str("0 !LDRAW_ORG Unofficial_Model\n");
    out.push_str("0 !LICENSE Redistributable under CCAL version 2.0\n");
    out.push('\n');

    for placement in &model.placements {
        let m = placement.orientation.to_ldraw_matrix();
        let _ = writeln!(
            out,
            "1 {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            placement.color,
            placement.pos.x,
            placement.pos.y,
            placement.pos.z,
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5],
            m[6],
            m[7],
            m[8],
            placement.part.file_name()
        );
    }

    out.push_str("0\n");
    out
}

/// The `.INFO` member Studio writes alongside the model.
///
/// The exact structure is not documented publicly and could not be verified
/// against a genuine Studio file, so this is a minimal plausible blob. If
/// Studio rejects it, set `enable_io_export = false` and attach the plain
/// `.ldr` instead — Studio imports LDraw natively.
#[must_use]
pub fn info_blob(model: &Model) -> String {
    format!(
        "Studio 2.0\nPartCount: {}\nName: {}\n",
        model.len(),
        model.name
    )
}

/// Builds a BrickLink Studio `.io` file.
///
/// `.io` is a ZIP encrypted with the fixed password `soho0909`, containing
/// `model.ldr`, `model2.ldr`, `.INFO` and `THUMBNAIL.PNG`. Studio writes both
/// model members, so both are included.
#[must_use]
pub fn to_io(model: &Model, thumbnail_png: &[u8]) -> Vec<u8> {
    let ldr = to_ldr(model);
    let entries = vec![
        Entry::new("model.ldr", ldr.clone()),
        Entry::new("model2.ldr", ldr),
        Entry::new(".INFO", info_blob(model)),
        Entry::new("THUMBNAIL.PNG", thumbnail_png.to_vec()),
    ];
    zip::write_encrypted_archive(&entries, STUDIO_PASSWORD)
}

/// A filename-safe version of a model name, for the attachment.
#[must_use]
pub fn file_stem(model_name: &str) -> String {
    let cleaned: String = model_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "model".to_string()
    } else {
        trimmed.chars().take(60).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::{PartKind, PartRef, Placement, PlacementId};
    use crate::build::units::{IVec3, Orientation};
    use std::io::Read;

    fn model_with(placements: Vec<Placement>) -> Model {
        let mut model = Model::new("Castle");
        model.placements = placements;
        model
    }

    fn placement(color: u16, pos: IVec3, orientation: Orientation) -> Placement {
        Placement {
            id: PlacementId::new(0, 0),
            part: PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
            color,
            pos,
            orientation,
            group: None,
        }
    }

    #[test]
    fn a_single_placement_becomes_one_type_one_line() {
        let model = model_with(vec![placement(4, IVec3::ZERO, Orientation::DEFAULT)]);
        let ldr = to_ldr(&model);
        assert!(
            ldr.contains("1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat"),
            "got:\n{ldr}"
        );
    }

    #[test]
    fn an_identity_orientation_writes_the_identity_matrix() {
        let model = model_with(vec![placement(
            14,
            IVec3::new(20, -24, 40),
            Orientation::DEFAULT,
        )]);
        let line = to_ldr(&model)
            .lines()
            .find(|l| l.starts_with("1 "))
            .unwrap()
            .to_string();
        assert_eq!(line, "1 14 20 -24 40 1 0 0 0 1 0 0 0 1 3001.dat");
    }

    #[test]
    fn a_quarter_turn_writes_its_rotation_matrix() {
        let model = model_with(vec![placement(4, IVec3::ZERO, Orientation::new(1, false))]);
        let line = to_ldr(&model)
            .lines()
            .find(|l| l.starts_with("1 "))
            .unwrap()
            .to_string();
        assert_eq!(line, "1 4 0 0 0 0 0 -1 0 1 0 1 0 0 3001.dat");
    }

    #[test]
    fn the_header_names_the_model_and_its_licence() {
        let ldr = to_ldr(&model_with(vec![]));
        assert!(ldr.starts_with("0 FILE model.ldr\n"));
        assert!(ldr.contains("0 Castle\n"));
        assert!(ldr.contains("!LICENSE Redistributable"));
    }

    #[test]
    fn coordinates_are_written_as_integers() {
        let ldr = to_ldr(&model_with(vec![placement(
            4,
            IVec3::new(-20, -48, 60),
            Orientation::DEFAULT,
        )]));
        assert!(ldr.contains("-20 -48 60"));
        assert!(!ldr.contains('.') || ldr.contains(".dat") || ldr.contains(".ldr"));
    }

    #[test]
    fn an_empty_model_still_produces_a_valid_file() {
        let ldr = to_ldr(&Model::new("Empty"));
        assert!(ldr.starts_with("0 FILE"));
        assert!(!ldr.lines().any(|l| l.starts_with("1 ")));
    }

    #[test]
    fn the_archive_contains_all_four_expected_members() {
        let model = model_with(vec![placement(4, IVec3::ZERO, Orientation::DEFAULT)]);
        let io = to_io(&model, b"\x89PNG\r\n\x1a\n");

        let zip = ::zip::ZipArchive::new(std::io::Cursor::new(io)).unwrap();
        let names: Vec<&str> = zip.file_names().collect();
        for expected in ["model.ldr", "model2.ldr", ".INFO", "THUMBNAIL.PNG"] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }

    #[test]
    fn the_archive_can_be_read_back_by_the_zip_crate() {
        // Conformance against an independent implementation: the `zip` crate
        // can decrypt ZipCrypto even though it cannot write it.
        let model = model_with(vec![placement(4, IVec3::ZERO, Orientation::DEFAULT)]);
        let io = to_io(&model, b"thumbnail bytes");

        let mut zip = ::zip::ZipArchive::new(std::io::Cursor::new(io)).unwrap();
        let mut recovered = String::new();
        zip.by_name_decrypt("model.ldr", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut recovered)
            .unwrap();

        assert_eq!(recovered, to_ldr(&model));
        assert!(recovered.contains("3001.dat"));
    }

    #[test]
    fn the_thumbnail_survives_the_round_trip_intact() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x01\x02binary\xff\xfe";
        let io = to_io(&Model::new("t"), png);

        let mut zip = ::zip::ZipArchive::new(std::io::Cursor::new(io)).unwrap();
        let mut recovered = Vec::new();
        zip.by_name_decrypt("THUMBNAIL.PNG", STUDIO_PASSWORD)
            .unwrap()
            .read_to_end(&mut recovered)
            .unwrap();
        assert_eq!(recovered, png);
    }

    #[test]
    fn both_model_members_have_the_same_contents() {
        let model = model_with(vec![placement(4, IVec3::ZERO, Orientation::DEFAULT)]);
        let io = to_io(&model, b"x");
        let mut zip = ::zip::ZipArchive::new(std::io::Cursor::new(io)).unwrap();

        let mut first = String::new();
        zip.by_name_decrypt("model.ldr", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut first)
            .unwrap();
        let mut second = String::new();
        zip.by_name_decrypt("model2.ldr", STUDIO_PASSWORD)
            .unwrap()
            .read_to_string(&mut second)
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn the_info_blob_reports_the_part_count() {
        let model = model_with(vec![
            placement(4, IVec3::ZERO, Orientation::DEFAULT),
            placement(4, IVec3::new(100, 0, 0), Orientation::DEFAULT),
        ]);
        let info = info_blob(&model);
        assert!(info.contains("PartCount: 2"));
        assert!(info.contains("Castle"));
    }

    #[test]
    fn file_stems_are_safe_and_bounded() {
        assert_eq!(file_stem("Castle"), "Castle");
        assert_eq!(file_stem("My Great Castle!"), "My-Great-Castle");
        assert_eq!(file_stem("../../etc/passwd"), "etc-passwd");
        assert_eq!(file_stem(""), "model");
        assert_eq!(file_stem("!!!"), "model");
        assert!(file_stem(&"x".repeat(500)).len() <= 60);
    }
}

//! Parser for the LDraw file format.
//!
//! The format is line-oriented and whitespace-separated; the first token gives
//! the line type. Parsing is deliberately tolerant — a malformed line is
//! skipped rather than failing the file, because the official library contains
//! plenty of unusual metadata and we only care about geometry.

#![allow(clippy::doc_markdown, clippy::many_single_char_names)]

use super::types::{LdrawLine, SubfileRef};
use crate::math::{Mat4, Vec3};

/// Normalises a sub-file reference: LDraw uses `\` as a path separator and is
/// case-insensitive, so we fold both to a canonical form.
#[must_use]
pub fn normalize_name(raw: &str) -> String {
    raw.trim().replace('\\', "/").to_lowercase()
}

fn parse_f32(tok: Option<&&str>) -> Option<f32> {
    tok?.parse::<f32>().ok()
}

fn parse_color(tok: Option<&&str>) -> Option<u16> {
    let raw = tok?;
    // Colour codes are normally plain integers. Direct colours are written as
    // `0x2RRGGBB`; we map those to the inherit code rather than dropping the
    // geometry, since we have no palette entry for them.
    raw.parse::<u16>().ok().or_else(|| {
        if raw.len() > 2 && (raw.starts_with("0x") || raw.starts_with("0X")) {
            Some(super::types::COLOR_INHERIT)
        } else {
            None
        }
    })
}

fn vec_at(tokens: &[&str], start: usize) -> Option<Vec3> {
    Some(Vec3::new(
        parse_f32(tokens.get(start))?,
        parse_f32(tokens.get(start + 1))?,
        parse_f32(tokens.get(start + 2))?,
    ))
}

/// Parses a single LDraw line. Returns `None` for blank lines and for lines
/// that are malformed or of an unknown type.
#[must_use]
pub fn parse_line(line: &str) -> Option<LdrawLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    match *tokens.first()? {
        "0" => Some(LdrawLine::Meta(
            trimmed
                .get(1..)
                .map(str::trim)
                .unwrap_or_default()
                .to_string(),
        )),
        "1" => {
            let color = parse_color(tokens.get(1))?;
            let x = parse_f32(tokens.get(2))?;
            let y = parse_f32(tokens.get(3))?;
            let z = parse_f32(tokens.get(4))?;
            let a = parse_f32(tokens.get(5))?;
            let b = parse_f32(tokens.get(6))?;
            let c = parse_f32(tokens.get(7))?;
            let d = parse_f32(tokens.get(8))?;
            let e = parse_f32(tokens.get(9))?;
            let f = parse_f32(tokens.get(10))?;
            let g = parse_f32(tokens.get(11))?;
            let h = parse_f32(tokens.get(12))?;
            let i = parse_f32(tokens.get(13))?;
            // The file name may itself contain spaces, so rejoin the tail.
            let file = normalize_name(&tokens[14..].join(" "));
            if file.is_empty() {
                return None;
            }
            Some(LdrawLine::Subfile(SubfileRef {
                color,
                transform: Mat4::from_ldraw(x, y, z, a, b, c, d, e, f, g, h, i),
                file,
            }))
        }
        "2" => Some(LdrawLine::Edge {
            color: parse_color(tokens.get(1))?,
            a: vec_at(&tokens, 2)?,
            b: vec_at(&tokens, 5)?,
        }),
        "3" => Some(LdrawLine::Triangle {
            color: parse_color(tokens.get(1))?,
            a: vec_at(&tokens, 2)?,
            b: vec_at(&tokens, 5)?,
            c: vec_at(&tokens, 8)?,
        }),
        "4" => Some(LdrawLine::Quad {
            color: parse_color(tokens.get(1))?,
            a: vec_at(&tokens, 2)?,
            b: vec_at(&tokens, 5)?,
            c: vec_at(&tokens, 8)?,
            d: vec_at(&tokens, 11)?,
        }),
        "5" => Some(LdrawLine::OptionalEdge {
            color: parse_color(tokens.get(1))?,
            a: vec_at(&tokens, 2)?,
            b: vec_at(&tokens, 5)?,
        }),
        _ => None,
    }
}

/// Parses a whole LDraw file, skipping lines that cannot be understood.
#[must_use]
pub fn parse_file(text: &str) -> Vec<LdrawLine> {
    text.lines().filter_map(parse_line).collect()
}

/// Splits a multi-part document (`.mpd`, and `.io`'s `model.ldr`) into its
/// named sub-files, in declaration order.
///
/// Content before the first `0 FILE` directive is returned under the name
/// `""` so that a plain single-file `.ldr` still round-trips.
#[must_use]
pub fn split_mpd(text: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current_name = String::new();
    let mut current_body = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("0 FILE ")
            .or_else(|| trimmed.strip_prefix("0 File "))
            .or_else(|| trimmed.strip_prefix("0 file "))
        {
            if !current_body.trim().is_empty() || !current_name.is_empty() {
                files.push((current_name.clone(), std::mem::take(&mut current_body)));
            }
            current_name = normalize_name(rest);
            continue;
        }
        current_body.push_str(line);
        current_body.push('\n');
    }

    if !current_body.trim().is_empty() || !current_name.is_empty() {
        files.push((current_name, current_body));
    }
    files
}

/// Returns true when the file header marks it as not redistributable, in which
/// case its geometry must never be embedded in an exported model.
#[must_use]
pub fn is_redistributable(lines: &[LdrawLine]) -> bool {
    !lines.iter().any(|l| match l {
        LdrawLine::Meta(text) => {
            let lower = text.to_lowercase();
            lower.contains("!license") && lower.contains("not redistributable")
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldraw::types::COLOR_INHERIT;

    #[test]
    fn parses_a_subfile_reference() {
        let line = parse_line("1 16 0 -24 0 1 0 0 0 1 0 0 0 1 3001.dat").unwrap();
        let sub = line.subfile().unwrap();
        assert_eq!(sub.color, COLOR_INHERIT);
        assert_eq!(sub.file, "3001.dat");
        assert_eq!(
            sub.transform.transform_point(Vec3::ZERO),
            Vec3::new(0.0, -24.0, 0.0)
        );
    }

    #[test]
    fn normalises_backslashes_and_case_in_subfile_names() {
        let line = parse_line("1 16 0 0 0 1 0 0 0 1 0 0 0 1 S\\4-4CYLI.DAT").unwrap();
        assert_eq!(line.subfile().unwrap().file, "s/4-4cyli.dat");
    }

    #[test]
    fn parses_triangles_and_quads() {
        let tri = parse_line("3 4 0 0 0 1 0 0 0 1 0").unwrap();
        assert_eq!(
            tri,
            LdrawLine::Triangle {
                color: 4,
                a: Vec3::new(0.0, 0.0, 0.0),
                b: Vec3::new(1.0, 0.0, 0.0),
                c: Vec3::new(0.0, 1.0, 0.0),
            }
        );

        let quad = parse_line("4 7 0 0 0 1 0 0 1 1 0 0 1 0").unwrap();
        assert!(matches!(quad, LdrawLine::Quad { color: 7, .. }));
    }

    #[test]
    fn parses_edges_and_optional_edges() {
        assert!(matches!(
            parse_line("2 24 0 0 0 1 0 0").unwrap(),
            LdrawLine::Edge { color: 24, .. }
        ));
        assert!(matches!(
            parse_line("5 24 0 0 0 1 0 0 0 1 0 0 0 1").unwrap(),
            LdrawLine::OptionalEdge { color: 24, .. }
        ));
    }

    #[test]
    fn keeps_meta_lines_verbatim() {
        assert_eq!(
            parse_line("0 !LICENSE Redistributable under CCAL version 2.0").unwrap(),
            LdrawLine::Meta("!LICENSE Redistributable under CCAL version 2.0".to_string())
        );
    }

    #[test]
    fn direct_colours_fall_back_to_inherit_rather_than_dropping_geometry() {
        let tri = parse_line("3 0x2FF0000 0 0 0 1 0 0 0 1 0").unwrap();
        assert!(matches!(
            tri,
            LdrawLine::Triangle {
                color: COLOR_INHERIT,
                ..
            }
        ));
    }

    #[test]
    fn skips_blank_and_malformed_lines() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("9 nonsense").is_none());
        // A type-3 line missing its last coordinate.
        assert!(parse_line("3 4 0 0 0 1 0 0 0 1").is_none());
        // A type-1 line with no file name.
        assert!(parse_line("1 16 0 0 0 1 0 0 0 1 0 0 0 1").is_none());
    }

    #[test]
    fn parse_file_skips_junk_but_keeps_geometry() {
        let text = "0 Brick 2 x 4\n\ngarbage line\n3 4 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 stud.dat\n";
        let lines = parse_file(text);
        assert_eq!(lines.len(), 3);
        assert!(matches!(lines[0], LdrawLine::Meta(_)));
        assert!(matches!(lines[1], LdrawLine::Triangle { .. }));
        assert!(matches!(lines[2], LdrawLine::Subfile(_)));
    }

    #[test]
    fn split_mpd_separates_named_files() {
        let text = "0 FILE main.ldr\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 sub.ldr\n0 FILE sub.ldr\n3 4 0 0 0 1 0 0 0 1 0\n";
        let files = split_mpd(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "main.ldr");
        assert!(files[0].1.contains("sub.ldr"));
        assert_eq!(files[1].0, "sub.ldr");
        assert!(files[1].1.contains("3 4"));
    }

    #[test]
    fn split_mpd_handles_a_plain_single_file() {
        let files = split_mpd("3 4 0 0 0 1 0 0 0 1 0\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "");
    }

    #[test]
    fn detects_non_redistributable_headers() {
        let ok = parse_file("0 !LICENSE Redistributable under CCAL version 2.0\n");
        assert!(is_redistributable(&ok));

        let bad = parse_file("0 !LICENSE Not redistributable : see NonCAreadme.txt\n");
        assert!(!is_redistributable(&bad));
    }
}

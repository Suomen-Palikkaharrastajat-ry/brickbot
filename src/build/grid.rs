//! Laying a drawing out into placements.
//!
//! A drawing is a grid of symbols, one cell per stud. A symbol repeated over a
//! rectangle the size of its part's footprint is **one** part, not one per
//! cell, which is what makes `bbbb` mean a single 1x4 brick rather than four
//! bricks in the same place.
//!
//! Deliberately separate from [`super::expand`]: that expands extents and
//! courses into structures, and shares nothing with this but the shape of what
//! comes out. Pure, and never sees the [`Model`] — [`super::lower`] resolves
//! the anchor and the legend's parts and hands both in.
//!
//! [`Model`]: super::model::Model

#![allow(clippy::doc_markdown)]

use super::dsl::ast::{GridLayer, GridSpec};
use super::model::PartRef;
use super::ops::Placed;
use super::units::{Direction, GridView, IVec3, LDU_BRICK, LDU_STUD, Orientation};

/// Half a stud. A part spanning `n` cells from index `i` has its centre at
/// `(2i + n) * HALF_STUD`, which is exact integer arithmetic for every `n`.
const HALF_STUD: i32 = LDU_STUD / 2;

/// The cells that mean "nothing here".
const EMPTY: [char; 2] = ['.', ' '];

/// A count of cells as a coordinate. Saturates rather than wrapping: a drawing
/// is bounded by the size of a Discord message long before this could matter,
/// but a silent wrap would fling parts to the far side of the model.
fn cells(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// One resolved legend entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegendPart {
    pub symbol: char,
    pub part: PartRef,
    pub color: u16,
}

/// Why a drawing could not be laid out.
///
/// A closed set, like [`super::lower::Unsupported`]: each variant is one
/// sentence in each locale bundle, rendered only by `report.rs`. Positions are
/// 1-based, the way a person counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridError {
    /// Nothing said what the symbols mean.
    NoLegend,
    /// A legend, but nothing drawn.
    NoCells,
    /// `bb = blue 1x1 brick` — the key must be one character.
    LegendSymbolNotSingleChar { text: String },
    UnknownSymbol {
        symbol: char,
        col: usize,
        row: usize,
        layer: usize,
        /// The symbols the legend *does* define, so the reply can say what
        /// was available rather than only what was wrong.
        known: String,
    },
    RaggedRows {
        layer: usize,
        row: usize,
        width: usize,
        expected: usize,
    },
    /// Cells that cannot be gathered into a whole part.
    Unplaced {
        symbol: char,
        col: usize,
        row: usize,
        layer: usize,
        count: usize,
    },
    /// More than one legend part needs looking up, which the one-choice
    /// dropdown cannot answer.
    ManyUnknownParts,
}

impl GridError {
    /// Localization key of the sentence describing this.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Self::NoLegend => "command.build.grid.no_legend",
            Self::NoCells => "command.build.grid.no_cells",
            Self::LegendSymbolNotSingleChar { .. } => "command.build.grid.legend_symbol",
            Self::UnknownSymbol { .. } => "command.build.grid.unknown_symbol",
            Self::RaggedRows { .. } => "command.build.grid.ragged_rows",
            Self::Unplaced { .. } => "command.build.grid.unplaced",
            Self::ManyUnknownParts => "command.build.grid.many_unknown_parts",
        }
    }
}

/// One part found in the drawing, before it is given a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    legend: usize,
    col: usize,
    row: usize,
    /// Span in columns and in rows, as it sits on the page.
    cols: usize,
    rows: usize,
    /// True when the footprint had to be turned to fit.
    rotated: bool,
}

/// How wide a part looks from a view, and how deep it runs away from it.
///
/// A plan view sees the whole footprint. An elevation sees only the face
/// turned towards it, so a part is never gathered up across two rows — two
/// bricks drawn one above the other really are two bricks.
const fn visible_span(view: GridView, part: &PartRef, rotated: bool) -> (usize, usize, usize) {
    let (a, b) = (part.footprint.0 as usize, part.footprint.1 as usize);
    let (across, other) = if rotated { (b, a) } else { (a, b) };
    let across = if across == 0 { 1 } else { across };
    let other = if other == 0 { 1 } else { other };
    if view.is_elevation() {
        (across, 1, other)
    } else {
        (across, other, 1)
    }
}

/// The orientation that puts a piece's drawn width on the view's column axis.
fn orientation_for(view: GridView, rotated: bool) -> Orientation {
    // An east or west elevation reads its columns along Z, where an unturned
    // part measures its second footprint dimension, so it starts a turn ahead.
    let sideways = matches!(view, GridView::East | GridView::West);
    Orientation::new(u8::from(sideways != rotated), false)
}

/// Checks a layer's rows are all the same width.
///
/// Trailing whitespace is already gone by the time a layer gets here, so a
/// difference that remains is a real misalignment rather than an invisible one
/// — and a drawing whose rows do not line up does not mean what it looks like.
fn check_rectangular(layer: &GridLayer, index: usize) -> Result<usize, GridError> {
    let expected = layer.rows.first().map_or(0, Vec::len);
    for (row, cells) in layer.rows.iter().enumerate() {
        if cells.len() != expected {
            return Err(GridError::RaggedRows {
                layer: index + 1,
                row: row + 1,
                width: cells.len(),
                expected,
            });
        }
    }
    Ok(expected)
}

/// Gathers one layer's cells into whole parts.
///
/// Scans row by row from the top left. At each cell not yet claimed it takes
/// the symbol's part and tries the footprint as drawn, then turned a quarter;
/// the first that fits wins and its cells are claimed. Nothing is ever
/// unclaimed again, so there is no backtracking and no search — the same
/// drawing always gives the same parts.
fn gather(
    view: GridView,
    layer: &GridLayer,
    legend: &[LegendPart],
    index: usize,
) -> Result<Vec<Piece>, GridError> {
    let width = check_rectangular(layer, index)?;
    let height = layer.rows.len();
    let mut used = vec![vec![false; width]; height];
    let mut pieces = Vec::new();
    let mut unplaced: Vec<(char, usize, usize)> = Vec::new();

    for row in 0..height {
        for col in 0..width {
            if used[row][col] {
                continue;
            }
            let symbol = layer.rows[row][col];
            if EMPTY.contains(&symbol) {
                used[row][col] = true;
                continue;
            }

            let Some(entry) = legend.iter().position(|e| e.symbol == symbol) else {
                return Err(GridError::UnknownSymbol {
                    symbol,
                    col: col + 1,
                    row: row + 1,
                    layer: index + 1,
                    known: legend
                        .iter()
                        .map(|e| e.symbol.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            };

            let part = &legend[entry].part;
            let placed = [false, true].into_iter().find_map(|rotated| {
                let (cols, rows, _) = visible_span(view, part, rotated);
                fits(layer, &used, symbol, col, row, cols, rows).then_some((cols, rows, rotated))
            });

            if let Some((cols, rows, rotated)) = placed {
                for line in &mut used[row..row + rows] {
                    for cell in &mut line[col..col + cols] {
                        *cell = true;
                    }
                }
                pieces.push(Piece {
                    legend: entry,
                    col,
                    row,
                    cols,
                    rows,
                    rotated,
                });
            } else {
                unplaced.push((symbol, col, row));
                used[row][col] = true;
            }
        }
    }

    if let Some((symbol, col, row)) = unplaced.first() {
        // Nothing is placed at all: half a drawing would leave the builder
        // undoing the rest by hand.
        return Err(GridError::Unplaced {
            symbol: *symbol,
            col: col + 1,
            row: row + 1,
            layer: index + 1,
            count: unplaced.len(),
        });
    }
    Ok(pieces)
}

/// Whether a `cols` by `rows` rectangle of `symbol` sits free at this corner.
fn fits(
    layer: &GridLayer,
    used: &[Vec<bool>],
    symbol: char,
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
) -> bool {
    if row + rows > layer.rows.len() || col + cols > layer.rows[row].len() {
        return false;
    }
    (row..row + rows)
        .all(|r| (col..col + cols).all(|c| !used[r][c] && layer.rows[r].get(c) == Some(&symbol)))
}

/// The height a piece occupies, in LDraw units.
fn piece_height(legend: &[LegendPart], piece: &Piece) -> i32 {
    legend[piece.legend].part.kind.height_ldu()
}

/// Lays a whole drawing out, relative to `anchor`.
///
/// `anchor` is the outer corner of cell (0,0) of the first block; columns run
/// from it along the view's column axis and rows along its row axis, so a plan
/// view drawn from the bottom reads mirrored, exactly as looking at the
/// underside of something does.
pub fn expand(
    spec: &GridSpec,
    legend: &[LegendPart],
    anchor: IVec3,
) -> Result<Vec<Placed>, GridError> {
    if legend.is_empty() {
        return Err(GridError::NoLegend);
    }
    if spec.layers.is_empty() {
        return Err(GridError::NoCells);
    }

    let view = spec.view;
    let col_dir = view.column_direction();
    let row_dir = view.row_direction();
    let mut out = Vec::new();
    // How far the current block sits from the anchor: up through the layers of
    // a plan view, back through the slices of an elevation.
    let mut block_offset = 0;

    for (index, layer) in spec.layers.iter().enumerate() {
        let pieces = gather(view, layer, legend, index)?;

        if view.is_elevation() {
            // Rows are courses of real parts, whose heights differ, so the
            // drawing is stacked from its bottom row up rather than cut at a
            // fixed pitch.
            let row_heights: Vec<i32> = (0..layer.rows.len())
                .map(|r| {
                    pieces
                        .iter()
                        .filter(|p| p.row == r)
                        .map(|p| piece_height(legend, p))
                        .max()
                        .unwrap_or(LDU_BRICK)
                })
                .collect();

            let mut deepest = 1;
            for piece in &pieces {
                let entry = &legend[piece.legend];
                let (_, _, depth) = visible_span(view, &entry.part, piece.rotated);
                deepest = deepest.max(depth);

                // Everything below this row lifts it off the anchor plane.
                let lift: i32 = row_heights[piece.row + 1..].iter().sum();
                let across = cells(2 * piece.col + piece.cols) * HALF_STUD;
                let into = block_offset + cells(depth) * HALF_STUD;

                out.push(Placed {
                    part: entry.part.clone(),
                    color: entry.color,
                    pos: anchor
                        .add(col_dir.step().scale(across))
                        .add(Direction::Up.step().scale(lift))
                        .add(view.into_screen().step().scale(into)),
                    orientation: orientation_for(view, piece.rotated),
                });
            }
            block_offset += cells(deepest) * LDU_STUD;
        } else {
            for piece in &pieces {
                let entry = &legend[piece.legend];
                let across = cells(2 * piece.col + piece.cols) * HALF_STUD;
                let down_page = cells(2 * piece.row + piece.rows) * HALF_STUD;

                out.push(Placed {
                    part: entry.part.clone(),
                    color: entry.color,
                    pos: anchor
                        .add(col_dir.step().scale(across))
                        .add(row_dir.step().scale(down_page))
                        .add(Direction::Up.step().scale(block_offset)),
                    orientation: orientation_for(view, piece.rotated),
                });
            }
            // The next layer rests on the tallest part of this one.
            block_offset += pieces
                .iter()
                .map(|p| piece_height(legend, p))
                .max()
                .unwrap_or(LDU_BRICK);
        }
    }

    if out.is_empty() {
        return Err(GridError::NoCells);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::PartKind;

    fn part(name: &str, w: u8, d: u8, kind: PartKind) -> PartRef {
        PartRef::new(name, name, (w, d), kind)
    }

    fn legend_of(entries: &[(char, PartRef, u16)]) -> Vec<LegendPart> {
        entries
            .iter()
            .map(|(symbol, part, color)| LegendPart {
                symbol: *symbol,
                part: part.clone(),
                color: *color,
            })
            .collect()
    }

    fn spec(view: GridView, blocks: &[&[&str]]) -> GridSpec {
        GridSpec {
            view,
            legend: Vec::new(),
            bad_legend_key: None,
            layers: blocks
                .iter()
                .map(|rows| GridLayer {
                    rows: rows.iter().map(|r| r.chars().collect()).collect(),
                })
                .collect(),
        }
    }

    fn brick_1x1() -> PartRef {
        part("3005", 1, 1, PartKind::Brick)
    }

    fn one_by_one(symbol: char) -> Vec<LegendPart> {
        legend_of(&[(symbol, brick_1x1(), 1)])
    }

    #[test]
    fn a_single_cell_becomes_one_part() {
        let placed = expand(
            &spec(GridView::Top, &[&["b"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(placed.len(), 1);
        // Centre of the one cell whose corner is the anchor.
        assert_eq!(placed[0].pos, IVec3::new(HALF_STUD, 0, HALF_STUD));
    }

    #[test]
    fn a_rectangle_of_one_symbol_becomes_one_part() {
        // Four cells of a 1x4 brick are one brick, not four in a heap.
        let legend = legend_of(&[('b', part("3010", 1, 4, PartKind::Brick), 4)]);
        let placed = expand(&spec(GridView::Top, &[&["bbbb"]]), &legend, IVec3::ZERO).unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].pos.x, 4 * HALF_STUD);
    }

    #[test]
    fn a_footprint_is_tried_as_drawn_then_rotated() {
        let legend = legend_of(&[('b', part("3010", 1, 4, PartKind::Brick), 4)]);

        // A 1x4 brick measures 1 across and 4 back, so a column of four cells
        // is exactly how it already lies: no turn needed.
        let down = expand(
            &spec(GridView::Top, &[&["b", "b", "b", "b"]]),
            &legend,
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].orientation.quarter_turns, 0);

        // Drawn across instead, it has to turn a quarter to fit.
        let across = expand(&spec(GridView::Top, &[&["bbbb"]]), &legend, IVec3::ZERO).unwrap();
        assert_eq!(across.len(), 1);
        assert_eq!(across[0].orientation.quarter_turns, 1);
    }

    #[test]
    fn the_legend_spelling_chooses_which_way_a_part_is_tried_first() {
        // The merge is greedy, so the same field tiles differently depending
        // on how the legend spelled the part. They are laid the way they were
        // drawn, which is the rule builders can reason about.
        let field: &[&str] = &["bb", "bb"];
        let as_1x2 = expand(
            &spec(GridView::Top, &[field]),
            &legend_of(&[('b', part("3004", 1, 2, PartKind::Brick), 1)]),
            IVec3::ZERO,
        )
        .unwrap();
        let as_2x1 = expand(
            &spec(GridView::Top, &[field]),
            &legend_of(&[('b', part("3004", 2, 1, PartKind::Brick), 1)]),
            IVec3::ZERO,
        )
        .unwrap();

        assert_eq!(as_1x2.len(), 2, "two dominoes cover a 2x2 field");
        assert_eq!(as_2x1.len(), 2);
        // 1x2 lays them running back; 2x1 lays them running across.
        let z_of = |placed: &[Placed]| placed.iter().map(|p| p.pos.z).collect::<Vec<_>>();
        assert_eq!(z_of(&as_1x2), vec![2 * HALF_STUD, 2 * HALF_STUD]);
        assert_eq!(z_of(&as_2x1), vec![HALF_STUD, 3 * HALF_STUD]);
    }

    #[test]
    fn the_scan_is_greedy_from_the_top_left() {
        // The 2x2 claims the top-left corner, leaving an L of single cells.
        let legend = legend_of(&[
            ('t', part("3003", 2, 2, PartKind::Brick), 1),
            ('b', brick_1x1(), 4),
        ]);
        let placed = expand(
            &spec(GridView::Top, &[&["ttb", "ttb", "bbb"]]),
            &legend,
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(placed.len(), 1 + 5);
    }

    #[test]
    fn the_same_drawing_always_produces_the_same_placements() {
        let legend = legend_of(&[('b', part("3004", 1, 2, PartKind::Brick), 1)]);
        let drawing = spec(GridView::Top, &[&["bbbb", "bbbb"]]);
        let first = expand(&drawing, &legend, IVec3::ZERO).unwrap();
        for _ in 0..5 {
            assert_eq!(expand(&drawing, &legend, IVec3::ZERO).unwrap(), first);
        }
    }

    #[test]
    fn cells_that_cannot_make_a_whole_part_are_reported_with_their_position() {
        // One lone cell of a 2x2 brick can never be a whole part.
        let legend = legend_of(&[('t', part("3003", 2, 2, PartKind::Brick), 1)]);
        let error = expand(&spec(GridView::Top, &[&[".t"]]), &legend, IVec3::ZERO).unwrap_err();
        assert_eq!(
            error,
            GridError::Unplaced {
                symbol: 't',
                col: 2,
                row: 1,
                layer: 1,
                count: 1,
            }
        );
    }

    #[test]
    fn an_unknown_symbol_names_itself_and_where_it_is() {
        let error = expand(
            &spec(GridView::Top, &[&["..", ".z"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap_err();
        assert_eq!(
            error,
            GridError::UnknownSymbol {
                symbol: 'z',
                col: 2,
                row: 2,
                layer: 1,
                // What the legend *did* define, so the reply can say so.
                known: "b".to_string(),
            }
        );
    }

    #[test]
    fn ragged_rows_are_reported() {
        let error = expand(
            &spec(GridView::Top, &[&["bbb", "bb"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap_err();
        assert_eq!(
            error,
            GridError::RaggedRows {
                layer: 1,
                row: 2,
                width: 2,
                expected: 3,
            }
        );
    }

    #[test]
    fn a_drawing_with_no_legend_or_no_cells_is_reported() {
        assert_eq!(
            expand(&spec(GridView::Top, &[&["b"]]), &[], IVec3::ZERO).unwrap_err(),
            GridError::NoLegend
        );
        assert_eq!(
            expand(&spec(GridView::Top, &[]), &one_by_one('b'), IVec3::ZERO).unwrap_err(),
            GridError::NoCells
        );
    }

    #[test]
    fn spaces_and_dots_both_mean_an_empty_cell() {
        let placed = expand(
            &spec(GridView::Top, &[&[". b", "b ."]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(placed.len(), 2);
    }

    #[test]
    fn a_top_view_maps_columns_east_and_rows_south() {
        let placed = expand(
            &spec(GridView::Top, &[&["b.", ".b"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        // Second cell is one stud further east and one further south.
        assert_eq!(
            placed[1].pos.sub(placed[0].pos),
            IVec3::new(LDU_STUD, 0, LDU_STUD)
        );
    }

    #[test]
    fn a_bottom_view_is_a_top_view_with_the_rows_reversed() {
        let drawing: &[&str] = &["b.", ".."];
        let top = expand(
            &spec(GridView::Top, &[drawing]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        let bottom = expand(
            &spec(GridView::Bottom, &[drawing]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        // The same cell, seen from underneath, is on the other side in Z.
        assert_eq!(top[0].pos.z, -bottom[0].pos.z);
        assert_eq!(top[0].pos.x, bottom[0].pos.x);
    }

    #[test]
    fn blank_lines_make_layers_that_stack_by_the_tallest_part() {
        let legend = legend_of(&[
            ('b', brick_1x1(), 1),
            ('p', part("3024", 1, 1, PartKind::Plate), 4),
        ]);
        let placed = expand(
            &spec(GridView::Top, &[&["b"], &["p"], &["b"]]),
            &legend,
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(placed.len(), 3);
        // A brick course, then a plate course, so the third sits at 24 + 8.
        assert_eq!(placed[0].pos.y, 0);
        assert_eq!(placed[1].pos.y, -LDU_BRICK);
        assert_eq!(placed[2].pos.y, -LDU_BRICK - 8);
    }

    #[test]
    fn every_elevation_view_puts_up_at_the_top_of_the_drawing() {
        for view in [
            GridView::North,
            GridView::South,
            GridView::East,
            GridView::West,
        ] {
            let placed =
                expand(&spec(view, &[&["b", "b"]]), &one_by_one('b'), IVec3::ZERO).unwrap();
            assert_eq!(placed.len(), 2);
            // The first row drawn is the higher one, and up is -Y.
            assert!(
                placed[0].pos.y < placed[1].pos.y,
                "{} drew its top row low",
                view.as_str()
            );
            // The bottom row rests on the anchor plane.
            assert_eq!(placed[1].pos.y, 0, "{} floats", view.as_str());
        }
    }

    #[test]
    fn an_elevation_view_never_merges_two_rows_into_one_part() {
        // Two 1x1 bricks drawn one above the other really are two bricks, even
        // though a 1x2 footprint would "fit" the rectangle.
        let legend = legend_of(&[('b', part("3004", 1, 2, PartKind::Brick), 1)]);
        let placed = expand(&spec(GridView::North, &[&["b", "b"]]), &legend, IVec3::ZERO).unwrap();
        assert_eq!(placed.len(), 2);
    }

    #[test]
    fn north_and_south_views_run_their_columns_opposite_ways() {
        let drawing: &[&str] = &["b."];
        let north = expand(
            &spec(GridView::North, &[drawing]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        let south = expand(
            &spec(GridView::South, &[drawing]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(north[0].pos.x, -south[0].pos.x);
    }

    #[test]
    fn east_and_west_views_run_their_columns_along_z() {
        let placed = expand(
            &spec(GridView::East, &[&["bb"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        assert_eq!(
            placed[0].pos.x, placed[1].pos.x,
            "columns must not run in X"
        );
        assert_ne!(placed[0].pos.z, placed[1].pos.z);
    }

    #[test]
    fn elevation_slices_recede_along_the_view_direction() {
        let placed = expand(
            &spec(GridView::North, &[&["b"], &["b"]]),
            &one_by_one('b'),
            IVec3::ZERO,
        )
        .unwrap();
        // North is -Z, so the second slice sits further back.
        assert!(placed[1].pos.z < placed[0].pos.z);
    }
}

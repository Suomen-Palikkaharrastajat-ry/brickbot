//! Drawing the model back as an ASCII grid.
//!
//! The inverse of [`super::grid`], and deliberately so: what it prints is
//! exactly what a builder can paste back to rebuild the same thing. That round
//! trip is how the drawing syntax is discovered — nobody has to read the
//! grammar if the bot writes it for them — and it is the property the tests
//! lean on hardest.
//!
//! A peer of `build::scene` (in the binary): model to text, where that is model to triangles.
//! Pure, and needs neither the LDraw part library nor the rasterizer, so it
//! still works when renders are falling back to plain boxes.

#![allow(clippy::doc_markdown)]

use super::model::{Model, Placement};
use super::queries;
use super::units::{Direction, GridView, IVec3, LDU_STUD};
use crate::ldraw::colors;

/// The model drawn from one side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextView {
    pub view: GridView,
    pub model_name: String,
    /// Symbol and the BrickTalk phrase it stands for, e.g. `blue 1x1 brick`.
    pub legend: Vec<(char, String)>,
    /// Each layer's rows, already joined into lines.
    pub layers: Vec<Vec<String>>,
    /// Layers dropped to fit inside a Discord message.
    pub omitted: u32,
}

/// The empty cell. Matches what the drawing grammar accepts, so the output can
/// be pasted straight back.
const EMPTY: char = '.';

/// Symbols to fall back on once the mnemonic ones are taken. Deliberately
/// excludes `.` and space, which mean "nothing here".
const POOL: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

/// The scalar coordinate of a point measured along a direction.
const fn along(point: IVec3, direction: Direction) -> i32 {
    let step = direction.step();
    point.x * step.x + point.y * step.y + point.z * step.z
}

/// A placement's extent along a direction, as `(near, far)` in LDraw units.
fn extent(placement: &Placement, direction: Direction) -> (i32, i32) {
    let (lo, hi) = placement.occupied_box();
    let a = along(lo, direction);
    let b = along(hi, direction);
    (a.min(b), a.max(b))
}

/// The distinct levels present along a direction, nearest first.
///
/// Layers and courses are bucketed by the values actually in the model rather
/// than cut at a fixed pitch, because a plate course is 8 LDU and a brick
/// course 24 — any fixed pitch would misalign one of them. It is also the
/// exact inverse of how [`super::grid`] stacks them.
fn levels(model: &Model, direction: Direction) -> Vec<i32> {
    let mut values: Vec<i32> = model
        .placements
        .iter()
        .map(|p| extent(p, direction).0)
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

/// Picks a stable symbol for every distinct part-and-colour in the model.
///
/// Walks [`queries::inventory`], which is already sorted by quantity and then
/// part number, so the assignment depends only on what the model contains and
/// never on the order it was built in. A colour's own initial is preferred, so
/// blue tends to come out as `b` and red as `r`.
fn assign_symbols(model: &Model) -> Vec<Key> {
    let mut taken: Vec<char> = Vec::new();
    let mut assigned = Vec::new();

    for (part, color, _) in queries::inventory(model) {
        let candidates = colors::name_of(color)
            .and_then(|name| name.chars().find(char::is_ascii_lowercase))
            .into_iter()
            .chain(part.kind.as_str().chars().next())
            .chain(POOL.chars());

        // More distinct parts than symbols: the rest are left out rather than
        // drawn as something they are not.
        let Some(symbol) = candidates.into_iter().find(|c| !taken.contains(c)) else {
            continue;
        };
        taken.push(symbol);
        assigned.push(Key {
            mark: symbol,
            ldraw_id: part.ldraw_id.clone(),
            color,
            phrase: phrase(&part, color),
        });
    }

    assigned
}

/// One entry of the drawn legend: the character, the element it stands for,
/// and the phrase that says so.
struct Key {
    mark: char,
    ldraw_id: String,
    color: u16,
    phrase: String,
}

/// The BrickTalk phrase for a part, so the legend reads as something that can
/// be typed back. Never localized — it is language, not UI text.
fn phrase(part: &super::model::PartRef, color: u16) -> String {
    let kind = part.kind.as_str();
    let size = if part.has_footprint() {
        format!("{}x{} ", part.footprint.0, part.footprint.1)
    } else {
        String::new()
    };
    colors::name_of(color).map_or_else(
        || format!("{} {size}{kind}", part.ldraw_id),
        |name| format!("{name} {size}{kind}"),
    )
}

/// Stamps one part's symbol over the cells it covers, growing the grid to
/// reach them. Rows and columns are filled with empty cells as they appear, so
/// a part far from the origin does not need the grid sized up front.
fn paint(
    grid: &mut Vec<Vec<char>>,
    symbol: char,
    first_col: i32,
    span: i32,
    row_index: usize,
    row_span: i32,
) {
    for r in row_index..row_index + usize::try_from(row_span).unwrap_or(1) {
        while grid.len() <= r {
            grid.push(Vec::new());
        }
        for c in first_col..first_col + span {
            let c = usize::try_from(c).unwrap_or(0);
            while grid[r].len() <= c {
                grid[r].push(EMPTY);
            }
            grid[r][c] = symbol;
        }
    }
}

/// Draws the model from one side.
///
/// `layer` picks a single block, counted from 1. Returns `None` when there is
/// nothing to draw, or when the layer asked for does not exist.
#[must_use]
pub fn render(model: &Model, view: GridView, layer: Option<u32>) -> Option<TextView> {
    if model.is_empty() {
        return None;
    }

    let col_dir = view.column_direction();
    let row_dir = view.row_direction();
    // An elevation's slices recede away from the viewer, but a plan view's
    // layers are numbered from the ground up the way real instructions are —
    // which is the opposite of the way the viewer is looking.
    let block_dir = if view.is_elevation() {
        view.into_screen()
    } else {
        Direction::Up
    };

    let block_levels = levels(model, block_dir);
    let row_levels = levels(model, row_dir);

    let symbols = assign_symbols(model);
    let symbol_of = |placement: &Placement| -> Option<char> {
        symbols
            .iter()
            .find(|key| key.ldraw_id == placement.part.ldraw_id && key.color == placement.color)
            .map(|key| key.mark)
    };

    // Cell zero is the model's own near edge, so a drawing starts hard left
    // and hard top rather than wherever it happens to sit in space.
    let col_origin = model
        .placements
        .iter()
        .map(|p| extent(p, col_dir).0)
        .min()?;
    let row_origin = model
        .placements
        .iter()
        .map(|p| extent(p, row_dir).0)
        .min()?;

    let mut layers: Vec<Vec<String>> = Vec::new();
    for (block_index, block_at) in block_levels.iter().enumerate() {
        if let Some(wanted) = layer {
            if block_index + 1 != wanted as usize {
                continue;
            }
        }

        let in_block: Vec<&Placement> = model
            .placements
            .iter()
            .filter(|p| extent(p, block_dir).0 == *block_at)
            .collect();
        if in_block.is_empty() {
            continue;
        }

        let mut grid: Vec<Vec<char>> = Vec::new();
        for placement in &in_block {
            let Some(symbol) = symbol_of(placement) else {
                continue;
            };
            let (col_lo, col_hi) = extent(placement, col_dir);
            let first_col = ((col_lo - col_origin) / LDU_STUD).max(0);
            let span = ((col_hi - col_lo) / LDU_STUD).max(1);

            // An elevation's rows are courses, whose heights differ between
            // bricks and plates, so they are bucketed by the levels actually
            // present. A plan view's rows are studs and divide evenly.
            let (row_index, row_span) = if view.is_elevation() {
                let level = extent(placement, row_dir).0;
                let index = row_levels.iter().position(|l| *l == level).unwrap_or(0);
                (index, 1)
            } else {
                let (row_lo, row_hi) = extent(placement, row_dir);
                (
                    usize::try_from((row_lo - row_origin) / LDU_STUD).unwrap_or(0),
                    ((row_hi - row_lo) / LDU_STUD).max(1),
                )
            };
            paint(&mut grid, symbol, first_col, span, row_index, row_span);
        }

        let width = grid.iter().map(Vec::len).max().unwrap_or(0);
        let rows: Vec<String> = grid
            .into_iter()
            .map(|mut row| {
                row.resize(width, EMPTY);
                row.into_iter().collect()
            })
            .collect();
        if !rows.is_empty() {
            layers.push(rows);
        }
    }

    if layers.is_empty() {
        return None;
    }

    // Only the symbols that ended up on the page belong in the legend.
    let drawn: Vec<char> = layers
        .iter()
        .flatten()
        .flat_map(|row| row.chars())
        .filter(|c| *c != EMPTY)
        .collect();
    let legend = symbols
        .into_iter()
        .filter(|key| drawn.contains(&key.mark))
        .map(|key| (key.mark, key.phrase))
        .collect();

    Some(TextView {
        view,
        model_name: model.name.clone(),
        legend,
        layers,
        omitted: 0,
    })
}

/// Renders a view as the very syntax it can be typed back in.
#[must_use]
pub fn to_block(view: &TextView) -> String {
    use std::fmt::Write;

    let mut out = format!("Build from {} at origin:\n```\n", view.view.as_str());
    for (symbol, text) in &view.legend {
        let _ = writeln!(out, "{symbol} = {text}");
    }
    if !view.legend.is_empty() {
        out.push('\n');
    }
    let blocks: Vec<String> = view.layers.iter().map(|rows| rows.join("\n")).collect();
    out.push_str(&blocks.join("\n\n"));
    out.push_str("\n```");
    out
}

/// Drops whole layers from the end until the drawing fits in `budget` bytes.
///
/// Returns how many were dropped. Layers go whole rather than being cut off
/// mid-row, because half a layer is not something anyone can paste back.
pub fn fit(view: &mut TextView, budget: usize) -> u32 {
    let mut dropped = 0;
    while view.layers.len() > 1 && to_block(view).len() > budget {
        view.layers.pop();
        dropped += 1;
    }
    view.omitted = dropped;
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PartKind, PartRef, PlacementId};
    use crate::units::Orientation;

    fn brick_1x1() -> PartRef {
        PartRef::new("3005", "Brick 1 x 1", (1, 1), PartKind::Brick)
    }

    fn place(model: &mut Model, part: PartRef, color: u16, pos: IVec3) {
        let seq = u32::try_from(model.placements.len()).unwrap_or(0);
        model.placements.push(Placement {
            id: PlacementId::new(0, seq),
            part,
            color,
            pos,
            orientation: Orientation::DEFAULT,
            group: None,
        });
    }

    /// Two blue bricks side by side, with a red one east of them.
    fn row_of_three() -> Model {
        let mut model = Model::new("Test");
        place(&mut model, brick_1x1(), 1, IVec3::new(10, 0, 10));
        place(&mut model, brick_1x1(), 1, IVec3::new(30, 0, 10));
        place(&mut model, brick_1x1(), 4, IVec3::new(50, 0, 10));
        model
    }

    #[test]
    fn an_empty_model_has_no_view() {
        assert!(render(&Model::new("Empty"), GridView::Top, None).is_none());
    }

    #[test]
    fn a_row_of_bricks_draws_as_a_row_of_symbols() {
        let view = render(&row_of_three(), GridView::Top, None).unwrap();
        assert_eq!(view.layers.len(), 1);
        assert_eq!(view.layers[0], vec!["bbr".to_string()]);
    }

    #[test]
    fn a_colour_gives_its_first_letter_when_it_is_free() {
        let view = render(&row_of_three(), GridView::Top, None).unwrap();
        let symbols: Vec<char> = view.legend.iter().map(|(c, _)| *c).collect();
        assert!(symbols.contains(&'b'), "blue should take 'b': {symbols:?}");
        assert!(symbols.contains(&'r'), "red should take 'r': {symbols:?}");
    }

    #[test]
    fn symbols_come_from_the_inventory_order_so_the_output_is_stable() {
        // The same model drawn twice must read identically, whatever order the
        // placements happen to sit in.
        let mut a = row_of_three();
        let b = row_of_three();
        a.placements.reverse();
        assert_eq!(
            render(&a, GridView::Top, None).unwrap().legend,
            render(&b, GridView::Top, None).unwrap().legend
        );
    }

    #[test]
    fn the_legend_reads_as_bricktalk_not_as_a_translated_string() {
        let view = render(&row_of_three(), GridView::Top, None).unwrap();
        let (_, text) = view.legend.iter().find(|(c, _)| *c == 'b').unwrap();
        assert_eq!(text, "blue 1x1 brick");
    }

    #[test]
    fn a_view_prints_as_something_that_can_be_typed_back() {
        let block = to_block(&render(&row_of_three(), GridView::Top, None).unwrap());
        assert!(block.starts_with("Build from top at origin:\n```\n"));
        assert!(block.contains("b = blue 1x1 brick\n"));
        assert!(block.contains("\nbbr"));
        assert!(block.ends_with("\n```"));
    }

    #[test]
    fn a_stack_draws_one_layer_per_level() {
        let mut model = Model::new("Tower");
        place(&mut model, brick_1x1(), 1, IVec3::new(10, 0, 10));
        place(&mut model, brick_1x1(), 4, IVec3::new(10, -24, 10));
        let view = render(&model, GridView::Top, None).unwrap();
        assert_eq!(view.layers.len(), 2);
        // Layer 0 is the lowest, so the blue brick comes first.
        assert_eq!(view.layers[0], vec!["b".to_string()]);
        assert_eq!(view.layers[1], vec!["r".to_string()]);
    }

    #[test]
    fn a_layer_selector_shows_only_that_layer() {
        let mut model = Model::new("Tower");
        place(&mut model, brick_1x1(), 1, IVec3::new(10, 0, 10));
        place(&mut model, brick_1x1(), 4, IVec3::new(10, -24, 10));
        let view = render(&model, GridView::Top, Some(2)).unwrap();
        assert_eq!(view.layers, vec![vec!["r".to_string()]]);
        assert!(render(&model, GridView::Top, Some(9)).is_none());
    }

    #[test]
    fn an_elevation_puts_the_highest_course_at_the_top() {
        let mut model = Model::new("Tower");
        place(&mut model, brick_1x1(), 1, IVec3::new(10, 0, 10));
        place(&mut model, brick_1x1(), 4, IVec3::new(10, -24, 10));
        let view = render(&model, GridView::North, None).unwrap();
        // One slice, two courses: red on top, blue below.
        assert_eq!(view.layers.len(), 1);
        assert_eq!(view.layers[0], vec!["r".to_string(), "b".to_string()]);
    }

    #[test]
    fn dropping_layers_brings_a_big_view_under_the_budget() {
        let mut model = Model::new("Tall");
        for i in 0..10 {
            place(&mut model, brick_1x1(), 1, IVec3::new(10, -24 * i, 10));
        }
        let mut view = render(&model, GridView::Top, None).unwrap();
        assert_eq!(view.layers.len(), 10);
        let dropped = fit(&mut view, 80);
        assert!(dropped > 0);
        assert_eq!(view.omitted, dropped);
        assert!(to_block(&view).len() <= 80 || view.layers.len() == 1);
    }

    #[test]
    fn fitting_never_drops_the_last_layer() {
        let mut view = render(&row_of_three(), GridView::Top, None).unwrap();
        fit(&mut view, 1);
        assert_eq!(view.layers.len(), 1);
    }
}

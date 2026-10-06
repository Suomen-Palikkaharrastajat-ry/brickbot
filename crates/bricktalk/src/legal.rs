//! What counts as a legal connection, and how to correct one that is not.
//!
//! There is no stud-and-anti-stud model anywhere in the codebase — `LDU_STUD`
//! is a pitch, and `p/stud.dat` is triangles for the renderer. Legality is
//! therefore expressed in the same integer boxes everything else here uses,
//! which is enough for System parts:
//!
//! 1. **Lattice** — a part's occupied box starts on the stud grid, and its
//!    base sits on a whole plate. Anything else connects to nothing.
//! 2. **No interpenetration** — two parts may share a face but not a volume.
//! 3. **Connected** — a part rests on the ground plane or on the top face of
//!    something it overlaps.
//!
//! Nothing here rejects by default. A command that would break these rules is
//! *corrected* and the correction reported, because a builder who says
//! `Add a brick at 3 5 7.` means a brick, not an error message. Refusing is
//! the last resort, for when there is genuinely nowhere to put the thing.
//!
//! This module is pure. It is applied by `dispatch`, on the path new commands
//! take — never on replay, so revisions recorded before it existed still
//! materialize exactly as they always did.

use super::lower::{LowerError, Unsupported};
use super::model::{Model, PartRef, Placement, PlacementId};
use super::ops::{Op, Placed, Repeat, apply};
use super::units::{IVec3, LDU_PLATE, LDU_STUD, Orientation};
use std::collections::BTreeMap;

/// Half a stud: the offset an odd-width part's centre needs for its edges to
/// land on the grid.
const HALF_STUD: i32 = LDU_STUD / 2;

/// The residue a part's centre must have on each horizontal axis, modulo
/// [`LDU_STUD`], for its occupied box to start on a stud boundary.
///
/// `occupied_box` measures out from the centre by `w * LDU_STUD / 2`, so an
/// even-width part is centred on a grid line and an odd-width part is centred
/// half a stud off it — which is why a 1x1 brick at the origin is *not* on the
/// lattice, and a 2x4 is.
#[must_use]
pub fn lattice_residue(part: &PartRef, orientation: Orientation) -> (i32, i32) {
    let (w, d) = part.footprint_or_default();
    let (w, d) = orientation.apply_footprint(i32::from(w), i32::from(d));
    ((w % 2) * HALF_STUD, (d % 2) * HALF_STUD)
}

/// True when a part at this position connects to the grid at all.
#[must_use]
pub fn is_on_lattice(pos: IVec3, part: &PartRef, orientation: Orientation) -> bool {
    let (rx, rz) = lattice_residue(part, orientation);
    pos.x.rem_euclid(LDU_STUD) == rx
        && pos.z.rem_euclid(LDU_STUD) == rz
        && pos.y.rem_euclid(LDU_PLATE) == 0
}

/// The nearest position that puts `part` on the lattice.
///
/// The identity when it is already there, which is what lets this run over
/// every operation without disturbing the ones that were right to begin with.
#[must_use]
pub fn snap_to_lattice(pos: IVec3, part: &PartRef, orientation: Orientation) -> IVec3 {
    let (rx, rz) = lattice_residue(part, orientation);
    IVec3::new(
        round_to(pos.x, LDU_STUD, rx),
        round_to(pos.y, LDU_PLATE, 0),
        round_to(pos.z, LDU_STUD, rz),
    )
}

/// Rounds `value` to the nearest `residue + n * modulus`.
///
/// Exact halves round toward positive infinity — an arbitrary choice, but a
/// fixed one, because the same sentence must always produce the same model.
const fn round_to(value: i32, modulus: i32, residue: i32) -> i32 {
    let shifted = value - residue;
    let steps = (shifted * 2 + modulus).div_euclid(modulus * 2);
    residue + steps * modulus
}

/// An axis-aligned box in `LDraw` units, as `(min, max)`. Remember that `min.y`
/// is the *top* face and `max.y` the bottom one, because -Y is up.
type Box3 = (IVec3, IVec3);

/// How hard to insist that a batch ends up held up by something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportPolicy {
    /// Every box must rest on the ground or on something. For a single part
    /// the builder placed by hand, where floating is simply a mistake.
    Every,
    /// The batch moves as one body. Prefer a resting place where everything
    /// is held up, but settle for one where *something* is.
    ///
    /// A structure's internal integrity is the expander's business; all this
    /// decides is where the whole thing sits. Insisting on more would refuse
    /// the wall in `docs/commands/build.md`'s own example, which legitimately
    /// overhangs the plate it is built on.
    Batch,
    /// Resolve overlaps and burial, but leave floating alone.
    ///
    /// For `Move`/`Raise`/`Lower`: a builder who lifts a brick deliberately
    /// must not have it fall straight back down, and may well be about to
    /// slide something underneath. `Show unsupported bricks.` still finds it.
    KeepHeight,
}

/// The vertical delta that brings `batch` to rest against `rest`.
///
/// Positive is downward, since -Y is up. Zero whenever the batch is already
/// legal, which is what lets this run over every operation harmlessly.
///
/// Candidates are the rest levels actually available — the ground plane, and
/// the top face of anything the batch overlaps horizontally — rather than a
/// search over every height. The nearest collision-free one wins, ties going
/// downward, so a part that is merely a plate too high comes down one plate
/// instead of being flung to the top of the model.
fn settle(rest: &[Box3], batch: &[Box3], policy: SupportPolicy) -> Result<i32, LowerError> {
    if batch.is_empty() {
        return Ok(0);
    }
    // Anything that cannot overlap the batch horizontally can neither block it
    // nor hold it up, at any height at all. Dropping those first is what keeps
    // a 400-part structure from being quadratic against a large model.
    let relevant: Vec<Box3> = rest
        .iter()
        .copied()
        .filter(|r| batch.iter().any(|b| overlaps_horizontally(*r, *b)))
        .collect();

    let mut candidates = vec![0];
    for b in batch {
        // Down onto the ground plane.
        candidates.push(-b.1.y);
        // Down onto, or up out of, anything that could hold this box.
        for r in &relevant {
            if overlaps_horizontally(*r, *b) {
                candidates.push(r.0.y - b.1.y);
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    // Nearest first; on a tie the larger delta, which is the lower position.
    candidates.sort_by_key(|d| (d.abs(), -d));

    let mut fallback = None;
    for delta in candidates {
        let moved: Vec<Box3> = batch.iter().map(|b| shift(*b, delta)).collect();
        // Never underground: the bottom face may reach the ground but not pass
        // through it.
        if moved.iter().any(|b| b.1.y > 0) {
            continue;
        }
        if moved
            .iter()
            .any(|b| relevant.iter().any(|r| super::model::boxes_overlap(*b, *r)))
        {
            continue;
        }
        let held = moved
            .iter()
            .filter(|b| is_supported(**b, &relevant, &moved))
            .count();
        match policy {
            SupportPolicy::KeepHeight => return Ok(delta),
            SupportPolicy::Every | SupportPolicy::Batch if held == moved.len() => {
                return Ok(delta);
            }
            SupportPolicy::Batch if held > 0 => fallback = fallback.or(Some(delta)),
            _ => {}
        }
    }
    fallback.ok_or(LowerError::Unsupported(Unsupported::NoLegalSpot))
}

/// Puts a batch of not-yet-placed parts somewhere legal: on the grid, out of
/// anything already built, and resting on something.
///
/// The batch keeps its shape — every part moves by the same vertical delta —
/// so a structure stays a structure. This is the entry point for anything
/// weighing up a placement it has not committed to, such as a proposal.
pub fn settle_placements(
    model: &Model,
    batch: &[Placed],
    policy: SupportPolicy,
) -> Result<Vec<Placed>, LowerError> {
    let snapped: Vec<Placed> = batch
        .iter()
        .map(|placed| Placed {
            pos: snap_to_lattice(placed.pos, &placed.part, placed.orientation),
            ..placed.clone()
        })
        .collect();
    let boxes: Vec<Box3> = snapped.iter().map(placed_box).collect();
    let rest: Vec<Box3> = model
        .placements
        .iter()
        .map(Placement::occupied_box)
        .collect();

    let delta = settle(&rest, &boxes, policy)?;
    Ok(snapped
        .into_iter()
        .map(|placed| Placed {
            pos: placed.pos.add(IVec3::new(0, delta, 0)),
            ..placed
        })
        .collect())
}

/// The box a batch member occupies where it currently sits.
#[must_use]
pub fn placed_box(placed: &Placed) -> Box3 {
    super::model::part_box(&placed.part, placed.orientation, placed.pos)
}

/// True when this box rests on the ground, on something already in the model,
/// or on one of its own batch-mates.
fn is_supported(b: Box3, rest: &[Box3], batch: &[Box3]) -> bool {
    if b.1.y == 0 {
        return true; // On the ground plane.
    }
    // Its bottom face must meet another box's top face, and overlap it.
    let meets = |other: &Box3| other.0.y == b.1.y && overlaps_horizontally(*other, b);
    rest.iter().any(meets) || batch.iter().any(|o| *o != b && meets(o))
}

/// Whether two boxes share any ground area. Independent of height, which is
/// what makes it safe to compute once and reuse across every candidate.
const fn overlaps_horizontally(a: Box3, b: Box3) -> bool {
    a.0.x < b.1.x && a.1.x > b.0.x && a.0.z < b.1.z && a.1.z > b.0.z
}

const fn shift(b: Box3, delta: i32) -> Box3 {
    (
        IVec3::new(b.0.x, b.0.y + delta, b.0.z),
        IVec3::new(b.1.x, b.1.y + delta, b.1.z),
    )
}

/// What correcting an operation actually changed, for telling the builder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Adjustment {
    /// How many placements were nudged onto the stud grid.
    pub snapped: usize,
    /// How far the batch settled, in `LDraw` units. Positive is downward.
    pub settled: i32,
    /// How many placements that move applied to.
    pub settled_count: usize,
}

impl Adjustment {
    /// True when the operation was legal as written and nothing was touched.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        self.snapped == 0 && self.settled == 0
    }
}

/// An operation made safe to apply, plus whatever it took to get there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legalized {
    /// The original operation, with any correction folded into it.
    pub op: Op,
    /// Follow-up operations, for the cases where the correction cannot be
    /// expressed in the original — a rotate whose new footprint needs a shift.
    pub corrections: Vec<Op>,
    pub adjustment: Adjustment,
}

impl Legalized {
    /// The operation and its follow-ups, in the order they must be applied.
    pub fn ops(&self) -> impl Iterator<Item = &Op> {
        std::iter::once(&self.op).chain(self.corrections.iter())
    }

    /// An operation that needed no correction at all.
    const fn unchanged(op: Op) -> Self {
        Self {
            op,
            corrections: Vec::new(),
            adjustment: Adjustment {
                snapped: 0,
                settled: 0,
                settled_count: 0,
            },
        }
    }
}

/// Corrects one operation so that applying it cannot produce illegal geometry.
///
/// `rev` must be the revision the operation is about to occupy, so the probe
/// below assigns exactly the [`PlacementId`]s the real `apply` will.
///
/// This runs on the path a *new* command takes, and the corrected operation is
/// what gets recorded. Replay is deliberately left alone: an old revision log
/// still materializes into the model it always did, and a new one is legal by
/// construction rather than by being re-checked.
///
/// [`PlacementId`]: super::model::PlacementId
pub fn legalize(model: &Model, rev: u32, op: &Op) -> Result<Legalized, LowerError> {
    // Operations that move no geometry cannot make it illegal.
    match op {
        Op::Paint { .. }
        | Op::Remove { .. }
        | Op::Select { .. }
        | Op::Group { .. }
        | Op::Landmark { .. }
        | Op::Rename { .. } => return Ok(Legalized::unchanged(op.clone())),
        _ => {}
    }

    // What the model would look like, so we can see the result of a move or a
    // rotate rather than having to predict it.
    let mut probe = model.clone();
    let effect = apply(&mut probe, rev, op);
    let touched: Vec<PlacementId> = effect
        .added
        .iter()
        .chain(effect.affected.iter())
        .copied()
        .collect();
    if touched.is_empty() {
        return Ok(Legalized::unchanged(op.clone()));
    }

    // An operation carrying one offset for many placements can only be
    // corrected by one offset, so its lattice correction has to come from the
    // offset itself rather than from where each placement happened to land.
    let shared_offset = match op {
        Op::Move { offset, .. } => Some(snap_offset(*offset)),
        _ => None,
    };

    let batch: Vec<&Placement> = touched.iter().filter_map(|id| probe.get(*id)).collect();
    let rest: Vec<Box3> = probe
        .placements
        .iter()
        .filter(|p| !touched.contains(&p.id))
        .map(Placement::occupied_box)
        .collect();

    // Where each touched placement goes once it is on the grid.
    let deltas: Vec<IVec3> = batch
        .iter()
        .map(|placement| {
            shared_offset.map_or_else(
                || {
                    snap_to_lattice(placement.pos, &placement.part, placement.orientation)
                        .sub(placement.pos)
                },
                // The offset moved it; correcting the offset corrects it too.
                |snapped| snapped.sub(offset_of_move(op)),
            )
        })
        .collect();
    let snapped = deltas.iter().filter(|d| **d != IVec3::ZERO).count();

    let boxes: Vec<Box3> = batch
        .iter()
        .zip(&deltas)
        .map(|(p, d)| translate(p.occupied_box(), *d))
        .collect();
    let settled = settle(&rest, &boxes, policy_for(op))?;

    let adjustment = Adjustment {
        snapped,
        settled,
        settled_count: if settled == 0 { 0 } else { batch.len() },
    };
    if adjustment.is_none() {
        return Ok(Legalized::unchanged(op.clone()));
    }

    let (op, corrections) = rewrite(op, &batch, &deltas, IVec3::new(0, settled, 0));
    Ok(Legalized {
        op,
        corrections,
        adjustment,
    })
}

/// Folds the corrections back into the operation, so that what gets recorded
/// is what should have been asked for in the first place.
///
/// `deltas` is per placement and `lift` applies to all of them.
fn rewrite(op: &Op, batch: &[&Placement], deltas: &[IVec3], lift: IVec3) -> (Op, Vec<Op>) {
    match op {
        Op::Add {
            part,
            color,
            pos,
            orientation,
            repeat,
            group,
        } => (
            Op::Add {
                part: part.clone(),
                color: *color,
                // Every copy shares one starting point, so the first
                // placement's correction is the operation's correction.
                pos: pos.add(deltas[0]).add(lift),
                orientation: *orientation,
                // A step off the lattice walks every later copy off it too.
                repeat: repeat.map(|r| Repeat {
                    count: r.count,
                    step: snap_offset(r.step),
                }),
                group: group.clone(),
            },
            Vec::new(),
        ),
        Op::AddMany { placements, group } => (
            Op::AddMany {
                placements: placements
                    .iter()
                    .zip(deltas)
                    .map(|(placed, delta)| Placed {
                        part: placed.part.clone(),
                        color: placed.color,
                        pos: placed.pos.add(*delta).add(lift),
                        orientation: placed.orientation,
                    })
                    .collect(),
                group: group.clone(),
            },
            Vec::new(),
        ),
        Op::Move { targets, offset } => (
            Op::Move {
                targets: targets.clone(),
                offset: snap_offset(*offset).add(lift),
            },
            Vec::new(),
        ),
        // A rotate or a replace changes a part's footprint, so where it must
        // sit changes with it — but neither operation has anywhere to put a
        // position. The shift has to follow as its own move.
        other => {
            let mut by_delta: BTreeMap<(i32, i32, i32), Vec<PlacementId>> = BTreeMap::new();
            for (placement, delta) in batch.iter().zip(deltas) {
                let total = delta.add(lift);
                if total != IVec3::ZERO {
                    by_delta
                        .entry((total.x, total.y, total.z))
                        .or_default()
                        .push(placement.id);
                }
            }
            // Grouped and sorted by offset, so the same rotate always produces
            // the same sequence of operations.
            let corrections = by_delta
                .into_iter()
                .map(|((x, y, z), targets)| Op::Move {
                    targets,
                    offset: IVec3::new(x, y, z),
                })
                .collect();
            (other.clone(), corrections)
        }
    }
}

/// How insistent to be about support, given what the operation is doing.
const fn policy_for(op: &Op) -> SupportPolicy {
    match op {
        // A part placed by hand, on its own, should not be left in mid-air.
        Op::Add { repeat: None, .. } => SupportPolicy::Every,
        // A structure moves as one body and may overhang.
        Op::Add { .. } | Op::AddMany { .. } => SupportPolicy::Batch,
        // An explicit move stays where it was put.
        _ => SupportPolicy::KeepHeight,
    }
}

/// Rounds a displacement onto the lattice: whole studs sideways, whole plates
/// up and down. Independent of any footprint, because moving a legal placement
/// by a lattice multiple leaves it legal.
const fn snap_offset(offset: IVec3) -> IVec3 {
    IVec3::new(
        round_to(offset.x, LDU_STUD, 0),
        round_to(offset.y, LDU_PLATE, 0),
        round_to(offset.z, LDU_STUD, 0),
    )
}

const fn offset_of_move(op: &Op) -> IVec3 {
    match op {
        Op::Move { offset, .. } => *offset,
        _ => IVec3::ZERO,
    }
}

const fn translate(b: Box3, d: IVec3) -> Box3 {
    (
        IVec3::new(b.0.x + d.x, b.0.y + d.y, b.0.z + d.z),
        IVec3::new(b.1.x + d.x, b.1.y + d.y, b.1.z + d.z),
    )
}

#[cfg(test)]
mod tests {
    use super::super::model::PartKind;
    use super::*;

    fn brick_2x4() -> PartRef {
        PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
    }

    fn brick_1x1() -> PartRef {
        PartRef::new("3005", "Brick 1 x 1", (1, 1), PartKind::Brick)
    }

    #[test]
    fn a_two_wide_part_sits_on_the_grid_when_its_centre_is_a_whole_stud() {
        assert_eq!(lattice_residue(&brick_2x4(), Orientation::DEFAULT), (0, 0));
        assert!(is_on_lattice(
            IVec3::ZERO,
            &brick_2x4(),
            Orientation::DEFAULT
        ));
        assert!(is_on_lattice(
            IVec3::new(20, -24, 40),
            &brick_2x4(),
            Orientation::DEFAULT
        ));
    }

    #[test]
    fn a_one_wide_part_sits_on_the_grid_when_its_centre_is_half_a_stud() {
        // Its edges are what must land on the grid, and they are 10 either
        // side of the centre. So the origin itself is the wrong place for it.
        assert_eq!(
            lattice_residue(&brick_1x1(), Orientation::DEFAULT),
            (10, 10)
        );
        assert!(!is_on_lattice(
            IVec3::ZERO,
            &brick_1x1(),
            Orientation::DEFAULT
        ));
        assert!(is_on_lattice(
            IVec3::new(10, 0, 10),
            &brick_1x1(),
            Orientation::DEFAULT
        ));
    }

    #[test]
    fn snapping_a_placement_that_is_already_legal_changes_nothing() {
        // Everything else in the build pipeline depends on this: snapping runs
        // over every operation, so it must be the identity on correct ones.
        for pos in [
            IVec3::ZERO,
            IVec3::new(20, -24, 40),
            IVec3::new(-60, -8, 100),
            IVec3::new(40, -96, -20),
        ] {
            assert_eq!(
                snap_to_lattice(pos, &brick_2x4(), Orientation::DEFAULT),
                pos,
                "{pos:?} was already legal"
            );
        }
        assert_eq!(
            snap_to_lattice(IVec3::new(10, -8, 30), &brick_1x1(), Orientation::DEFAULT),
            IVec3::new(10, -8, 30)
        );
    }

    #[test]
    fn a_position_between_two_studs_rounds_to_the_nearer_one() {
        assert_eq!(
            snap_to_lattice(IVec3::new(3, 0, 7), &brick_2x4(), Orientation::DEFAULT),
            IVec3::ZERO
        );
        assert_eq!(
            snap_to_lattice(IVec3::new(17, 0, -23), &brick_2x4(), Orientation::DEFAULT),
            IVec3::new(20, 0, -20)
        );
    }

    #[test]
    fn a_position_exactly_between_two_studs_rounds_the_same_way_every_time() {
        // Which way hardly matters; that it is always the same way does,
        // because the same sentence must always build the same model.
        let up = snap_to_lattice(IVec3::new(10, 0, 10), &brick_2x4(), Orientation::DEFAULT);
        assert_eq!(up, IVec3::new(20, 0, 20));
        assert_eq!(
            snap_to_lattice(IVec3::new(-10, 0, -10), &brick_2x4(), Orientation::DEFAULT),
            IVec3::ZERO
        );
    }

    #[test]
    fn a_height_between_plates_rounds_because_a_stud_is_not_a_plate() {
        // `Move it up 1 stud.` produces 20, which is neither a plate nor a
        // brick and attaches to nothing. It sits exactly between two plates
        // (16) and one brick (24), so the tie rule picks the lower of the two.
        assert_eq!(
            snap_to_lattice(IVec3::new(0, -20, 0), &brick_2x4(), Orientation::DEFAULT).y,
            -16
        );
        // Off a tie there is nothing to argue about.
        assert_eq!(
            snap_to_lattice(IVec3::new(0, -22, 0), &brick_2x4(), Orientation::DEFAULT).y,
            -24
        );
        assert_eq!(
            snap_to_lattice(IVec3::new(0, -4, 0), &brick_2x4(), Orientation::DEFAULT).y,
            0
        );
    }

    #[test]
    fn turning_a_part_a_quarter_swaps_which_grid_residue_it_needs() {
        let part = PartRef::new("3004", "Brick 1 x 2", (1, 2), PartKind::Brick);
        assert_eq!(lattice_residue(&part, Orientation::DEFAULT), (10, 0));
        assert_eq!(lattice_residue(&part, Orientation::new(1, false)), (0, 10));
    }

    /// A brick-height box `studs` wide and deep, centred on `x`/`z`, whose
    /// bottom face is at `y`.
    fn brick_box(x: i32, y: i32, z: i32, studs: i32) -> Box3 {
        let half = studs * HALF_STUD;
        (
            IVec3::new(x - half, y - 24, z - half),
            IVec3::new(x + half, y, z + half),
        )
    }

    #[test]
    fn a_placement_on_the_ground_plane_is_already_settled() {
        let batch = [brick_box(0, 0, 0, 2)];
        assert_eq!(settle(&[], &batch, SupportPolicy::Every), Ok(0));
    }

    #[test]
    fn a_placement_floating_over_a_brick_comes_down_onto_it() {
        let rest = [brick_box(0, 0, 0, 2)];
        // Two bricks up in the air over it; the only thing to land on is its
        // top face at -24, so it falls one brick.
        let batch = [brick_box(0, -48, 0, 2)];
        assert_eq!(settle(&rest, &batch, SupportPolicy::Every), Ok(24));
    }

    #[test]
    fn a_placement_floating_over_nothing_comes_down_to_the_ground() {
        let batch = [brick_box(0, -96, 0, 2)];
        assert_eq!(settle(&[], &batch, SupportPolicy::Every), Ok(96));
    }

    #[test]
    fn a_placement_landing_inside_another_is_lifted_onto_its_top_face() {
        let rest = [brick_box(0, 0, 0, 2)];
        // Exactly on top of the existing brick: the two would occupy the same
        // volume. The nearest rest level is that brick's top face.
        let batch = [brick_box(0, 0, 0, 2)];
        assert_eq!(settle(&rest, &batch, SupportPolicy::Every), Ok(-24));
    }

    #[test]
    fn a_rest_level_that_is_already_occupied_is_passed_over_for_the_next() {
        // A tower of two bricks, and a part hovering inside the upper one. The
        // nearest rest level is the lower brick's top face — but the upper
        // brick is sitting on it, so the part carries on up to the very top.
        let rest = [brick_box(0, 0, 0, 2), brick_box(0, -24, 0, 2)];
        let batch = [brick_box(0, -36, 0, 2)];
        let delta = settle(&rest, &batch, SupportPolicy::Every).unwrap();
        assert_eq!(delta, -12, "should have landed on the top of the tower");
        assert_eq!(shift(batch[0], delta).1.y, -48);
    }

    #[test]
    fn settling_never_puts_anything_below_the_ground_plane() {
        // `under` a ground-level brick asks for a position at +24. There is no
        // room down there, so the only legal answer is back up to the ground.
        let rest = [brick_box(0, 0, 0, 2)];
        let batch = [brick_box(0, 24, 0, 2)];
        let delta = settle(&rest, &batch, SupportPolicy::Every).unwrap();
        assert!(
            24 + delta <= 0,
            "settled to y={}, which is underground",
            24 + delta
        );
    }

    #[test]
    fn a_batch_settles_as_one_body_so_a_wall_keeps_its_shape() {
        // Two courses, the upper resting on the lower, floating together.
        let batch = [brick_box(0, -72, 0, 2), brick_box(0, -96, 0, 2)];
        let delta = settle(&[], &batch, SupportPolicy::Batch).unwrap();
        assert_eq!(delta, 72);
        // The gap between the courses is untouched: they moved together.
        let moved: Vec<Box3> = batch.iter().map(|b| shift(*b, delta)).collect();
        assert_eq!(moved[0].1.y, 0);
        assert_eq!(moved[1].1.y, -24);
    }

    #[test]
    fn a_batch_may_overhang_what_it_rests_on() {
        // The documented example session builds an 8-stud wall on an 8x8
        // plate, and the far end hangs off it. That is a real thing people
        // build, so a batch only needs something under part of it.
        let rest = [(IVec3::new(-80, -8, -80), IVec3::new(80, 0, 80))];
        let batch = [brick_box(0, -8, 0, 2), brick_box(120, -8, 0, 2)];
        assert_eq!(settle(&rest, &batch, SupportPolicy::Batch), Ok(0));
    }

    #[test]
    fn a_batch_with_nowhere_free_to_rest_is_reported_rather_than_guessed() {
        // Two parts far apart vertically and not over each other. One rigid
        // shift cannot bring both to rest: whatever grounds the high one
        // buries the low one.
        let batch = [brick_box(0, 0, 0, 2), brick_box(200, -100, 0, 2)];
        assert_eq!(
            settle(&[], &batch, SupportPolicy::Every),
            Err(LowerError::Unsupported(Unsupported::NoLegalSpot))
        );
        // As a batch it is fine: the structure keeps its shape and rests on
        // what it can, which is how an overhang gets built.
        assert_eq!(settle(&[], &batch, SupportPolicy::Batch), Ok(0));
    }

    #[test]
    fn a_lift_that_was_asked_for_is_left_where_it_was_put() {
        // `Raise by one plate.` must not fall straight back down: the builder
        // may be about to slide something underneath.
        let batch = [brick_box(0, -8, 0, 2)];
        assert_eq!(settle(&[], &batch, SupportPolicy::KeepHeight), Ok(0));
        // Burial is still corrected, though, because there is no down there.
        let buried = [brick_box(0, 48, 0, 2)];
        let delta = settle(&[], &buried, SupportPolicy::KeepHeight).unwrap();
        assert_eq!(48 + delta, 0);
    }

    fn add_at(pos: IVec3) -> Op {
        Op::Add {
            part: brick_2x4(),
            color: 4,
            pos,
            orientation: Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    /// A model holding the placements an operation would have created.
    fn model_with(ops: &[Op]) -> Model {
        let mut model = Model::new("t");
        for (rev, op) in ops.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            apply(&mut model, rev as u32, op);
        }
        model
    }

    fn pos_of(op: &Op) -> IVec3 {
        match op {
            Op::Add { pos, .. } => *pos,
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn legalizing_a_correct_operation_returns_it_unchanged() {
        let op = add_at(IVec3::ZERO);
        let legalized = legalize(&Model::new("t"), 0, &op).unwrap();
        assert_eq!(legalized.op, op);
        assert!(legalized.corrections.is_empty());
        assert!(legalized.adjustment.is_none());
    }

    #[test]
    fn legalizing_rewrites_the_operation_so_the_history_is_legal_too() {
        // Correcting only the model would leave the recorded operation wrong,
        // and replay would bring the illegal position straight back.
        let legalized = legalize(&Model::new("t"), 0, &add_at(IVec3::new(3, 5, 7))).unwrap();
        assert_eq!(pos_of(&legalized.op), IVec3::ZERO);
        assert_eq!(legalized.adjustment.snapped, 1);
    }

    #[test]
    fn legalizing_a_second_part_in_the_same_place_stacks_it() {
        let model = model_with(&[add_at(IVec3::ZERO)]);
        let legalized = legalize(&model, 1, &add_at(IVec3::ZERO)).unwrap();
        assert_eq!(pos_of(&legalized.op), IVec3::new(0, -24, 0));
        assert_eq!(legalized.adjustment.settled, -24);
    }

    #[test]
    fn legalizing_a_repeated_add_snaps_the_step_as_well_as_the_start() {
        // A step off the lattice walks every later copy off it too, so the
        // step needs correcting even when the starting point is already right.
        let op = Op::Add {
            part: brick_2x4(),
            color: 4,
            pos: IVec3::ZERO,
            orientation: Orientation::DEFAULT,
            repeat: Some(Repeat {
                count: 3,
                step: IVec3::new(43, 0, 0),
            }),
            group: None,
        };
        let legalized = legalize(&Model::new("t"), 0, &op).unwrap();
        match &legalized.op {
            Op::Add { repeat, .. } => assert_eq!(repeat.unwrap().step, IVec3::new(40, 0, 0)),
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn legalizing_a_move_keeps_one_offset_for_every_target() {
        // `Move it east 2 plates.` measures sideways in a vertical unit and
        // lands 16 across, which is not a stud.
        let model = model_with(&[add_at(IVec3::ZERO), add_at(IVec3::new(0, -24, 0))]);
        let targets: Vec<PlacementId> = model.placements.iter().map(|p| p.id).collect();
        let op = Op::Move {
            targets: targets.clone(),
            offset: IVec3::new(16, 0, 0),
        };
        let legalized = legalize(&model, 2, &op).unwrap();
        match &legalized.op {
            Op::Move { targets: t, offset } => {
                assert_eq!(*offset, IVec3::new(20, 0, 0));
                assert_eq!(*t, targets, "every target still moves together");
            }
            other => panic!("not a move: {other:?}"),
        }
        assert!(legalized.corrections.is_empty());
    }

    #[test]
    fn legalizing_a_lift_leaves_it_lifted() {
        // `Raise by one plate.` is deliberate. Settling it back down would
        // make the verb useless.
        let model = model_with(&[add_at(IVec3::ZERO)]);
        let op = Op::Move {
            targets: vec![model.placements[0].id],
            offset: IVec3::new(0, -8, 0),
        };
        let legalized = legalize(&model, 1, &op).unwrap();
        assert_eq!(legalized.op, op);
        assert!(legalized.adjustment.is_none());
    }

    #[test]
    fn legalizing_a_rotate_adds_the_shift_its_new_footprint_needs() {
        // A 1x2 turned a quarter swaps which grid residue it needs, so the
        // rotate alone would leave it half a stud off.
        let part = PartRef::new("3004", "Brick 1 x 2", (1, 2), PartKind::Brick);
        let model = model_with(&[Op::Add {
            part,
            color: 4,
            pos: IVec3::new(10, 0, 0),
            orientation: Orientation::DEFAULT,
            repeat: None,
            group: None,
        }]);
        let id = model.placements[0].id;
        let op = Op::Rotate {
            targets: vec![id],
            turns: 1,
            flip: false,
        };
        let legalized = legalize(&model, 1, &op).unwrap();
        assert_eq!(legalized.op, op, "the rotate itself is untouched");
        // Turning an odd-by-even part about its centre genuinely moves which
        // studs it can reach, so it has to shift half a stud on both axes.
        // Both land on a tie, and the tie rule sends both the same way.
        assert_eq!(
            legalized.corrections,
            vec![Op::Move {
                targets: vec![id],
                offset: IVec3::new(10, 0, 10),
            }]
        );
    }

    #[test]
    fn legalizing_a_paint_or_a_rename_is_the_identity() {
        let model = model_with(&[add_at(IVec3::ZERO)]);
        let id = model.placements[0].id;
        for op in [
            Op::Paint {
                targets: vec![id],
                color: 1,
            },
            Op::Rename {
                name: "castle".into(),
            },
            Op::Remove { targets: vec![id] },
        ] {
            let legalized = legalize(&model, 1, &op).unwrap();
            assert_eq!(legalized.op, op);
            assert!(legalized.adjustment.is_none());
        }
    }

    #[test]
    fn a_part_of_unknown_size_is_held_to_the_grid_like_a_single_stud() {
        let unknown = PartRef::new("99999", "Mystery", (0, 0), PartKind::Other);
        assert_eq!(lattice_residue(&unknown, Orientation::DEFAULT), (10, 10));
        assert_eq!(
            snap_to_lattice(IVec3::new(3, 5, 7), &unknown, Orientation::DEFAULT),
            IVec3::new(10, 8, 10)
        );
    }
}

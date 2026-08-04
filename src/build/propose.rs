//! Working out where a part could plausibly go, so the builder can choose.
//!
//! Naming a part without saying where it goes — `2x2 brick` — is a question,
//! not a command. This module answers it with a handful of ranked, legal
//! placements; `session` shows them and the builder picks one.
//!
//! **No geometry is decided here.** Every candidate is put on the grid and
//! settled by [`super::legal`], so a proposal is legal by exactly the same
//! rules as anything typed out in full. All this module does is enumerate the
//! plausible anchors and rank them.
//!
//! A *batch* is the unit of work: a `Vec<Placed>` whose first member is the
//! reference point everything else is measured from. One brick, two bricks
//! stacked, and a whole drawn block are all batches, which is why one engine
//! serves the bare-part sentence, a location-less drawing and the drafting
//! loop alike.

use super::dsl::ast::Arrangement;
use super::legal::{self, SupportPolicy};
use super::model::{Model, PartRef, Placement, PlacementId, part_box};
use super::ops::Placed;
use super::units::{Direction, IVec3, LDU_STUD};

/// How many exposed surfaces are worth considering before ranking. A big model
/// has a lot of flat top faces and all but the nearest few are noise.
const SURFACE_SCAN: usize = 8;

/// Why a placement was offered, which is also how it gets described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The model is empty, so it goes at the origin.
    Ground,
    /// On top of whatever was last touched — what a bare sentence does today.
    OnLastTouched,
    /// Alongside the last-touched part.
    Beside { direction: Direction },
    /// On top of some other exposed surface.
    OnSurface { under: PlacementId },
    /// On the ground, clear of everything already built.
    GroundBeside { direction: Direction },
}

impl Origin {
    /// Ordering used to break ties, so the same model always proposes the same
    /// list in the same order.
    const fn rank(self) -> u8 {
        match self {
            Self::Ground => 0,
            Self::OnLastTouched => 1,
            Self::Beside { .. } => 2,
            Self::OnSurface { .. } => 3,
            Self::GroundBeside { .. } => 4,
        }
    }

    /// Localization key of the sentence describing this proposal.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Ground => "command.build.propose_ground",
            Self::OnLastTouched => "command.build.propose_on_last",
            Self::Beside { .. } => "command.build.propose_beside",
            Self::OnSurface { .. } => "command.build.propose_on_surface",
            Self::GroundBeside { .. } => "command.build.propose_ground_beside",
        }
    }

    /// The direction this proposal stepped in, when it stepped at all.
    #[must_use]
    pub const fn direction(self) -> Option<Direction> {
        match self {
            Self::Beside { direction } | Self::GroundBeside { direction } => Some(direction),
            _ => None,
        }
    }
}

/// One place the batch could go, already legal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub placements: Vec<Placed>,
    pub origin: Origin,
    pub score: i32,
}

/// Ranked, deduplicated, legal places `batch` could go — best first.
///
/// Empty only when the model leaves nowhere at all, which is rare enough that
/// callers can treat it as "say you couldn't".
#[must_use]
pub fn propose(model: &Model, batch: &[Placed], limit: usize) -> Vec<Candidate> {
    if batch.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let focus = focus_point(model);

    for (translation, origin) in anchors(model, batch) {
        let moved: Vec<Placed> = batch
            .iter()
            .map(|placed| Placed {
                pos: placed.pos.add(translation),
                ..placed.clone()
            })
            .collect();
        // Legality is `legal`'s decision, not ours. A candidate it cannot make
        // legal is simply not offered.
        let Ok(settled) = legal::settle_placements(model, &moved, SupportPolicy::Batch) else {
            continue;
        };
        // Two generators often arrive at the same spot — beside a lone brick
        // and on the ground beside the model, say. Offer it once.
        if candidates.iter().any(|c| c.placements == settled) {
            continue;
        }
        let score = score(origin, &settled, focus);
        candidates.push(Candidate {
            placements: settled,
            origin,
            score,
        });
    }

    candidates.sort_by_key(|c| (-c.score, c.origin.rank()));
    candidates.truncate(limit);
    candidates
}

/// Proposals for several shapes at once, ranked against each other.
///
/// `2 2x2 bricks` says nothing about whether they go up or along, so the
/// arrangements compete for the same few slots as the positions do: the answer
/// to "where?" and "how?" is one list, not two questions in a row.
#[must_use]
pub fn propose_any(model: &Model, batches: &[Vec<Placed>], limit: usize) -> Vec<Candidate> {
    let mut all: Vec<Candidate> = Vec::new();
    for batch in batches {
        for candidate in propose(model, batch, limit) {
            if !all.iter().any(|c| c.placements == candidate.placements) {
                all.push(candidate);
            }
        }
    }
    all.sort_by_key(|c| (-c.score, c.origin.rank()));
    all.truncate(limit);
    all
}

/// Every translation worth trying, in a fixed order.
fn anchors(model: &Model, batch: &[Placed]) -> Vec<(IVec3, Origin)> {
    let reference = batch[0].pos;
    let Some(bounds) = batch_bounds(batch) else {
        return Vec::new();
    };

    if model.is_empty() {
        return vec![(IVec3::ZERO.sub(reference), Origin::Ground)];
    }

    let mut out = Vec::new();

    // On top of, and alongside, whatever was last touched — the thing the
    // builder is most likely still thinking about.
    if let Some(last) = model.last_ref.last().and_then(|id| model.get(*id)) {
        out.push((last.stack_position().sub(reference), Origin::OnLastTouched));
        for direction in [
            Direction::East,
            Direction::West,
            Direction::South,
            Direction::North,
        ] {
            let gap = span_along(last.occupied_box(), direction)
                .midpoint(span_along(bounds, direction))
                .max(LDU_STUD);
            out.push((
                last.pos.add(direction.step().scale(gap)).sub(reference),
                Origin::Beside { direction },
            ));
        }
    }

    // Other flat tops, nearest first.
    let focus = focus_point(model);
    let mut surfaces: Vec<&Placement> = model
        .placements
        .iter()
        .filter(|p| is_exposed_on_top(model, p))
        .collect();
    surfaces.sort_by_key(|p| (manhattan(p.pos, focus), p.id));
    for placement in surfaces.into_iter().take(SURFACE_SCAN) {
        out.push((
            placement.stack_position().sub(reference),
            Origin::OnSurface {
                under: placement.id,
            },
        ));
    }

    // Beside the whole model, on the ground — where a second structure starts.
    if let Some((min, max)) = model.bounds() {
        for direction in [
            Direction::East,
            Direction::West,
            Direction::South,
            Direction::North,
        ] {
            // Clear of the model's footprint by a stud, with the batch's own
            // bottom face on the ground.
            let mut translation = IVec3::new(0, -bounds.1.y, 0);
            match direction {
                Direction::East => translation.x = max.x + LDU_STUD - bounds.0.x,
                Direction::West => translation.x = min.x - LDU_STUD - bounds.1.x,
                Direction::South => translation.z = max.z + LDU_STUD - bounds.0.z,
                Direction::North => translation.z = min.z - LDU_STUD - bounds.1.z,
                Direction::Up | Direction::Down => continue,
            }
            out.push((translation, Origin::GroundBeside { direction }));
        }
    }

    out
}

/// How good a proposal looks, before the builder sees it.
///
/// Nearness to whatever is being worked on dominates: the answer wanted is
/// almost always next to the last thing built, and a legal spot on the far
/// side of a large model is technically right and useless.
fn score(origin: Origin, placements: &[Placed], focus: IVec3) -> i32 {
    let base = match origin {
        Origin::Ground | Origin::OnLastTouched => 100,
        Origin::Beside { .. } => 80,
        Origin::OnSurface { .. } => 60,
        Origin::GroundBeside { .. } => 40,
    };
    let distance = placements
        .first()
        .map_or(0, |p| manhattan(p.pos, focus) / LDU_STUD);
    base - distance.min(30)
}

/// What the builder is most likely looking at: the last part touched, else the
/// middle of the model.
fn focus_point(model: &Model) -> IVec3 {
    model
        .last_ref
        .last()
        .and_then(|id| model.get(*id))
        .map_or_else(
            || {
                model.bounds().map_or(IVec3::ZERO, |(min, max)| {
                    IVec3::new(
                        min.x.midpoint(max.x),
                        min.y.midpoint(max.y),
                        min.z.midpoint(max.z),
                    )
                })
            },
            |p| p.pos,
        )
}

/// True when nothing sits directly on this placement's top face.
fn is_exposed_on_top(model: &Model, placement: &Placement) -> bool {
    let (min_a, _) = placement.occupied_box();
    !model.placements.iter().any(|other| {
        if other.id == placement.id {
            return false;
        }
        let (min_b, max_b) = other.occupied_box();
        max_b.y == min_a.y
            && min_a.x < max_b.x
            && placement.occupied_box().1.x > min_b.x
            && min_a.z < max_b.z
            && placement.occupied_box().1.z > min_b.z
    })
}

/// The box the whole batch occupies, in its own coordinates.
fn batch_bounds(batch: &[Placed]) -> Option<(IVec3, IVec3)> {
    batch
        .iter()
        .map(|placed| part_box(&placed.part, placed.orientation, placed.pos))
        .reduce(|(lo, hi), (min, max)| {
            (
                IVec3::new(lo.x.min(min.x), lo.y.min(min.y), lo.z.min(min.z)),
                IVec3::new(hi.x.max(max.x), hi.y.max(max.y), hi.z.max(max.z)),
            )
        })
}

/// How far a box reaches along a direction. Vertical directions have no
/// horizontal extent, so they step by nothing.
const fn span_along(bounds: (IVec3, IVec3), direction: Direction) -> i32 {
    match direction {
        Direction::East | Direction::West => bounds.1.x - bounds.0.x,
        Direction::North | Direction::South => bounds.1.z - bounds.0.z,
        Direction::Up | Direction::Down => 0,
    }
}

const fn manhattan(a: IVec3, b: IVec3) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

/// The batches a quantity of one part could mean, in local coordinates.
///
/// With no arrangement given, every shape is offered so they compete as
/// proposals — which is the honest answer to `2 2x2 bricks`, since nothing in
/// it says whether they go up or along.
#[must_use]
pub fn arrangements(
    part: &PartRef,
    color: u16,
    count: u32,
    want: Option<Arrangement>,
) -> Vec<(Arrangement, Vec<Placed>)> {
    let count = count.max(1);
    // One part has only one shape, whatever anybody says about it.
    if count == 1 {
        return vec![(Arrangement::Stacked, vec![member(part, color, IVec3::ZERO)])];
    }

    let wanted: Vec<Arrangement> = want.map_or_else(
        || {
            vec![
                Arrangement::Stacked,
                Arrangement::RowEast,
                Arrangement::RowSouth,
            ]
        },
        |one| vec![one],
    );

    let (w, d) = part.footprint_or_default();
    wanted
        .into_iter()
        .map(|arrangement| {
            let step = match arrangement {
                Arrangement::Stacked => IVec3::new(0, -part.kind.height_ldu(), 0),
                Arrangement::RowEast => IVec3::new(i32::from(w) * LDU_STUD, 0, 0),
                Arrangement::RowSouth => IVec3::new(0, 0, i32::from(d) * LDU_STUD),
            };
            let placements = (0..count)
                .map(|i| {
                    #[allow(clippy::cast_possible_wrap)]
                    member(part, color, step.scale(i as i32))
                })
                .collect();
            (arrangement, placements)
        })
        .collect()
}

fn member(part: &PartRef, color: u16, pos: IVec3) -> Placed {
    Placed {
        part: part.clone(),
        color,
        pos,
        orientation: super::units::Orientation::DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::PartKind;
    use crate::build::ops::{Op, apply};

    fn brick_2x4() -> PartRef {
        PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
    }

    fn brick_2x2() -> PartRef {
        PartRef::new("3003", "Brick 2 x 2", (2, 2), PartKind::Brick)
    }

    /// A one-part batch at the local origin — what a bare `2x2 brick` means.
    fn one(part: &PartRef) -> Vec<Placed> {
        vec![member(part, 4, IVec3::ZERO)]
    }

    /// A model holding bricks at the given positions, the last one touched.
    fn model_with(positions: &[IVec3]) -> Model {
        let mut model = Model::new("t");
        for (rev, pos) in positions.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            apply(
                &mut model,
                rev as u32,
                &Op::Add {
                    part: brick_2x4(),
                    color: 4,
                    pos: *pos,
                    orientation: super::super::units::Orientation::DEFAULT,
                    repeat: None,
                    group: None,
                },
            );
        }
        model
    }

    /// Everything a proposal must never get wrong.
    fn assert_legal(model: &Model, candidate: &Candidate) {
        for placed in &candidate.placements {
            assert!(
                legal::is_on_lattice(placed.pos, &placed.part, placed.orientation),
                "{:?} is off the stud grid",
                placed.pos
            );
            assert!(placed.pos.y <= 0, "{:?} is underground", placed.pos);
            let mine = legal::placed_box(placed);
            for existing in &model.placements {
                assert!(
                    !crate::build::model::boxes_overlap(mine, existing.occupied_box()),
                    "{:?} is inside an existing part",
                    placed.pos
                );
            }
        }
    }

    #[test]
    fn an_empty_model_proposes_the_origin() {
        let proposals = propose(&Model::new("t"), &one(&brick_2x4()), 4);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].origin, Origin::Ground);
        assert_eq!(proposals[0].placements[0].pos, IVec3::ZERO);
    }

    #[test]
    fn the_first_proposal_is_on_top_of_what_was_last_touched() {
        // The conversational default has to stay the default: someone who just
        // wants it stacked should find that offer first.
        let model = model_with(&[IVec3::ZERO]);
        let proposals = propose(&model, &one(&brick_2x4()), 4);
        assert_eq!(proposals[0].origin, Origin::OnLastTouched);
        assert_eq!(proposals[0].placements[0].pos, IVec3::new(0, -24, 0));
    }

    #[test]
    fn every_exposed_face_of_the_last_part_is_offered() {
        let model = model_with(&[IVec3::ZERO]);
        let proposals = propose(&model, &one(&brick_2x4()), 8);
        let directions: Vec<Direction> = proposals
            .iter()
            .filter_map(|c| c.origin.direction())
            .collect();
        for wanted in [
            Direction::East,
            Direction::West,
            Direction::North,
            Direction::South,
        ] {
            assert!(
                directions.contains(&wanted),
                "nothing offered to the {wanted:?}, got {directions:?}"
            );
        }
    }

    #[test]
    fn every_proposal_is_a_legal_placement() {
        // The whole point: the dialog can never offer somewhere illegal.
        let model = model_with(&[IVec3::ZERO, IVec3::new(0, -24, 0), IVec3::new(80, 0, 0)]);
        for batch in [one(&brick_2x4()), one(&brick_2x2())] {
            let proposals = propose(&model, &batch, 8);
            assert!(!proposals.is_empty(), "nothing was proposed at all");
            for candidate in proposals {
                assert_legal(&model, &candidate);
            }
        }
    }

    #[test]
    fn a_batch_keeps_its_shape_in_every_proposal() {
        let model = model_with(&[IVec3::ZERO]);
        let (_, batch) = arrangements(&brick_2x2(), 4, 3, Some(Arrangement::Stacked))
            .into_iter()
            .next()
            .unwrap();
        let proposals = propose(&model, &batch, 6);
        assert!(!proposals.is_empty(), "nothing was proposed at all");
        for candidate in proposals {
            assert_eq!(candidate.placements.len(), 3);
            // Still one on top of the next, whatever happened to the whole.
            let mut ys: Vec<i32> = candidate.placements.iter().map(|p| p.pos.y).collect();
            ys.sort_unstable();
            assert_eq!(ys[1] - ys[0], 24);
            assert_eq!(ys[2] - ys[1], 24);
            assert_legal(&model, &candidate);
        }
    }

    #[test]
    fn two_proposals_that_land_in_the_same_place_are_offered_once() {
        let model = model_with(&[IVec3::ZERO]);
        let proposals = propose(&model, &one(&brick_2x4()), 16);
        let mut seen: Vec<Vec<IVec3>> = proposals
            .iter()
            .map(|c| c.placements.iter().map(|p| p.pos).collect())
            .collect();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), before, "the same spot was offered twice");
    }

    #[test]
    fn proposals_are_ranked_the_same_way_every_time() {
        let model = model_with(&[IVec3::ZERO, IVec3::new(80, 0, 0)]);
        let once = propose(&model, &one(&brick_2x4()), 4);
        let twice = propose(&model, &one(&brick_2x4()), 4);
        assert_eq!(once, twice);
        // And ordered best first.
        assert!(once.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn a_second_structure_is_offered_its_own_ground() {
        let model = model_with(&[IVec3::ZERO]);
        let proposals = propose(&model, &one(&brick_2x4()), 16);
        assert!(
            proposals
                .iter()
                .any(|c| matches!(c.origin, Origin::GroundBeside { .. })),
            "nowhere offered to start something new"
        );
    }

    #[test]
    fn batch_proposals_are_anchored_from_their_centre() {
        let model = model_with(&[IVec3::ZERO]); // The reference part
        let mut batch = one(&brick_2x2());
        // Add a second part to the batch to make its center different from its first part
        batch.push(member(&brick_2x2(), 4, IVec3::new(40, 0, 0)));
        let proposals = propose(&model, &batch, 16);

        let beside_east = proposals
            .iter()
            .find(|c| {
                matches!(
                    c.origin,
                    Origin::Beside {
                        direction: crate::build::units::Direction::East
                    }
                )
            })
            .unwrap();
        // 2x4 at origin goes from -40 to 40 on X.
        // Batch is two 2x2s at 0 and 40, so it goes from -20 to 60. Center is 20. Width is 80.
        // Gap should be (80 + 80) / 2 = 80. Target center = 80. Translation = 80 - 20 = 60.
        // First part at 0 -> 60. Second part at 40 -> 100.
        assert_eq!(beside_east.placements[0].pos, IVec3::new(60, 0, 0));
        assert_eq!(beside_east.placements[1].pos, IVec3::new(100, 0, 0));
    }

    #[test]
    fn a_bare_quantity_offers_stacked_and_side_by_side() {
        let shapes = arrangements(&brick_2x2(), 4, 2, None);
        assert_eq!(shapes.len(), 3);
        let kinds: Vec<Arrangement> = shapes.iter().map(|(a, _)| *a).collect();
        assert!(kinds.contains(&Arrangement::Stacked));
        assert!(kinds.contains(&Arrangement::RowEast));
        assert!(kinds.contains(&Arrangement::RowSouth));
        // Stacked really is stacked: one brick height apart.
        let stacked = &shapes[0].1;
        assert_eq!(stacked[1].pos, IVec3::new(0, -24, 0));
    }

    #[test]
    fn saying_stacked_narrows_the_arrangements_to_one() {
        let shapes = arrangements(&brick_2x2(), 4, 3, Some(Arrangement::RowEast));
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].0, Arrangement::RowEast);
        assert_eq!(shapes[0].1[2].pos, IVec3::new(80, 0, 0));
    }

    #[test]
    fn a_single_part_has_exactly_one_arrangement() {
        // Nothing to arrange, so nothing to ask about.
        assert_eq!(arrangements(&brick_2x2(), 4, 1, None).len(), 1);
        assert_eq!(
            arrangements(&brick_2x2(), 4, 1, Some(Arrangement::RowEast)).len(),
            1
        );
    }
}

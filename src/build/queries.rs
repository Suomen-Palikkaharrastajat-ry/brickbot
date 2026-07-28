//! Answering questions about the model.
//!
//! Queries never change anything, so they are pure functions of the model and
//! its history and need no operations at all.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::Query;
use super::lower::{predicate_of, resolve_ref, LowerError};
use super::model::{Model, PartRef, PlacementId};
use super::ops::History;
use super::units::IVec3;

/// The result of a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryAnswer {
    Height {
        ldu: i32,
        bricks: i32,
        plates: i32,
    },
    Count(usize),
    Collisions(Vec<(PlacementId, PlacementId)>),
    Unsupported(Vec<PlacementId>),
    Position(IVec3),
    Author(String),
    /// The most recent edit, described, or `None` when nothing has happened.
    Changed(Option<String>),
    /// Part, colour code and quantity, sorted for stable output.
    Inventory(Vec<(PartRef, u16, usize)>),
    NothingToReport,
}

/// The bounding box of a subset of placements, if any of them exist.
fn subset_bounds(model: &Model, ids: &[PlacementId]) -> Option<(IVec3, IVec3)> {
    let mut result: Option<(IVec3, IVec3)> = None;
    for placement in ids.iter().filter_map(|id| model.get(*id)) {
        let (min, max) = placement
            .occupied_box()
            .unwrap_or((placement.pos, placement.pos));
        result = Some(match result {
            None => (min, max),
            Some((lo, hi)) => (
                IVec3::new(lo.x.min(min.x), lo.y.min(min.y), lo.z.min(min.z)),
                IVec3::new(hi.x.max(max.x), hi.y.max(max.y), hi.z.max(max.z)),
            ),
        });
    }
    result
}

const fn height_answer(ldu: i32) -> QueryAnswer {
    QueryAnswer::Height {
        ldu,
        bricks: ldu / super::units::LDU_BRICK,
        plates: ldu / super::units::LDU_PLATE,
    }
}

/// Summarises the model as a parts list, grouped by part and colour.
///
/// Sorted by quantity then part number so the output is stable regardless of
/// the order things were built in.
#[must_use]
pub fn inventory(model: &Model) -> Vec<(PartRef, u16, usize)> {
    let mut counts: Vec<(PartRef, u16, usize)> = Vec::new();
    for placement in &model.placements {
        if let Some(entry) = counts
            .iter_mut()
            .find(|(part, color, _)| part.ldraw_id == placement.part.ldraw_id && *color == placement.color)
        {
            entry.2 += 1;
        } else {
            counts.push((placement.part.clone(), placement.color, 1));
        }
    }
    counts.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.ldraw_id.cmp(&b.0.ldraw_id)));
    counts
}

/// Answers a question about the model.
pub fn answer(
    model: &Model,
    history: &History,
    query: &Query,
) -> Result<QueryAnswer, LowerError> {
    match query {
        Query::HowTall(reference) => match reference {
            None => Ok(height_answer(model.height_ldu())),
            Some(reference) => {
                let ids = resolve_ref(model, reference)?;
                let ldu = subset_bounds(model, &ids).map_or(0, |(min, max)| max.y - min.y);
                Ok(height_answer(ldu))
            }
        },

        Query::Count(spec) => {
            let predicate = predicate_of(spec);
            Ok(QueryAnswer::Count(
                model.count_matching(|p| predicate.matches(p, model)),
            ))
        }

        Query::FindCollisions => {
            let collisions = model.collisions();
            if collisions.is_empty() {
                return Ok(QueryAnswer::NothingToReport);
            }
            Ok(QueryAnswer::Collisions(collisions))
        }

        Query::ShowUnsupported => {
            let floating = model.unsupported();
            if floating.is_empty() {
                return Ok(QueryAnswer::NothingToReport);
            }
            Ok(QueryAnswer::Unsupported(floating))
        }

        Query::WhereIs(reference) => {
            let ids = resolve_ref(model, reference)?;
            let (min, max) = subset_bounds(model, &ids).ok_or(LowerError::NothingToActOn)?;
            // The centre of whatever was named, in LDraw units.
            Ok(QueryAnswer::Position(IVec3::new(
                min.x.midpoint(max.x),
                min.y.midpoint(max.y),
                min.z.midpoint(max.z),
            )))
        }

        Query::WhoAdded(reference) => {
            let ids = resolve_ref(model, reference)?;
            let first = ids.first().ok_or(LowerError::NothingToActOn)?;
            history
                .author_of(*first)
                .map(|author| QueryAnswer::Author(author.to_string()))
                .ok_or(LowerError::NothingToActOn)
        }

        Query::WhatChanged => Ok(QueryAnswer::Changed(
            history.last().map(|entry| describe_op(&entry.op)),
        )),

        Query::ListParts => {
            let parts = inventory(model);
            if parts.is_empty() {
                return Ok(QueryAnswer::NothingToReport);
            }
            Ok(QueryAnswer::Inventory(parts))
        }
    }
}

/// A short, language-neutral description of an operation, for "what changed?".
#[must_use]
pub fn describe_op(op: &super::ops::Op) -> String {
    use super::ops::Op;
    match op {
        Op::Add { part, .. } => format!("added {}", part.name),
        Op::AddMany { placements, group } => {
            let what = group.as_deref().unwrap_or("parts");
            format!("added {} {what}", placements.len())
        }
        Op::Move { targets, .. } => format!("moved {} part(s)", targets.len()),
        Op::Remove { targets } => format!("removed {} part(s)", targets.len()),
        Op::Paint { targets, .. } => format!("repainted {} part(s)", targets.len()),
        Op::Rotate { targets, .. } => format!("rotated {} part(s)", targets.len()),
        Op::Replace { targets, part } => {
            format!("replaced {} part(s) with {}", targets.len(), part.name)
        }
        Op::Select { .. } => "changed the selection".to_string(),
        Op::Group { name, targets } => format!("grouped {} part(s) as {name}", targets.len()),
        Op::Landmark { name, .. } => format!("named the point {name}"),
        Op::Rename { name } => format!("renamed the model to {name}"),
    }
}

/// True when the answer says there is nothing worth reporting.
#[must_use]
pub const fn is_empty_answer(answer: &QueryAnswer) -> bool {
    matches!(answer, QueryAnswer::NothingToReport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::dsl::ast::{PartSpec, Ref};
    use crate::build::model::{PartKind, Placement};
    use crate::build::ops::{apply, Op};
    use crate::build::units::Orientation;

    fn brick() -> PartRef {
        PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
    }

    fn add(color: u16, pos: IVec3) -> Op {
        Op::Add {
            part: brick(),
            color,
            pos,
            orientation: Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    /// A two-brick stack built by two different people.
    fn stacked() -> (Model, History) {
        let mut history = History::new("t");
        history.push(add(4, IVec3::ZERO), "alice");
        history.push(add(14, IVec3::new(0, -24, 0)), "bob");
        (history.materialize(), history)
    }

    #[test]
    fn reports_height_in_bricks_and_plates() {
        let (model, history) = stacked();
        assert_eq!(
            answer(&model, &history, &Query::HowTall(None)).unwrap(),
            QueryAnswer::Height {
                ldu: 48,
                bricks: 2,
                plates: 6,
            }
        );
    }

    #[test]
    fn reports_the_height_of_a_named_group() {
        let mut history = History::new("t");
        history.push(add(4, IVec3::ZERO), "alice");
        history.push(
            Op::Group {
                name: "tower".to_string(),
                targets: vec![PlacementId::new(0, 0)],
            },
            "alice",
        );
        history.push(add(4, IVec3::new(500, 0, 0)), "bob");
        let model = history.materialize();

        let tall = answer(
            &model,
            &history,
            &Query::HowTall(Some(Ref::Named("tower".to_string()))),
        )
        .unwrap();
        // Only the one brick in the group, not the whole model.
        assert_eq!(
            tall,
            QueryAnswer::Height {
                ldu: 24,
                bricks: 1,
                plates: 3,
            }
        );
    }

    #[test]
    fn counts_only_matching_parts() {
        let (model, history) = stacked();
        let red = Query::Count(Box::new(PartSpec {
            color: Some(4),
            quantity: 1,
            ..PartSpec::default()
        }));
        assert_eq!(
            answer(&model, &history, &red).unwrap(),
            QueryAnswer::Count(1)
        );

        let everything = Query::Count(Box::new(PartSpec {
            quantity: 1,
            ..PartSpec::default()
        }));
        assert_eq!(
            answer(&model, &history, &everything).unwrap(),
            QueryAnswer::Count(2)
        );
    }

    #[test]
    fn finds_overlapping_parts() {
        let mut history = History::new("t");
        history.push(add(4, IVec3::ZERO), "alice");
        history.push(add(14, IVec3::new(20, 0, 0)), "bob");
        let model = history.materialize();

        match answer(&model, &history, &Query::FindCollisions).unwrap() {
            QueryAnswer::Collisions(pairs) => assert_eq!(pairs.len(), 1),
            other => panic!("expected collisions, got {other:?}"),
        }
    }

    #[test]
    fn a_tidy_model_has_nothing_to_report() {
        let (model, history) = stacked();
        assert_eq!(
            answer(&model, &history, &Query::FindCollisions).unwrap(),
            QueryAnswer::NothingToReport
        );
        assert_eq!(
            answer(&model, &history, &Query::ShowUnsupported).unwrap(),
            QueryAnswer::NothingToReport
        );
    }

    #[test]
    fn reports_floating_bricks_as_unsupported() {
        let mut history = History::new("t");
        history.push(add(4, IVec3::ZERO), "alice");
        history.push(add(4, IVec3::new(0, -240, 0)), "bob");
        let model = history.materialize();

        match answer(&model, &history, &Query::ShowUnsupported).unwrap() {
            QueryAnswer::Unsupported(ids) => {
                assert_eq!(ids, vec![PlacementId::new(1, 0)]);
            }
            other => panic!("expected unsupported bricks, got {other:?}"),
        }
    }

    #[test]
    fn names_who_added_a_part() {
        let (model, history) = stacked();
        assert_eq!(
            answer(&model, &history, &Query::WhoAdded(Ref::Highest)).unwrap(),
            QueryAnswer::Author("bob".to_string())
        );
    }

    #[test]
    fn reports_where_something_is() {
        let mut history = History::new("t");
        history.push(add(4, IVec3::new(100, 0, 200)), "alice");
        let model = history.materialize();

        match answer(&model, &history, &Query::WhereIs(Ref::Last)).unwrap() {
            QueryAnswer::Position(pos) => {
                assert_eq!(pos.x, 100);
                assert_eq!(pos.z, 200);
            }
            other => panic!("expected a position, got {other:?}"),
        }
    }

    #[test]
    fn describes_the_most_recent_change() {
        let (model, history) = stacked();
        match answer(&model, &history, &Query::WhatChanged).unwrap() {
            QueryAnswer::Changed(Some(text)) => assert!(text.contains("added")),
            other => panic!("expected a change, got {other:?}"),
        }
    }

    #[test]
    fn inventory_groups_by_part_and_colour() {
        let mut model = Model::new("t");
        for i in 0..3 {
            apply(&mut model, i, &add(4, IVec3::new(i32::try_from(i).unwrap() * 100, 0, 0)));
        }
        apply(&mut model, 9, &add(14, IVec3::new(500, 0, 0)));

        let parts = inventory(&model);
        assert_eq!(parts.len(), 2, "one line per part-and-colour combination");
        // Sorted by quantity, so the three red bricks come first.
        assert_eq!(parts[0].1, 4);
        assert_eq!(parts[0].2, 3);
        assert_eq!(parts[1].1, 14);
        assert_eq!(parts[1].2, 1);
    }

    #[test]
    fn an_empty_model_answers_every_query_without_erroring() {
        let model = Model::new("empty");
        let history = History::new("empty");

        assert_eq!(
            answer(&model, &history, &Query::HowTall(None)).unwrap(),
            QueryAnswer::Height {
                ldu: 0,
                bricks: 0,
                plates: 0
            }
        );
        assert_eq!(
            answer(&model, &history, &Query::FindCollisions).unwrap(),
            QueryAnswer::NothingToReport
        );
        assert_eq!(
            answer(&model, &history, &Query::ShowUnsupported).unwrap(),
            QueryAnswer::NothingToReport
        );
        assert_eq!(
            answer(&model, &history, &Query::ListParts).unwrap(),
            QueryAnswer::NothingToReport
        );
        assert_eq!(
            answer(&model, &history, &Query::WhatChanged).unwrap(),
            QueryAnswer::Changed(None)
        );
        // A question about a specific thing still reports honestly.
        assert!(answer(&model, &history, &Query::WhereIs(Ref::Last)).is_err());
    }

    #[test]
    fn every_operation_has_a_description() {
        let ops = [
            add(4, IVec3::ZERO),
            Op::AddMany {
                placements: Vec::new(),
                group: Some("wall".to_string()),
            },
            Op::Move {
                targets: vec![],
                offset: IVec3::ZERO,
            },
            Op::Remove { targets: vec![] },
            Op::Paint {
                targets: vec![],
                color: 4,
            },
            Op::Rotate {
                targets: vec![],
                turns: 1,
                flip: false,
            },
            Op::Replace {
                targets: vec![],
                part: brick(),
            },
            Op::Select {
                predicate: crate::build::SelectPredicate::default(),
            },
            Op::Group {
                name: "roof".to_string(),
                targets: vec![],
            },
            Op::Landmark {
                name: "gate".to_string(),
                pos: IVec3::ZERO,
            },
            Op::Rename {
                name: "Castle".to_string(),
            },
        ];
        for op in &ops {
            assert!(!describe_op(op).is_empty(), "{op:?} has no description");
        }
    }

    #[test]
    fn subset_bounds_ignores_placements_that_are_gone() {
        let (model, _) = stacked();
        let ghost = PlacementId::new(99, 0);
        assert!(subset_bounds(&model, &[ghost]).is_none());
        assert!(subset_bounds(&model, &[PlacementId::new(0, 0), ghost]).is_some());
    }

    #[test]
    fn a_part_with_no_footprint_still_contributes_a_position() {
        let mut model = Model::new("t");
        model.placements.push(Placement {
            id: PlacementId::new(0, 0),
            part: PartRef::new("99999", "Mystery", (0, 0), PartKind::Other),
            color: 4,
            pos: IVec3::new(60, 0, 0),
            orientation: Orientation::DEFAULT,
            group: None,
        });
        let bounds = subset_bounds(&model, &[PlacementId::new(0, 0)]).unwrap();
        assert_eq!(bounds.0, IVec3::new(60, 0, 0));
    }
}

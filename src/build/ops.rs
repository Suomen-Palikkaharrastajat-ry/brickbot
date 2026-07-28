//! Model operations and the revision history.
//!
//! History is an append-only log of *operations*, not of model snapshots. The
//! model at any revision is a fold of the operations up to it, which is what
//! makes undo, redo, revert and "who added the roof?" fall out for free — and
//! makes the whole thing a pure function that needs no mocks to test.

#![allow(clippy::doc_markdown)]

use super::model::{Model, PartKind, PartRef, Placement, PlacementId};
use super::units::{IVec3, Orientation};
use serde::{Deserialize, Serialize};

/// How a repeated placement steps between copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    pub count: u32,
    pub step: IVec3,
}

/// A rule for choosing placements, stored rather than the placements it
/// currently matches.
///
/// `Select all red bricks.` means "all red bricks", not "these seven ids": if
/// history is replayed after an earlier edit, the selection must be
/// re-evaluated against the model as it then stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectPredicate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PartKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Select the highest placement only.
    #[serde(default)]
    pub highest: bool,
    /// Select everything above this Y coordinate (remember: smaller is higher).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above_y: Option<i32>,
}

impl SelectPredicate {
    #[must_use]
    pub fn matches(&self, placement: &Placement, model: &Model) -> bool {
        if let Some(color) = self.color {
            if placement.color != color {
                return false;
            }
        }
        if let Some(kind) = self.kind {
            if placement.part.kind != kind {
                return false;
            }
        }
        if let Some(group) = &self.group {
            if placement.group.as_deref() != Some(group.as_str()) {
                return false;
            }
        }
        if let Some(y) = self.above_y {
            if placement.pos.y >= y {
                return false;
            }
        }
        if self.highest {
            return model.highest().is_some_and(|h| h.id == placement.id);
        }
        true
    }

    /// The placements this predicate currently matches.
    #[must_use]
    pub fn resolve(&self, model: &Model) -> Vec<PlacementId> {
        model.all_matching(|p| self.matches(p, model))
    }
}

/// One part in a multi-placement operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub part: PartRef,
    pub color: u16,
    pub pos: IVec3,
    #[serde(default)]
    pub orientation: Orientation,
}

/// A single atomic edit.
///
/// Targets are stored as concrete [`PlacementId`]s rather than as references,
/// because ids are stable across replay. An operation whose targets no longer
/// exist quietly does nothing, which keeps replay total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Several placements created together.
    ///
    /// A structure such as `Build a wall 10 studs long 6 bricks high.` is one
    /// operation, not sixty: it is a single undo step, and the placements get
    /// sequence numbers 0..n within the same revision so each brick still has
    /// its own stable id and stays individually editable.
    AddMany {
        placements: Vec<Placed>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    Add {
        part: PartRef,
        color: u16,
        pos: IVec3,
        #[serde(default)]
        orientation: Orientation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<Repeat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    Move {
        targets: Vec<PlacementId>,
        offset: IVec3,
    },
    Remove {
        targets: Vec<PlacementId>,
    },
    Paint {
        targets: Vec<PlacementId>,
        color: u16,
    },
    Rotate {
        targets: Vec<PlacementId>,
        #[serde(default)]
        turns: i32,
        #[serde(default)]
        flip: bool,
    },
    Replace {
        targets: Vec<PlacementId>,
        part: PartRef,
    },
    Select {
        predicate: SelectPredicate,
    },
    Group {
        name: String,
        targets: Vec<PlacementId>,
    },
    Landmark {
        name: String,
        pos: IVec3,
    },
    Rename {
        name: String,
    },
}

/// What an operation did, for reporting back to the chat.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Effect {
    /// Placements created by this operation.
    pub added: Vec<PlacementId>,
    /// Placements changed or removed.
    pub affected: Vec<PlacementId>,
    /// True when the operation matched nothing at all.
    pub no_op: bool,
}

impl Effect {
    #[must_use]
    fn touching(ids: Vec<PlacementId>) -> Self {
        Self {
            no_op: ids.is_empty(),
            affected: ids,
            ..Self::default()
        }
    }
}

/// Applies one operation to the model.
///
/// `rev` is the index of this operation in the history and becomes the
/// revision component of any [`PlacementId`] the operation creates.
#[allow(clippy::too_many_lines)]
pub fn apply(model: &mut Model, rev: u32, op: &Op) -> Effect {
    match op {
        Op::AddMany { placements, group } => {
            let mut added = Vec::with_capacity(placements.len());
            for (seq, placed) in placements.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                let id = PlacementId::new(rev, seq as u32);
                model.placements.push(Placement {
                    id,
                    part: placed.part.clone(),
                    color: placed.color,
                    pos: placed.pos,
                    orientation: placed.orientation,
                    group: group.clone(),
                });
                added.push(id);
            }
            if let Some(name) = group {
                model
                    .groups
                    .entry(name.to_lowercase())
                    .or_default()
                    .extend(added.iter().copied());
            }
            model.touch(added.clone());
            Effect {
                no_op: added.is_empty(),
                added,
                affected: Vec::new(),
            }
        }

        Op::Add {
            part,
            color,
            pos,
            orientation,
            repeat,
            group,
        } => {
            let (count, step) = repeat.map_or((1, IVec3::ZERO), |r| (r.count.max(1), r.step));
            let mut added = Vec::new();
            for seq in 0..count {
                #[allow(clippy::cast_possible_wrap)]
                let offset = step.scale(seq as i32);
                let id = PlacementId::new(rev, seq);
                model.placements.push(Placement {
                    id,
                    part: part.clone(),
                    color: *color,
                    pos: pos.add(offset),
                    orientation: *orientation,
                    group: group.clone(),
                });
                added.push(id);
            }
            if let Some(name) = group {
                model
                    .groups
                    .entry(name.to_lowercase())
                    .or_default()
                    .extend(added.iter().copied());
            }
            model.touch(added.clone());
            Effect {
                no_op: added.is_empty(),
                added,
                affected: Vec::new(),
            }
        }

        Op::Move { targets, offset } => {
            let mut moved = Vec::new();
            for id in targets {
                if let Some(placement) = model.get_mut(*id) {
                    placement.pos = placement.pos.add(*offset);
                    moved.push(*id);
                }
            }
            model.touch(moved.clone());
            Effect::touching(moved)
        }

        Op::Remove { targets } => {
            let before = model.placements.len();
            model.placements.retain(|p| !targets.contains(&p.id));
            let removed: Vec<PlacementId> = targets
                .iter()
                .copied()
                .filter(|id| !model.contains(*id))
                .collect();
            // Drop the removed placements from groups and the selection so
            // nothing dangles.
            for ids in model.groups.values_mut() {
                ids.retain(|id| !targets.contains(id));
            }
            model.groups.retain(|_, ids| !ids.is_empty());
            model.selection.retain(|id| !targets.contains(id));
            model.touch(Vec::new());
            Effect {
                no_op: before == model.placements.len(),
                affected: removed,
                added: Vec::new(),
            }
        }

        Op::Paint { targets, color } => {
            let mut painted = Vec::new();
            for id in targets {
                if let Some(placement) = model.get_mut(*id) {
                    placement.color = *color;
                    painted.push(*id);
                }
            }
            model.touch(painted.clone());
            Effect::touching(painted)
        }

        Op::Rotate {
            targets,
            turns,
            flip,
        } => {
            let mut rotated = Vec::new();
            for id in targets {
                if let Some(placement) = model.get_mut(*id) {
                    let mut orientation = placement.orientation.turned(*turns);
                    if *flip {
                        orientation = orientation.flipped();
                    }
                    placement.orientation = orientation;
                    rotated.push(*id);
                }
            }
            model.touch(rotated.clone());
            Effect::touching(rotated)
        }

        Op::Replace { targets, part } => {
            let mut replaced = Vec::new();
            for id in targets {
                if let Some(placement) = model.get_mut(*id) {
                    placement.part = part.clone();
                    replaced.push(*id);
                }
            }
            model.touch(replaced.clone());
            Effect::touching(replaced)
        }

        Op::Select { predicate } => {
            let selected = predicate.resolve(model);
            model.selection.clone_from(&selected);
            model.touch(selected.clone());
            Effect::touching(selected)
        }

        Op::Group { name, targets } => {
            let live: Vec<PlacementId> = targets
                .iter()
                .copied()
                .filter(|id| model.contains(*id))
                .collect();
            for id in &live {
                if let Some(placement) = model.get_mut(*id) {
                    placement.group = Some(name.to_lowercase());
                }
            }
            if live.is_empty() {
                model.groups.remove(&name.to_lowercase());
            } else {
                model.groups.insert(name.to_lowercase(), live.clone());
            }
            model.touch(live.clone());
            Effect::touching(live)
        }

        Op::Landmark { name, pos } => {
            model.landmarks.insert(name.to_lowercase(), *pos);
            Effect::default()
        }

        Op::Rename { name } => {
            model.name.clone_from(name);
            Effect::default()
        }
    }
}

/// One operation as recorded in the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedOp {
    pub op: Op,
    /// Discord user id of whoever issued the command.
    pub author: String,
    pub at: chrono::DateTime<chrono::Utc>,
    /// Set when this single operation was undone out of order, by
    /// `Undo Alice's last edit.`
    ///
    /// The entry stays in the log rather than being deleted, so that the
    /// revision indices — and therefore every [`PlacementId`] — stay stable.
    #[serde(default)]
    pub skipped: bool,
}

/// The full edit history of a build session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub name: String,
    #[serde(default)]
    pub ops: Vec<AppliedOp>,
    /// How many operations are currently in effect. Everything from here on is
    /// the redo tail.
    #[serde(default)]
    pub cursor: usize,
}

impl History {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ops: Vec::new(),
            cursor: 0,
        }
    }

    /// Rebuilds the model by folding every live operation up to the cursor.
    #[must_use]
    pub fn materialize(&self) -> Model {
        let mut model = Model::new(&self.name);
        for (index, entry) in self.ops.iter().take(self.cursor).enumerate() {
            if entry.skipped {
                continue;
            }
            #[allow(clippy::cast_possible_truncation)]
            apply(&mut model, index as u32, &entry.op);
        }
        model
    }

    /// The revision index the next pushed operation will occupy.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn next_rev(&self) -> u32 {
        self.cursor as u32
    }

    /// Appends an operation, discarding any redo tail.
    pub fn push(&mut self, op: Op, author: impl Into<String>) -> u32 {
        self.ops.truncate(self.cursor);
        let rev = self.next_rev();
        self.ops.push(AppliedOp {
            op,
            author: author.into(),
            at: chrono::Utc::now(),
            skipped: false,
        });
        self.cursor = self.ops.len();
        rev
    }

    /// Steps back one operation. Returns false when there is nothing to undo.
    pub const fn undo(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        true
    }

    /// Steps forward one operation. Returns false when there is nothing to
    /// redo.
    pub const fn redo(&mut self) -> bool {
        if self.cursor >= self.ops.len() {
            return false;
        }
        self.cursor += 1;
        true
    }

    /// Rewinds to a specific revision count. Returns false when out of range.
    pub const fn revert_to(&mut self, revision: usize) -> bool {
        if revision > self.ops.len() {
            return false;
        }
        self.cursor = revision;
        true
    }

    /// Undoes the most recent live operation by a particular author, leaving
    /// everyone else's edits in place.
    pub fn undo_last_by(&mut self, author: &str) -> bool {
        let Some(index) = self.ops[..self.cursor]
            .iter()
            .rposition(|entry| entry.author == author && !entry.skipped)
        else {
            return false;
        };
        self.ops[index].skipped = true;
        true
    }

    /// The author of the operation that created a placement.
    #[must_use]
    pub fn author_of(&self, id: PlacementId) -> Option<&str> {
        self.ops
            .get(id.rev as usize)
            .map(|entry| entry.author.as_str())
    }

    /// The most recent live operation, for "what changed?".
    #[must_use]
    pub fn last(&self) -> Option<&AppliedOp> {
        self.ops[..self.cursor].iter().rev().find(|e| !e.skipped)
    }

    #[must_use]
    pub const fn revision(&self) -> usize {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brick() -> PartRef {
        PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
    }

    fn plate() -> PartRef {
        PartRef::new("3020", "Plate 2 x 4", (2, 4), PartKind::Plate)
    }

    fn add_at(color: u16, pos: IVec3) -> Op {
        Op::Add {
            part: brick(),
            color,
            pos,
            orientation: Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    #[test]
    fn adding_a_part_creates_a_placement_and_sets_the_reference() {
        let mut model = Model::new("t");
        let effect = apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        assert_eq!(model.len(), 1);
        assert_eq!(effect.added, vec![PlacementId::new(0, 0)]);
        assert_eq!(model.last_ref, vec![PlacementId::new(0, 0)]);
    }

    #[test]
    fn a_repeated_add_creates_one_placement_per_step() {
        let mut model = Model::new("t");
        let op = Op::Add {
            part: brick(),
            color: 4,
            pos: IVec3::ZERO,
            orientation: Orientation::DEFAULT,
            repeat: Some(Repeat {
                count: 3,
                step: IVec3::new(0, -24, 0),
            }),
            group: None,
        };
        apply(&mut model, 7, &op);
        assert_eq!(model.len(), 3);
        assert_eq!(model.placements[0].id, PlacementId::new(7, 0));
        assert_eq!(model.placements[2].id, PlacementId::new(7, 2));
        assert_eq!(model.placements[2].pos, IVec3::new(0, -48, 0));
    }

    #[test]
    fn add_many_gives_every_placement_its_own_sequence_number() {
        // Regression: an expanded structure used to emit one Op::Add per
        // brick, and every one of them claimed PlacementId(rev, 0). Later
        // edits then all landed on the same brick.
        let mut model = Model::new("t");
        let op = Op::AddMany {
            placements: (0..4)
                .map(|i| Placed {
                    part: brick(),
                    color: 4,
                    pos: IVec3::new(i * 100, 0, 0),
                    orientation: Orientation::DEFAULT,
                })
                .collect(),
            group: Some("wall".to_string()),
        };
        let effect = apply(&mut model, 3, &op);

        assert_eq!(model.len(), 4);
        assert_eq!(effect.added.len(), 4);
        let ids: std::collections::HashSet<_> = effect.added.iter().collect();
        assert_eq!(ids.len(), 4, "placement ids must be distinct");
        assert_eq!(model.placements[0].id, PlacementId::new(3, 0));
        assert_eq!(model.placements[3].id, PlacementId::new(3, 3));
        // Every one belongs to the structure's group.
        assert_eq!(model.resolve_group("wall").map(|ids| ids.len()), Some(4));
    }

    #[test]
    fn add_many_is_a_single_undo_step() {
        let mut history = History::new("t");
        history.push(
            Op::AddMany {
                placements: (0..5)
                    .map(|i| Placed {
                        part: brick(),
                        color: 4,
                        pos: IVec3::new(i * 100, 0, 0),
                        orientation: Orientation::DEFAULT,
                    })
                    .collect(),
                group: None,
            },
            "alice",
        );
        assert_eq!(history.materialize().len(), 5);
        history.undo();
        assert!(
            history.materialize().is_empty(),
            "one undo removes them all"
        );
    }

    #[test]
    fn moving_shifts_only_the_targeted_placements() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        apply(&mut model, 1, &add_at(4, IVec3::new(100, 0, 0)));

        apply(
            &mut model,
            2,
            &Op::Move {
                targets: vec![PlacementId::new(0, 0)],
                offset: IVec3::new(20, 0, 0),
            },
        );
        assert_eq!(model.get(PlacementId::new(0, 0)).unwrap().pos.x, 20);
        assert_eq!(model.get(PlacementId::new(1, 0)).unwrap().pos.x, 100);
    }

    #[test]
    fn removing_clears_groups_and_selection_too() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        let id = PlacementId::new(0, 0);
        apply(
            &mut model,
            1,
            &Op::Group {
                name: "wall".to_string(),
                targets: vec![id],
            },
        );
        apply(
            &mut model,
            2,
            &Op::Select {
                predicate: SelectPredicate::default(),
            },
        );
        assert_eq!(model.selection, vec![id]);

        apply(&mut model, 3, &Op::Remove { targets: vec![id] });
        assert!(model.is_empty());
        assert!(model.groups.is_empty());
        assert!(model.selection.is_empty());
    }

    #[test]
    fn operations_on_missing_targets_are_harmless_no_ops() {
        let mut model = Model::new("t");
        let ghost = PlacementId::new(99, 0);
        for op in [
            Op::Move {
                targets: vec![ghost],
                offset: IVec3::new(1, 0, 0),
            },
            Op::Paint {
                targets: vec![ghost],
                color: 4,
            },
            Op::Remove {
                targets: vec![ghost],
            },
            Op::Rotate {
                targets: vec![ghost],
                turns: 1,
                flip: false,
            },
        ] {
            let effect = apply(&mut model, 0, &op);
            assert!(effect.no_op, "{op:?} should have been a no-op");
        }
    }

    #[test]
    fn painting_and_replacing_change_the_placement_in_place() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        let id = PlacementId::new(0, 0);

        apply(
            &mut model,
            1,
            &Op::Paint {
                targets: vec![id],
                color: 14,
            },
        );
        assert_eq!(model.get(id).unwrap().color, 14);

        apply(
            &mut model,
            2,
            &Op::Replace {
                targets: vec![id],
                part: plate(),
            },
        );
        assert_eq!(model.get(id).unwrap().part.kind, PartKind::Plate);
        // The position is untouched; only the element changes.
        assert_eq!(model.get(id).unwrap().pos, IVec3::ZERO);
    }

    #[test]
    fn rotating_accumulates_turns_and_flips() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        let id = PlacementId::new(0, 0);

        apply(
            &mut model,
            1,
            &Op::Rotate {
                targets: vec![id],
                turns: 1,
                flip: false,
            },
        );
        apply(
            &mut model,
            2,
            &Op::Rotate {
                targets: vec![id],
                turns: 1,
                flip: true,
            },
        );
        let o = model.get(id).unwrap().orientation;
        assert_eq!(o.quarter_turns, 2);
        assert!(o.upside_down);
    }

    #[test]
    fn a_select_predicate_filters_by_colour_and_kind() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        apply(&mut model, 1, &add_at(14, IVec3::new(100, 0, 0)));
        apply(
            &mut model,
            2,
            &Op::Add {
                part: plate(),
                color: 4,
                pos: IVec3::new(200, 0, 0),
                orientation: Orientation::DEFAULT,
                repeat: None,
                group: None,
            },
        );

        let red_bricks = SelectPredicate {
            color: Some(4),
            kind: Some(PartKind::Brick),
            ..SelectPredicate::default()
        };
        assert_eq!(red_bricks.resolve(&model), vec![PlacementId::new(0, 0)]);

        let everything = SelectPredicate::default();
        assert_eq!(everything.resolve(&model).len(), 3);
    }

    #[test]
    fn the_highest_predicate_picks_a_single_placement() {
        let mut model = Model::new("t");
        apply(&mut model, 0, &add_at(4, IVec3::ZERO));
        apply(&mut model, 1, &add_at(4, IVec3::new(0, -48, 0)));

        let highest = SelectPredicate {
            highest: true,
            ..SelectPredicate::default()
        };
        assert_eq!(highest.resolve(&model), vec![PlacementId::new(1, 0)]);
    }

    #[test]
    fn history_folds_operations_into_a_model() {
        let mut history = History::new("Castle");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(0, -24, 0)), "bob");

        let model = history.materialize();
        assert_eq!(model.name, "Castle");
        assert_eq!(model.len(), 2);
        assert_eq!(history.revision(), 2);
    }

    #[test]
    fn undo_and_redo_move_the_cursor() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(100, 0, 0)), "bob");
        assert_eq!(history.materialize().len(), 2);

        assert!(history.undo());
        assert_eq!(history.materialize().len(), 1);

        assert!(history.redo());
        assert_eq!(history.materialize().len(), 2);

        assert!(!history.redo(), "nothing left to redo");
    }

    #[test]
    fn undo_on_an_empty_history_reports_failure() {
        let mut history = History::new("t");
        assert!(!history.undo());
    }

    #[test]
    fn pushing_after_an_undo_discards_the_redo_tail() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(100, 0, 0)), "alice");
        history.undo();
        history.push(add_at(2, IVec3::new(200, 0, 0)), "bob");

        assert_eq!(history.ops.len(), 2);
        assert!(!history.redo());
        let model = history.materialize();
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[1].color, 2);
    }

    #[test]
    fn revert_to_rewinds_to_an_arbitrary_revision() {
        let mut history = History::new("t");
        for i in 0..5 {
            history.push(add_at(4, IVec3::new(i * 100, 0, 0)), "alice");
        }
        assert!(history.revert_to(2));
        assert_eq!(history.materialize().len(), 2);
        assert!(!history.revert_to(99), "out of range");
    }

    #[test]
    fn undoing_one_authors_edit_leaves_the_others_alone() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(100, 0, 0)), "bob");
        history.push(add_at(2, IVec3::new(200, 0, 0)), "alice");

        assert!(history.undo_last_by("alice"));
        let model = history.materialize();
        assert_eq!(model.len(), 2);
        // Alice's first brick and Bob's brick remain.
        assert!(model.contains(PlacementId::new(0, 0)));
        assert!(model.contains(PlacementId::new(1, 0)));
        assert!(!model.contains(PlacementId::new(2, 0)));

        assert!(!history.undo_last_by("carol"), "carol has made no edits");
    }

    #[test]
    fn skipping_an_operation_keeps_later_placement_ids_stable() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(100, 0, 0)), "bob");
        history.push(
            Op::Paint {
                targets: vec![PlacementId::new(1, 0)],
                color: 2,
            },
            "carol",
        );

        // Undoing Alice's edit must not renumber Bob's brick, or Carol's
        // paint would land on the wrong part.
        assert!(history.undo_last_by("alice"));
        let model = history.materialize();
        assert_eq!(model.get(PlacementId::new(1, 0)).unwrap().color, 2);
    }

    #[test]
    fn tracks_who_added_what() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(100, 0, 0)), "bob");

        assert_eq!(history.author_of(PlacementId::new(0, 0)), Some("alice"));
        assert_eq!(history.author_of(PlacementId::new(1, 0)), Some("bob"));
        assert_eq!(history.author_of(PlacementId::new(9, 0)), None);
        assert_eq!(history.last().unwrap().author, "bob");
    }

    #[test]
    fn renaming_and_landmarks_take_effect() {
        let mut history = History::new("t");
        history.push(
            Op::Rename {
                name: "Castle".to_string(),
            },
            "alice",
        );
        history.push(
            Op::Landmark {
                name: "Gate".to_string(),
                pos: IVec3::new(20, 0, 40),
            },
            "alice",
        );

        let model = history.materialize();
        assert_eq!(model.name, "Castle");
        assert_eq!(model.landmarks.get("gate"), Some(&IVec3::new(20, 0, 40)));
    }

    #[test]
    fn a_history_round_trips_through_json() {
        let mut history = History::new("Castle");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(
            Op::Select {
                predicate: SelectPredicate {
                    color: Some(4),
                    ..SelectPredicate::default()
                },
            },
            "bob",
        );

        let json = serde_json::to_string(&history).unwrap();
        let back: History = serde_json::from_str(&json).unwrap();
        assert_eq!(back, history);
        assert_eq!(back.materialize(), history.materialize());
    }

    #[test]
    fn materialize_is_deterministic() {
        let mut history = History::new("t");
        history.push(add_at(4, IVec3::ZERO), "alice");
        history.push(add_at(14, IVec3::new(0, -24, 0)), "bob");
        assert_eq!(history.materialize(), history.materialize());
    }
}

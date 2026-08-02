//! The pure heart of the thread listener.
//!
//! One function takes a chat message and the session's history, and returns
//! everything the Discord layer needs to do: which operations to record, what
//! to say, which reactions to add, whether to re-render. Keeping the decisions
//! here means the serenity code is a thin shell and the behaviour is tested
//! without a network, a database or a mock.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::{Meta, ObjectExpr, Statement};
use super::dsl::parser::{SentenceOutcome, parse_message};
use super::lower::{LowerError, Unsupported, lower_command_with};
use super::model::{PartKind, PartRef};
use super::ops::{History, Op, Placed, apply};
use super::parts;
use super::propose;
use super::units::MaxModelSize;
use super::{legal, queries, report};

/// A part the bot could not resolve, which the `/part` picker will answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLookup {
    pub query: String,
    pub kind: PartKind,
    /// The sentence to retry once a part has been chosen.
    pub sentence: String,
}

/// Something the builder named without saying where it goes, waiting on a
/// choice of position.
///
/// Carries shapes rather than positions: working out *where* needs the model,
/// which the Discord layer has to reload under the session lock anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProposal {
    /// The shapes on offer — several when nothing said how they should sit.
    pub batches: Vec<Vec<Placed>>,
    /// What is being placed, as the builder would recognise it.
    pub label: String,
}

/// The people building in a session, so a sentence can name one of them.
///
/// `Undo alice's last edit.` names a person the way chat does, but the
/// operation log stores Discord user ids. Without a directory the two are
/// compared directly and never match, which is why that sentence used to
/// always answer "there is nothing to undo".
///
/// Kept as a plain list of pairs rather than anything serenity-shaped, so
/// [`process_message`] stays a pure function that tests can drive by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Builders {
    /// `(lowercased display name, user id)`, in no particular order.
    entries: Vec<(String, String)>,
}

impl Builders {
    #[must_use]
    pub fn new(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(name, id)| (name.to_lowercase(), id))
                .collect(),
        }
    }

    /// Resolves a name typed in chat to a Discord user id.
    ///
    /// Tries an exact name, then a user id given verbatim, then an unambiguous
    /// prefix — `Undo al's last edit.` should work when Alice is the only
    /// `al…` in the thread, but must not guess between Alice and Alan.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&str> {
        let needle = name.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        if let Some((_, id)) = self.entries.iter().find(|(n, _)| *n == needle) {
            return Some(id);
        }
        if let Some((_, id)) = self.entries.iter().find(|(_, id)| *id == needle) {
            return Some(id);
        }
        let mut matches = self
            .entries
            .iter()
            .filter(|(n, _)| n.starts_with(&needle))
            .map(|(_, id)| id.as_str());
        let first = matches.next()?;
        // Ambiguous prefixes resolve to nobody rather than to the wrong person.
        matches.next().is_none().then_some(first)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Reactions the bot adds to the triggering message.
pub mod reactions {
    /// The command was applied.
    pub const APPLIED: &str = "✅";
    /// A `Note:` or `TODO:` was noticed but not acted on.
    pub const COMMENT: &str = "📝";
    /// Something was wrong with the command.
    pub const PROBLEM: &str = "❓";
}

/// Everything the Discord layer should do about one message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Operations to apply and persist, in order.
    pub ops: Vec<Op>,
    /// Messages to post in the thread.
    pub replies: Vec<String>,
    /// Reactions to add to the triggering message.
    pub reactions: Vec<&'static str>,
    /// True when the model changed and a new picture is warranted.
    pub needs_render: bool,
    pub needs_export: bool,
    pub finished: bool,
    /// Set when a part must be looked up before the sentence can be finished.
    pub pending_lookup: Option<PendingLookup>,
    /// Set when the builder named something but not where it goes, so the
    /// choice is offered rather than guessed.
    pub pending_proposal: Option<PendingProposal>,
    /// The revision index `ops[0]` must be written at, and from which any redo
    /// tail must be discarded.
    ///
    /// Not simply the loaded history's cursor: `Undo. Add a brick.` rewinds
    /// first, so the new operation belongs at the revision the undo freed. The
    /// caller must not derive this itself.
    pub ops_start: Option<usize>,
    /// Where the history cursor ends up, when anything moved it — an
    /// `Undo`/`Redo`/`Revert`, or new operations being appended.
    pub cursor: Option<usize>,
    /// The author whose last edit should be marked skipped.
    pub skip_last_by: Option<String>,
}

impl Outcome {
    /// True when the bot should stay completely silent.
    ///
    /// This is the common case in a busy thread, and getting it right is what
    /// makes the feature liveable: ordinary conversation must produce nothing
    /// at all — no reply, no reaction, no render.
    #[must_use]
    pub const fn is_silent(&self) -> bool {
        self.ops.is_empty()
            && self.replies.is_empty()
            && self.reactions.is_empty()
            && !self.needs_render
            && !self.needs_export
            && !self.finished
            && self.pending_lookup.is_none()
            && self.pending_proposal.is_none()
            && self.ops_start.is_none()
            && self.cursor.is_none()
            && self.skip_last_by.is_none()
    }

    fn problem(&mut self, message: String) {
        self.replies.push(message);
        if !self.reactions.contains(&reactions::PROBLEM) {
            self.reactions.push(reactions::PROBLEM);
        }
    }

    fn applied(&mut self) {
        if !self.reactions.contains(&reactions::APPLIED) {
            self.reactions.push(reactions::APPLIED);
        }
    }
}

/// Whether a message contains anything the interpreter would act on.
///
/// Parsing needs no model, so this answers the question without loading the
/// session history — which is worth doing, because ordinary conversation is
/// most of what a build thread carries.
#[must_use]
pub fn is_ordinary_chat(text: &str) -> bool {
    parse_message(text)
        .iter()
        .all(|sentence| matches!(sentence, SentenceOutcome::Ignored))
}

/// Interprets one chat message against the session's history.
///
/// Sentences are processed in order. A failure stops the rest of the message
/// rather than silently applying the remainder of a sequence whose earlier
/// steps did not happen.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn process_message(
    history: &History,
    text: &str,
    locale: &str,
    builders: &Builders,
    author: &str,
    max_size: MaxModelSize,
) -> Outcome {
    run_sentences(history, text, locale, builders, author, None, max_size)
}

/// Interprets a message with a part the builder has just chosen.
///
/// Used to finish a sentence that paused on an unknown part: the sentence is
/// replayed exactly as typed, with `chosen` answering the lookup it stopped
/// on. Going back through the ordinary path — rather than patching an
/// operation together by hand — is what keeps a resolved part behaving like a
/// catalogue hit, modifiers, location and all.
#[must_use]
pub fn process_message_with_part(
    history: &History,
    text: &str,
    locale: &str,
    builders: &Builders,
    author: &str,
    chosen: &PartRef,
    max_size: MaxModelSize,
) -> Outcome {
    run_sentences(
        history,
        text,
        locale,
        builders,
        author,
        Some(chosen),
        max_size,
    )
}

/// A model's extent on each axis, in LDU — what the max-size check compares
/// against its configured limit. `(x, y, z)`, zero for an empty model.
fn span(bounds: Option<(super::units::IVec3, super::units::IVec3)>) -> (i32, i32, i32) {
    bounds.map_or((0, 0, 0), |(lo, hi)| {
        (hi.x - lo.x, hi.y - lo.y, hi.z - lo.z)
    })
}

#[allow(clippy::too_many_lines)]
fn run_sentences(
    history: &History,
    text: &str,
    locale: &str,
    builders: &Builders,
    // Whoever sent the message, so operations simulated within it are
    // attributed correctly to later sentences of the same message.
    author: &str,
    chosen: Option<&PartRef>,
    max_size: MaxModelSize,
) -> Outcome {
    let mut outcome = Outcome::default();
    // A working copy, so each sentence sees the effect of the previous one.
    let mut working = history.clone();
    // The model `working` folds to, carried across sentences and kept in step
    // as operations are applied. Materializing replays the whole log, so doing
    // it per sentence — and again per operation — made a long session quadratic
    // in its own history. It is rebuilt only when an undo, redo or revert moves
    // the cursor, where the log no longer folds to what we are holding.
    let mut model = working.materialize();
    // Set when a sentence failed, so the rest of the message is abandoned.
    let mut stopped = false;

    'sentences: for (index, sentence) in parse_message(text).into_iter().enumerate() {
        let statement = match sentence {
            // Ordinary chat: contribute nothing whatsoever.
            SentenceOutcome::Ignored => continue,
            SentenceOutcome::Error(error) => {
                outcome.problem(report::describe_parse_error(&error, locale));
                stopped = true;
                break 'sentences;
            }
            SentenceOutcome::Statement(statement) => statement,
        };

        match statement {
            Statement::Comment => {
                if !outcome.reactions.contains(&reactions::COMMENT) {
                    outcome.reactions.push(reactions::COMMENT);
                }
            }

            // A part named with no verb asks where it could go, so the answer
            // is a set of choices rather than an operation. Nothing is applied
            // and nothing is recorded until the builder picks one.
            Statement::Propose { spec, arrangement } => {
                let part = match parts::resolve(&spec) {
                    parts::Resolution::Known(part) => part,
                    // An unusual part still has to be identified first; the
                    // existing picker does that, and the sentence is replayed.
                    parts::Resolution::NeedsLookup { query, kind } => {
                        outcome.pending_lookup = Some(PendingLookup {
                            query,
                            kind,
                            sentence: nth_sentence(text, index),
                        });
                        stopped = true;
                        break 'sentences;
                    }
                };
                let color = spec.color.unwrap_or(super::lower::DEFAULT_COLOR);
                let label = report::describe_part(&part, color, spec.quantity, locale);
                outcome.pending_proposal = Some(PendingProposal {
                    batches: propose::arrangements(&part, color, spec.quantity, arrangement)
                        .into_iter()
                        .map(|(_, placements)| placements)
                        .collect(),
                    label,
                });
                stopped = true;
                break 'sentences;
            }

            Statement::Meta(meta) => {
                if !apply_meta(&mut outcome, &mut working, &meta, locale, builders) {
                    stopped = true;
                    break 'sentences;
                }
                if meta.rewrites_history() {
                    model = working.materialize();
                }
            }

            Statement::Query(query) => {
                match queries::answer(&model, &working, &query) {
                    Ok(answer) => outcome
                        .replies
                        .push(report::describe_answer(&answer, &model, locale)),
                    // A question naming something we have never heard of is
                    // far more likely to be chat — "where is everyone?" — than
                    // a real query, and it changes nothing either way, so the
                    // silence policy wins. Questions that fail for any other
                    // reason are still worth answering.
                    Err(
                        LowerError::UnknownName(_)
                        | LowerError::UnresolvedReference(_)
                        | LowerError::NothingToActOn,
                    ) => {}
                    Err(error) => {
                        outcome.problem(report::describe_error(&error, locale));
                        stopped = true;
                        break 'sentences;
                    }
                }
            }

            Statement::Command(command) => {
                match lower_command_with(&model, &command, chosen) {
                    Ok(ops) => {
                        // A drawing that never says where it goes used to land
                        // on whatever was last touched. Offer the choice
                        // instead — it is the same question a bare part asks,
                        // and the block is just a bigger batch.
                        if matches!(command.object, ObjectExpr::Grid(_))
                            && command.location.is_none()
                        {
                            if let Some(Op::AddMany { placements, .. }) = ops.first() {
                                outcome.pending_proposal = Some(PendingProposal {
                                    batches: vec![placements.clone()],
                                    label: report::describe_drawing(placements.len(), locale),
                                });
                                stopped = true;
                                break 'sentences;
                            }
                        }
                        for op in ops {
                            // Correct the operation *before* it is applied or
                            // recorded, so the revision log is legal by
                            // construction. Fixing only the model would put
                            // the illegal position straight back on reload.
                            let rev = working.next_rev();
                            let legalized = match legal::legalize(&model, rev, &op) {
                                Ok(legalized) => legalized,
                                Err(error) => {
                                    outcome.problem(report::describe_error(&error, locale));
                                    stopped = true;
                                    break 'sentences;
                                }
                            };

                            // A cap on the model's own footprint, not on what
                            // one command may place — many small commands
                            // could otherwise creep a model past its box just
                            // as easily as one big one. Compared against the
                            // model *before* this op, so an already-oversized
                            // model (built before the limit existed, or under
                            // a looser session size) isn't punished for edits
                            // that don't make it any bigger.
                            let before = span(model.bounds());
                            let mut probe = model.clone();
                            for op in legalized.ops() {
                                apply(&mut probe, rev, op);
                            }
                            let after = span(probe.bounds());
                            if max_size.exceeded_by(before, after) {
                                outcome.problem(report::describe_error(
                                    &LowerError::Unsupported(Unsupported::ExceedsMaxSize {
                                        x: max_size.x,
                                        y: max_size.y,
                                        z: max_size.z,
                                    }),
                                    locale,
                                ));
                                stopped = true;
                                break 'sentences;
                            }

                            let note = report::describe_adjustment(&legalized.adjustment, locale);
                            for (index, op) in legalized.ops().enumerate() {
                                let rev = working.next_rev();
                                let effect = apply(&mut model, rev, op);
                                // Only the operation the builder asked for is
                                // worth narrating; the follow-up shifts a
                                // rotate needs are what the note is for.
                                if index == 0 {
                                    outcome
                                        .replies
                                        .push(report::describe_effect(op, &effect, locale));
                                }
                                // The first operation fixes where the whole
                                // batch is written, which is not the loaded
                                // cursor when an undo came earlier in the same
                                // message.
                                if outcome.ops_start.is_none() {
                                    outcome.ops_start = Some(rev as usize);
                                }
                                working.push(op.clone(), author);
                                outcome.ops.push(op.clone());
                            }
                            if let Some(note) = note {
                                outcome.replies.push(note);
                            }
                        }
                        outcome.applied();
                        outcome.needs_render = true;
                    }
                    Err(LowerError::NeedsPartLookup { query, kind }) => {
                        // Pause here: the builder must pick a part before this
                        // sentence, or anything after it, can be carried out.
                        outcome.pending_lookup = Some(PendingLookup {
                            query,
                            kind,
                            sentence: nth_sentence(text, index),
                        });
                        stopped = true;
                        break 'sentences;
                    }
                    Err(error) => {
                        outcome.problem(report::describe_error(&error, locale));
                        stopped = true;
                        break 'sentences;
                    }
                }
            }
        }
    }

    // Only the last reply is worth posting when several commands succeeded;
    // a running commentary on every brick would drown the thread. A message
    // that stopped early keeps every reply, because the last of them is the
    // problem and the ones before it say what did happen.
    if !stopped && outcome.ops.len() > 1 && outcome.replies.len() > 1 {
        let last = outcome.replies.pop().unwrap_or_default();
        outcome.replies.clear();
        outcome.replies.push(last);
    }

    // The cursor is reported once, from the simulation, rather than by each
    // meta command in turn: `Undo. Add a brick.` moves it twice and only the
    // final position is meaningful to the caller.
    if working.revision() != history.revision() || !outcome.ops.is_empty() {
        outcome.cursor = Some(working.revision());
    }

    outcome
}

/// Applies a session-level command. Returns false when processing should stop.
fn apply_meta(
    outcome: &mut Outcome,
    working: &mut History,
    meta: &Meta,
    locale: &str,
    builders: &Builders,
) -> bool {
    use rust_i18n::t;

    match meta {
        Meta::Undo => {
            if working.undo() {
                outcome
                    .replies
                    .push(t!("command.build.undone", locale = locale).to_string());
                outcome.needs_render = true;
                outcome.applied();
            } else {
                outcome.problem(t!("command.build.nothing_to_undo", locale = locale).to_string());
                return false;
            }
        }
        Meta::UndoBy(name) => {
            // The sentence names a person; the log records user ids. Resolving
            // here is what makes the two comparable at all.
            let Some(user_id) = builders.resolve(name).map(ToString::to_string) else {
                outcome.problem(
                    t!(
                        "command.build.error_unknown_builder",
                        locale = locale,
                        name = name
                    )
                    .to_string(),
                );
                return false;
            };
            if working.undo_last_by(&user_id) {
                outcome.skip_last_by = Some(user_id);
                outcome
                    .replies
                    .push(t!("command.build.undone", locale = locale).to_string());
                outcome.needs_render = true;
                outcome.applied();
            } else {
                outcome.problem(t!("command.build.nothing_to_undo", locale = locale).to_string());
                return false;
            }
        }
        Meta::Redo => {
            if working.redo() {
                outcome
                    .replies
                    .push(t!("command.build.redone", locale = locale).to_string());
                outcome.needs_render = true;
                outcome.applied();
            } else {
                outcome.problem(t!("command.build.nothing_to_redo", locale = locale).to_string());
                return false;
            }
        }
        Meta::RevertTo(revision) => {
            let target = *revision as usize;
            if working.revert_to(target) {
                outcome.replies.push(
                    t!(
                        "command.build.reverted",
                        locale = locale,
                        revision = revision
                    )
                    .to_string(),
                );
                outcome.needs_render = true;
                outcome.applied();
            } else {
                outcome.problem(
                    t!(
                        "command.build.cannot_revert",
                        locale = locale,
                        revision = revision
                    )
                    .to_string(),
                );
                return false;
            }
        }
        Meta::Export => {
            outcome.needs_export = true;
            outcome.applied();
        }
        Meta::Render => {
            outcome.needs_render = true;
            outcome.applied();
        }
        Meta::Help(topic) => outcome.replies.push(report::help_text(*topic, locale)),
        Meta::Finish => {
            outcome.finished = true;
            outcome.applied();
        }
    }
    true
}

/// Recovers the nth sentence of a message, for retrying after a part lookup.
///
/// Goes back through the lexer rather than re-splitting the text by hand, so a
/// sentence carrying a drawing is recovered with its fence attached — without
/// it, `Build from top:` on its own means nothing when it is replayed.
fn nth_sentence(text: &str, index: usize) -> String {
    super::dsl::lexer::tokenize(text)
        .into_iter()
        .nth(index)
        .map_or_else(|| text.trim().to_string(), |s| s.replay_text())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default `max_model_size` — most tests aren't exercising the cap
    /// and just want the ordinary limit out of the way.
    const MAX_MODEL_SIZE: MaxModelSize = MaxModelSize::cube(10);

    fn empty() -> History {
        History::new("Castle")
    }

    /// The people in the fixture thread, as the session layer would supply
    /// them: chat names on the left, Discord user ids on the right.
    fn builders() -> Builders {
        Builders::new([
            ("Alice".to_string(), "111".to_string()),
            ("Bob".to_string(), "222".to_string()),
        ])
    }

    fn interpret(history: &History, text: &str, locale: &str) -> Outcome {
        process_message(history, text, locale, &builders(), "111", MAX_MODEL_SIZE)
    }

    fn add_brick(color: u16) -> Op {
        Op::Add {
            part: crate::build::PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
            color,
            pos: crate::build::IVec3::ZERO,
            orientation: crate::build::Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    /// Runs a script, returning the resulting history.
    fn run(script: &[&str]) -> History {
        let mut history = empty();
        for text in script {
            let outcome = interpret(&history, text, "en-US");
            for op in outcome.ops {
                history.push(op, "111");
            }
            if let Some(cursor) = outcome.cursor {
                history.cursor = cursor;
            }
        }
        history
    }

    /// The interpreter carries one model across a message instead of replaying
    /// the log per sentence. These check the carried model still equals the
    /// replayed one.
    #[test]
    fn a_multi_sentence_message_matches_the_same_sentences_sent_separately() {
        let batched = run(&["Add a red 2x4 brick at origin. Add a blue 2x4 brick."]).materialize();
        let separate =
            run(&["Add a red 2x4 brick at origin.", "Add a blue 2x4 brick."]).materialize();

        let colors =
            |m: &crate::build::Model| m.placements.iter().map(|p| p.color).collect::<Vec<_>>();
        assert_eq!(colors(&batched), colors(&separate));
        assert_eq!(batched.len(), 2);
    }

    #[test]
    fn a_revert_inside_a_message_is_seen_by_the_sentences_after_it() {
        let history = run(&[
            "Add a red 2x4 brick at origin.",
            "Add a blue 2x4 brick.",
            "Add a yellow 2x4 brick.",
        ]);
        assert_eq!(history.materialize().len(), 3);

        // Reverting and then building in one breath: the new brick must land on
        // the reverted model, so it takes the revision the revert freed rather
        // than being appended after the yellow one.
        let outcome = interpret(
            &history,
            "Revert to revision 2. Add a green 2x4 brick.",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 1);
        assert_eq!(outcome.ops_start, Some(2));

        let mut applied = history;
        applied.cursor = outcome.ops_start.unwrap();
        for op in outcome.ops {
            applied.push(op, "111");
        }
        let model = applied.materialize();
        assert_eq!(model.len(), 3);
        // Green replaced yellow rather than following it.
        assert_eq!(model.placements.last().unwrap().color, 2);
    }

    /// Known bug, unrelated to how the model is carried: `Outcome::ops` is
    /// append-only, so an undo *between* two commands in one message leaves the
    /// superseded operation in the list. The session then persists a log the
    /// interpreter never had — here `[red, blue, yellow]` with the cursor at 2,
    /// which materializes to red+blue, when the message plainly asked for
    /// red+yellow. Sending the same four sentences as separate messages is
    /// correct, which is why this went unnoticed.
    #[test]
    #[ignore = "documents a known bug in Outcome::ops; see the comment above"]
    fn an_undo_between_two_commands_in_one_message_is_persisted_wrongly() {
        let batched = run(&[
            "Add a red 2x4 brick at origin. Add a blue 2x4 brick. Undo. Add a yellow 2x4 brick.",
        ])
        .materialize();
        let separate = run(&[
            "Add a red 2x4 brick at origin.",
            "Add a blue 2x4 brick.",
            "Undo.",
            "Add a yellow 2x4 brick.",
        ])
        .materialize();

        let colors =
            |m: &crate::build::Model| m.placements.iter().map(|p| p.color).collect::<Vec<_>>();
        assert_eq!(colors(&batched), colors(&separate));
    }

    #[test]
    fn a_message_of_pure_chat_produces_nothing() {
        // The most important case: a build thread must stay usable for talking.
        for chat in [
            "nice, that looks great!",
            "wow",
            "I reckon the roof needs more work",
            "haha yeah",
            "brb",
            "the tower is my favourite bit",
            "",
            "   ",
        ] {
            let outcome = interpret(&empty(), chat, "en-US");
            assert!(
                outcome.is_silent(),
                "{chat:?} should have been silent, got {outcome:?}"
            );
        }
    }

    #[test]
    fn a_command_produces_an_operation_a_reply_and_a_render() {
        let outcome = interpret(&empty(), "Add a red 2x4 brick at origin.", "en-US");
        assert_eq!(outcome.ops.len(), 1);
        assert_eq!(outcome.replies.len(), 1);
        assert!(outcome.reactions.contains(&reactions::APPLIED));
        assert!(outcome.needs_render);
        assert!(!outcome.is_silent());
    }

    #[test]
    fn a_multi_sentence_message_applies_in_order() {
        let history = run(&["Add a red 2x4 brick at origin. Add a blue 2x4 brick."]);
        let model = history.materialize();
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[0].color, 4);
        assert_eq!(model.placements[1].color, 1);
        // The second brick stacked on the first, so the conversational context
        // carried between sentences of the same message.
        assert_eq!(model.placements[1].pos.y, -24);
    }

    #[test]
    fn a_message_typed_as_several_lines_applies_every_line() {
        // Regression: newlines were collapsed before the message was split
        // into sentences, so only the first line ran and the rest vanished
        // without a word.
        let history = run(&["Add a red 2x4 brick at origin\nAdd a blue 2x4 brick"]);
        let model = history.materialize();
        assert_eq!(model.len(), 2, "both lines ran");
        assert_eq!(model.placements[0].color, 4);
        assert_eq!(model.placements[1].color, 1);
    }

    #[test]
    fn several_commands_produce_one_summary_not_a_running_commentary() {
        let outcome = interpret(
            &empty(),
            "Add a red brick. Add a blue brick. Add a yellow brick.",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 3);
        assert_eq!(outcome.replies.len(), 1, "the thread should not be spammed");
    }

    #[test]
    fn a_failure_midway_stops_the_rest() {
        let outcome = interpret(
            &empty(),
            "Add a red 2x4 brick at origin. Move the gatehouse east. Add a blue brick.",
            "en-US",
        );
        // The first sentence was applied ...
        assert_eq!(outcome.ops.len(), 1);
        // ... the second reported ...
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert!(outcome.replies.iter().any(|r| r.contains("gatehouse")));
        // ... and the third never ran.
        assert!(!outcome.replies.iter().any(|r| r.contains("blue")));
    }

    #[test]
    fn chat_mixed_with_a_command_acts_only_on_the_command() {
        let outcome = interpret(
            &empty(),
            "ooh nice idea! Add a red 2x4 brick at origin. love it",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 1);
        assert_eq!(outcome.replies.len(), 1);
    }

    #[test]
    fn a_comment_is_acknowledged_but_changes_nothing() {
        let outcome = interpret(&empty(), "Note: this is temporary support.", "en-US");
        assert!(outcome.ops.is_empty());
        assert!(outcome.replies.is_empty());
        assert_eq!(outcome.reactions, vec![reactions::COMMENT]);
        assert!(!outcome.needs_render);
        assert!(!outcome.is_silent(), "the reaction is still feedback");
    }

    #[test]
    fn a_malformed_command_is_reported_not_ignored() {
        let outcome = interpret(&empty(), "Add the", "en-US");
        assert!(outcome.ops.is_empty());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert_eq!(outcome.replies.len(), 1);
    }

    #[test]
    fn an_unknown_part_asks_for_a_lookup() {
        let outcome = interpret(&empty(), "Add a red 3x12 brick at origin.", "en-US");
        let pending = outcome.pending_lookup.expect("expected a pending lookup");
        assert_eq!(pending.kind, PartKind::Brick);
        assert!(pending.sentence.contains("3x12"));
        assert!(
            outcome.ops.is_empty(),
            "nothing is applied until it resolves"
        );
    }

    #[test]
    fn a_lookup_pauses_the_rest_of_the_message() {
        let outcome = interpret(
            &empty(),
            "Add a red 2x4 brick at origin. Add a 3x12 brick. Add a blue brick.",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 1, "only the first sentence ran");
        assert!(outcome.pending_lookup.is_some());
    }

    // --- Part disambiguation ----------------------------------------------

    fn chosen_part() -> crate::build::PartRef {
        crate::build::PartRef::new("3009", "Brick 1 x 6", (1, 6), PartKind::Brick)
    }

    #[test]
    fn a_chosen_part_finishes_the_sentence_that_was_waiting() {
        let history = empty();
        let paused = interpret(&history, "Add a red 3x12 brick at origin.", "en-US");
        let pending = paused.pending_lookup.expect("expected a pending lookup");
        assert!(paused.ops.is_empty());

        let outcome = process_message_with_part(
            &history,
            &pending.sentence,
            "en-US",
            &builders(),
            "111",
            &chosen_part(),
            MAX_MODEL_SIZE,
        );
        assert_eq!(outcome.ops.len(), 1);
        assert!(outcome.needs_render);

        let mut applied = history;
        for op in &outcome.ops {
            applied.push(op.clone(), "111");
        }
        let model = applied.materialize();
        assert_eq!(model.len(), 1);
        assert_eq!(model.placements[0].part.ldraw_id, "3009");
        assert_eq!(model.placements[0].color, 4, "the sentence's colour won");
    }

    #[test]
    fn a_chosen_part_also_serves_a_structure() {
        let history = empty();
        let outcome = process_message_with_part(
            &history,
            "Build a wall 4 studs long 2 bricks high using 3x12 bricks.",
            "en-US",
            &builders(),
            "111",
            &chosen_part(),
            MAX_MODEL_SIZE,
        );
        assert_eq!(outcome.ops.len(), 1);
        let mut applied = history;
        for op in &outcome.ops {
            applied.push(op.clone(), "111");
        }
        let model = applied.materialize();
        assert!(!model.is_empty());
        assert!(
            model.placements.iter().all(|p| p.part.ldraw_id == "3009"),
            "the whole wall is built from the chosen part"
        );
    }

    #[test]
    fn a_chosen_part_does_not_disturb_a_sentence_that_never_needed_one() {
        let history = empty();
        let outcome = process_message_with_part(
            &history,
            "Add a red 2x4 brick at origin.",
            "en-US",
            &builders(),
            "111",
            &chosen_part(),
            MAX_MODEL_SIZE,
        );
        let mut applied = history;
        for op in &outcome.ops {
            applied.push(op.clone(), "111");
        }
        assert_eq!(
            applied.materialize().placements[0].part.ldraw_id,
            "3001",
            "the catalogue still answers what it can"
        );
    }

    #[test]
    fn undo_and_redo_move_the_history() {
        let mut history = run(&["Add a red 2x4 brick at origin."]);
        assert_eq!(history.materialize().len(), 1);

        let outcome = interpret(&history, "Undo.", "en-US");
        assert_eq!(outcome.cursor, Some(0));
        assert!(outcome.needs_render);
        history.cursor = outcome.cursor.unwrap();
        assert!(history.materialize().is_empty());

        let outcome = interpret(&history, "Redo.", "en-US");
        assert_eq!(outcome.cursor, Some(1));
        history.cursor = outcome.cursor.unwrap();
        assert_eq!(history.materialize().len(), 1);
    }

    #[test]
    fn an_undo_followed_by_a_command_writes_at_the_freed_revision() {
        // Regression: the outcome reported only the *final* cursor, so the
        // Discord layer appended the new operation at the pre-undo index and
        // then set the cursor below it, losing the new brick.
        let history = run(&["Add a red 2x4 brick at origin.", "Add a blue 2x4 brick."]);
        assert_eq!(history.ops.len(), 2);

        let outcome = interpret(&history, "Undo. Add a yellow 2x4 brick.", "en-US");

        assert_eq!(outcome.ops.len(), 1);
        assert_eq!(
            outcome.ops_start,
            Some(1),
            "the new operation replaces the undone one"
        );
        assert_eq!(
            outcome.cursor,
            Some(2),
            "the cursor ends past the new operation"
        );

        // Applying the outcome the way the session layer does must leave the
        // yellow brick in the model.
        let mut applied = history;
        applied.cursor = outcome.ops_start.unwrap();
        for op in &outcome.ops {
            applied.push(op.clone(), "alice");
        }
        applied.cursor = outcome.cursor.unwrap();

        let model = applied.materialize();
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[1].color, 14, "the yellow brick survived");
    }

    #[test]
    fn a_plain_command_reports_where_its_operations_start() {
        let history = run(&["Add a red 2x4 brick at origin."]);
        let outcome = interpret(&history, "Add a blue 2x4 brick.", "en-US");
        assert_eq!(outcome.ops_start, Some(1));
        assert_eq!(outcome.cursor, Some(2));
    }

    #[test]
    fn a_message_that_changes_nothing_reports_no_revision_movement() {
        let history = run(&["Add a red 2x4 brick at origin."]);
        let outcome = interpret(&history, "How tall is it?", "en-US");
        assert!(outcome.ops_start.is_none());
        assert!(outcome.cursor.is_none());
    }

    #[test]
    fn undo_with_nothing_to_undo_says_so() {
        let outcome = interpret(&empty(), "Undo.", "en-US");
        assert!(outcome.cursor.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert!(!outcome.needs_render);
    }

    #[test]
    fn reverting_to_an_impossible_revision_is_reported() {
        let history = run(&["Add a red 2x4 brick at origin."]);
        let outcome = interpret(&history, "Revert to revision 99.", "en-US");
        assert!(outcome.cursor.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
    }

    #[test]
    fn undoing_one_persons_edit_is_recorded_separately() {
        // Regression: the sentence names a person, but the log records Discord
        // user ids. Comparing the two directly — which is what this used to do
        // — never matched, so this always answered "nothing to undo".
        let mut history = empty();
        history.push(
            Op::Rename {
                name: "Castle".to_string(),
            },
            "111",
        );
        let outcome = interpret(&history, "Undo alice's last edit.", "en-US");
        assert_eq!(
            outcome.skip_last_by.as_deref(),
            Some("111"),
            "the name resolved to a user id"
        );
        assert!(outcome.needs_render);
        assert!(outcome.reactions.contains(&reactions::APPLIED));
    }

    #[test]
    fn undoing_someone_elses_edit_leaves_yours_alone() {
        let mut history = empty();
        history.push(add_brick(4), "111");
        history.push(add_brick(1), "222");

        let outcome = interpret(&history, "Undo bob's last edit.", "en-US");
        assert_eq!(outcome.skip_last_by.as_deref(), Some("222"));

        // Marking bob's revision skipped leaves alice's brick standing.
        let mut applied = history;
        applied.ops[1].skipped = true;
        let model = applied.materialize();
        assert_eq!(model.len(), 1);
        assert_eq!(model.placements[0].color, 4);
    }

    #[test]
    fn undoing_a_stranger_says_who_it_does_not_know() {
        let mut history = empty();
        history.push(add_brick(4), "111");
        let outcome = interpret(&history, "Undo mallory's last edit.", "en-US");

        assert!(outcome.skip_last_by.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert!(
            outcome.replies[0].contains("mallory"),
            "got: {}",
            outcome.replies[0]
        );
    }

    #[test]
    fn a_name_can_be_abbreviated_but_never_guessed_between_two_people() {
        let unique = Builders::new([
            ("Alice".to_string(), "111".to_string()),
            ("Bob".to_string(), "222".to_string()),
        ]);
        assert_eq!(unique.resolve("al"), Some("111"));
        assert_eq!(unique.resolve("ALICE"), Some("111"));
        // A user id always resolves, so an uncached member is still nameable.
        assert_eq!(unique.resolve("222"), Some("222"));
        assert_eq!(unique.resolve("nobody"), None);
        assert_eq!(unique.resolve(""), None);

        let ambiguous = Builders::new([
            ("Alice".to_string(), "111".to_string()),
            ("Alan".to_string(), "333".to_string()),
        ]);
        assert_eq!(
            ambiguous.resolve("al"),
            None,
            "an ambiguous prefix must not pick a person"
        );
        assert_eq!(ambiguous.resolve("alice"), Some("111"));
    }

    #[test]
    fn who_added_something_answers_with_a_mention() {
        let mut history = empty();
        history.push(add_brick(4), "111");
        let outcome = interpret(&history, "Who added that?", "en-US");
        assert_eq!(outcome.replies.len(), 1);
        assert!(
            outcome.replies[0].contains("<@111>"),
            "got: {}",
            outcome.replies[0]
        );
    }

    #[test]
    fn export_render_help_and_finish_set_their_flags() {
        let outcome = interpret(&empty(), "Export.", "en-US");
        assert!(outcome.needs_export);

        let outcome = interpret(&empty(), "Render.", "en-US");
        assert!(outcome.needs_render);

        let outcome = interpret(&empty(), "Help.", "en-US");
        assert_eq!(outcome.replies.len(), 1);
        assert!(outcome.replies[0].contains("Add a red 2x4 brick"));

        let outcome = interpret(&empty(), "Finish.", "en-US");
        assert!(outcome.finished);
    }

    #[test]
    fn help_for_a_verb_answers_with_that_verbs_own_text() {
        let general = interpret(&empty(), "Help.", "en-US");
        let select = interpret(&empty(), "Help select.", "en-US");
        assert_eq!(select.replies.len(), 1);
        assert_ne!(
            select.replies[0], general.replies[0],
            "a topic should not just repeat the general help"
        );
        assert!(select.replies[0].to_lowercase().contains("select"));

        // An unrecognised topic falls back to the general text rather than
        // erroring.
        let unknown = interpret(&empty(), "Help wobble.", "en-US");
        assert!(unknown.reactions.is_empty(), "got: {unknown:?}");
        assert_eq!(unknown.replies, general.replies);
    }

    #[test]
    fn a_query_answers_without_changing_anything() {
        let history = run(&["Add a red 2x4 brick at origin.", "Add a blue 2x4 brick."]);
        let outcome = interpret(&history, "How tall is it?", "en-US");

        assert!(outcome.ops.is_empty());
        assert!(!outcome.needs_render);
        assert_eq!(outcome.replies.len(), 1);
        assert!(
            outcome.replies[0].contains('2'),
            "got: {}",
            outcome.replies[0]
        );
    }

    #[test]
    fn a_question_about_something_we_have_never_heard_of_is_silent() {
        // This used to answer "I don't know anything called 'roof'". In a
        // thread that is also a conversation, "where is everyone?" has the
        // very same shape, so the bot barked at ordinary chat. A question
        // changes nothing, so saying nothing is the safe reading.
        for chat in [
            "Where is the roof?",
            "Where is everyone?",
            "How tall is the gatehouse?",
            "Who added the thing?",
        ] {
            let outcome = interpret(&empty(), chat, "en-US");
            assert!(
                outcome.is_silent(),
                "{chat:?} should have been silent, got {outcome:?}"
            );
        }
    }

    #[test]
    fn a_question_about_something_real_is_still_answered() {
        let history = run(&["Add a red 2x4 brick at origin.", "Call this tower."]);
        let outcome = interpret(&history, "Where is tower?", "en-US");
        assert_eq!(outcome.replies.len(), 1, "got: {outcome:?}");
        assert!(!outcome.is_silent());
    }

    #[test]
    fn a_verb_led_query_that_cannot_run_is_still_reported() {
        // Only the question-word forms are ambiguous with chat. `Find` and
        // `Show` open with a verb, so a builder who gets one wrong is told.
        let outcome = interpret(&empty(), "Find nothing at all.", "en-US");
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
    }

    #[test]
    fn replies_are_localised() {
        let english = interpret(&empty(), "Add a red 2x4 brick at origin.", "en-US");
        let finnish = interpret(&empty(), "Add a red 2x4 brick at origin.", "fi-FI");
        assert_ne!(english.replies, finnish.replies);
        assert_eq!(english.ops, finnish.ops, "the model is language-neutral");
    }

    #[test]
    fn the_design_documents_example_session_runs_end_to_end() {
        let history = run(&[
            "Add a light bluish gray 6x6 plate at origin.",
            "Build a wall 6 studs long 2 bricks high.",
            "Build a tower 4 bricks high.",
            "Paint the highest brick red.",
        ]);
        let model = history.materialize();
        assert!(
            model.len() > 10,
            "expected a real model, got {}",
            model.len()
        );
        assert!(model.height_in_bricks() > 4);
        // The painted brick is the top one.
        assert_eq!(model.highest().unwrap().color, 4);
    }

    // --- Max model size ------------------------------------------------

    #[test]
    fn a_command_that_would_exceed_the_configured_max_size_is_rejected() {
        let outcome = process_message(
            &empty(),
            "Add a red 2x4 brick at origin.",
            "en-US",
            &builders(),
            "111",
            MaxModelSize::cube(1),
        );
        assert!(outcome.ops.is_empty(), "nothing should have been placed");
        assert!(
            outcome.replies.iter().any(|r| r.contains("1x1x1")),
            "expected the size-limit message, got {:?}",
            outcome.replies
        );
    }

    #[test]
    fn a_command_that_fits_the_configured_max_size_is_applied() {
        let outcome = process_message(
            &empty(),
            "Add a red 1x1 brick at origin.",
            "en-US",
            &builders(),
            "111",
            MaxModelSize::cube(1),
        );
        assert_eq!(outcome.ops.len(), 1);
    }

    #[test]
    fn many_small_commands_cannot_creep_a_model_past_the_limit_either() {
        // The cap looks at the whole model, not just what one command adds,
        // so a second small brick placed far from the first is refused even
        // though neither command alone looks oversized.
        let history = run(&["Add a red 1x1 brick at origin."]);
        let outcome = process_message(
            &history,
            "Add a blue 1x1 brick east 5.",
            "en-US",
            &builders(),
            "111",
            MaxModelSize::cube(1),
        );
        assert!(outcome.ops.is_empty(), "nothing should have been placed");
    }

    #[test]
    fn an_already_oversized_model_is_not_punished_for_edits_that_dont_grow_it() {
        // A model built before the limit existed — or under a looser guild
        // override — must not have every future edit rejected just because
        // it is already bigger than the box. Only growing past the limit is
        // refused.
        let history = process_message(
            &empty(),
            "Add a red 2x4 brick at origin.",
            "en-US",
            &builders(),
            "111",
            MaxModelSize::cube(100),
        )
        .ops
        .into_iter()
        .fold(empty(), |mut h, op| {
            h.push(op, "111");
            h
        });

        let outcome = process_message(
            &history,
            "Paint it blue.",
            "en-US",
            &builders(),
            "111",
            MaxModelSize::cube(1),
        );
        assert_eq!(outcome.ops.len(), 1, "a repaint does not grow the model");
    }

    // --- Placement proposals -----------------------------------------------

    #[test]
    fn a_part_on_its_own_asks_where_it_should_go() {
        let outcome = interpret(&empty(), "2x2 brick", "en-US");
        let pending = outcome
            .pending_proposal
            .as_ref()
            .expect("naming a part should ask where it goes");
        assert_eq!(pending.batches.len(), 1, "one brick has one shape");
        assert_eq!(pending.batches[0].len(), 1);
        assert_eq!(pending.batches[0][0].part.footprint, (2, 2));
    }

    #[test]
    fn a_proposal_applies_nothing_until_it_is_chosen() {
        // The whole point of asking: nothing is built and nothing is recorded
        // while the question is still open.
        let outcome = interpret(&empty(), "2x2 brick", "en-US");
        assert!(outcome.ops.is_empty());
        assert!(outcome.ops_start.is_none());
        assert!(!outcome.needs_render);
    }

    #[test]
    fn a_pending_proposal_is_not_silence() {
        // A proposal the session never notices would be worse than not asking.
        let outcome = interpret(&empty(), "2x2 brick", "en-US");
        assert!(!outcome.is_silent());
    }

    #[test]
    fn several_of_a_part_offer_every_way_they_could_sit() {
        let outcome = interpret(&empty(), "3 2x2 bricks", "en-US");
        let pending = outcome.pending_proposal.expect("should ask");
        assert_eq!(pending.batches.len(), 3, "stacked, and two ways along");
        assert!(pending.batches.iter().all(|b| b.len() == 3));
    }

    #[test]
    fn saying_how_they_sit_leaves_only_that_shape_to_place() {
        let outcome = interpret(&empty(), "3 2x2 bricks stacked", "en-US");
        let pending = outcome.pending_proposal.expect("should ask");
        assert_eq!(pending.batches.len(), 1);
        // Really stacked: a brick height apart.
        assert_eq!(pending.batches[0][1].pos.y, -24);
    }

    #[test]
    fn an_unusual_part_is_identified_before_it_is_placed() {
        // The part picker answers first; only then is there something whose
        // position could be proposed.
        let outcome = interpret(&empty(), "1x7 brick", "en-US");
        assert!(outcome.pending_proposal.is_none());
        assert!(outcome.pending_lookup.is_some());
    }

    #[test]
    fn a_drawing_with_no_location_asks_where_it_goes() {
        let text = "Build from top:\n```\nb = red 1x1 brick\n\nbb\n```";
        let outcome = interpret(&empty(), text, "en-US");
        let pending = outcome
            .pending_proposal
            .as_ref()
            .expect("a drawing that never says where should ask");
        assert_eq!(pending.batches.len(), 1, "a drawing is one shape");
        assert_eq!(pending.batches[0].len(), 2, "two 1x1 bricks");
        assert!(outcome.ops.is_empty(), "nothing built until it is placed");
    }

    #[test]
    fn a_drawing_that_says_where_it_goes_is_unchanged() {
        let text = "Build from top at origin:\n```\nb = red 1x1 brick\n\nbb\n```";
        let outcome = interpret(&empty(), text, "en-US");
        assert!(outcome.pending_proposal.is_none());
        assert_eq!(outcome.ops.len(), 1);
    }

    #[test]
    fn a_verb_still_places_directly_without_asking() {
        // Saying it in full must keep working exactly as it did: the proposal
        // path is for the sentences that used to do nothing at all.
        let outcome = interpret(&empty(), "Add a red 2x4 brick at origin.", "en-US");
        assert!(outcome.pending_proposal.is_none());
        assert_eq!(outcome.ops.len(), 1);
    }

    #[test]
    fn a_proposal_offers_only_legal_places_to_put_it() {
        // The engine and the dialog meet here: whatever the batch, every offer
        // has to be somewhere the part could really go.
        let history = run(&["Add a red 2x4 brick at origin."]);
        let model = history.materialize();
        let pending = interpret(&history, "2x2 brick", "en-US")
            .pending_proposal
            .expect("should ask");
        let candidates = crate::build::propose::propose_any(&model, &pending.batches, 4);
        assert!(!candidates.is_empty());
        for candidate in candidates {
            for placed in &candidate.placements {
                assert!(crate::build::legal::is_on_lattice(
                    placed.pos,
                    &placed.part,
                    placed.orientation
                ));
                assert!(placed.pos.y <= 0);
            }
        }
    }

    // --- Legal geometry ----------------------------------------------------

    /// The invariants a model must hold after any command.
    ///
    /// Deliberately not "nothing floats": an overhang is a real thing people
    /// build, and the documented example session has one — its 6-stud wall
    /// runs off the edge of the 6x6 plate underneath it. What must never
    /// happen is a part off the grid, inside another, or below the ground.
    fn assert_legal(model: &crate::build::Model) {
        assert!(model.collisions().is_empty(), "parts overlap");
        for p in &model.placements {
            assert!(
                crate::build::legal::is_on_lattice(p.pos, &p.part, p.orientation),
                "{} is off the stud grid at {:?}",
                p.part.name,
                p.pos
            );
            assert!(
                p.pos.y <= 0,
                "{} is underground at {:?}",
                p.part.name,
                p.pos
            );
        }
    }

    #[test]
    fn an_off_grid_coordinate_is_nudged_onto_the_stud_grid() {
        // Bare coordinates used to be taken completely literally, so a typo
        // put down a brick that connected to nothing at all.
        let model = run(&["Add a red 2x4 brick at 3 5 7."]).materialize();
        assert_eq!(model.placements[0].pos, crate::build::IVec3::ZERO);
        assert_legal(&model);
    }

    #[test]
    fn a_move_measured_in_the_wrong_unit_lands_back_on_the_grid() {
        // A plate is a vertical measure; sideways it is 16 LDU, which is not
        // a stud and leaves the brick attached to nothing.
        let model =
            run(&["Add a red 2x4 brick at origin.", "Move it east 2 plates."]).materialize();
        assert_eq!(model.placements[0].pos.x, 20);
        assert_legal(&model);
    }

    #[test]
    fn a_second_brick_in_the_same_place_is_stacked_rather_than_buried() {
        let model = run(&[
            "Add a red 2x4 brick at origin.",
            "Add a blue 2x4 brick at origin.",
        ])
        .materialize();
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[1].pos, crate::build::IVec3::new(0, -24, 0));
        assert_legal(&model);
    }

    #[test]
    fn a_brick_added_in_mid_air_comes_down_to_rest() {
        let model = run(&["Add a red 2x4 brick at 0 -96 0."]).materialize();
        assert_eq!(model.placements[0].pos.y, 0, "it should have landed");
        assert_legal(&model);
    }

    #[test]
    fn a_part_of_unknown_size_obeys_the_grid_like_any_other() {
        // A part whose name carries no size used to drop out of every check,
        // so it could sit anywhere at all and overlap anything.
        let chosen = crate::build::PartRef::new("99999", "Mystery Thing", (0, 0), PartKind::Other);
        let history = empty();
        let paused = interpret(&history, "Add a red 3x12 brick at 3 5 7.", "en-US");
        let pending = paused.pending_lookup.expect("expected a pending lookup");

        let outcome = process_message_with_part(
            &history,
            &pending.sentence,
            "en-US",
            &builders(),
            "111",
            &chosen,
            MAX_MODEL_SIZE,
        );
        let mut history = empty();
        for op in outcome.ops {
            history.push(op, "111");
        }
        let model = history.materialize();
        assert_eq!(model.len(), 1);
        assert!(!model.placements[0].part.has_footprint());
        assert_legal(&model);
    }

    #[test]
    fn the_operation_written_to_the_history_is_the_corrected_one() {
        // Correcting only the model would leave the log wrong, and the next
        // reload would replay the illegal position straight back in.
        let outcome = interpret(&empty(), "Add a red 2x4 brick at 3 5 7.", "en-US");
        match outcome.ops.first().expect("an operation") {
            Op::Add { pos, .. } => assert_eq!(*pos, crate::build::IVec3::ZERO),
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn a_corrected_placement_is_reported_to_the_builder() {
        // Sideways off the grid, at a legal height: the nudge is all there is
        // to say about it.
        let nudged = interpret(&empty(), "Add a red 2x4 brick at 3 0 7.", "en-US");
        assert_eq!(nudged.replies.len(), 2, "{:?}", nudged.replies);
        assert!(
            nudged.replies[1].contains("stud grid"),
            "{:?}",
            nudged.replies
        );

        // Dropped onto what was already there.
        let stacked = interpret(
            &run(&["Add a red 2x4 brick at origin."]),
            "Add a blue 2x4 brick at origin.",
            "en-US",
        );
        assert!(
            stacked.replies[1].contains("Lifted"),
            "{:?}",
            stacked.replies
        );

        // And a command that was legal to begin with says nothing extra.
        let clean = interpret(&empty(), "Add a red 2x4 brick at origin.", "en-US");
        assert_eq!(clean.replies.len(), 1, "{:?}", clean.replies);
    }

    #[test]
    fn replaying_a_history_never_moves_anything() {
        // Sessions recorded before any of this existed still hold illegal
        // geometry. Replay must reproduce them exactly, or every old thread
        // would redraw itself the first time someone opened it.
        let mut history = empty();
        history.push(
            Op::Add {
                part: crate::build::PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
                color: 4,
                pos: crate::build::IVec3::new(3, 5, 7),
                orientation: crate::build::Orientation::DEFAULT,
                repeat: None,
                group: None,
            },
            "111",
        );
        let model = history.materialize();
        assert_eq!(model.placements[0].pos, crate::build::IVec3::new(3, 5, 7));
    }

    #[test]
    fn every_sentence_of_the_example_session_leaves_a_legal_model() {
        // The session from the docs, checked sentence by sentence: a model
        // that is legal at the end but was illegal along the way would still
        // have been rendered and shown to the thread that way.
        let script = [
            "Add a light bluish gray 6x6 plate at origin.",
            "Build a wall 6 studs long 2 bricks high.",
            "Build a tower 4 bricks high.",
            "Paint the highest brick red.",
        ];
        let mut history = empty();
        for text in script {
            let outcome = interpret(&history, text, "en-US");
            for op in outcome.ops {
                history.push(op, "111");
            }
            if let Some(cursor) = outcome.cursor {
                history.cursor = cursor;
            }
            assert_legal(&history.materialize());
        }
    }

    #[test]
    fn nth_sentence_recovers_the_original_text() {
        let text = "Add a brick. Add a 3x12 brick. Undo.";
        assert_eq!(nth_sentence(text, 0), "Add a brick");
        assert_eq!(nth_sentence(text, 1), "Add a 3x12 brick");
        assert_eq!(nth_sentence(text, 9), text.trim());
    }

    // --- Drawings ----------------------------------------------------------

    /// The drawing from the docs: two blue bricks over a red pair, seen from
    /// above.
    const DRAWING: &str = "Build from top at origin:\n\
        ```\n\
        b = blue 1x1 brick\n\
        r = red 1x1 brick\n\
        \n\
        .bb.\n\
        rbbr\n\
        ```";

    #[test]
    fn a_drawing_in_a_code_block_builds_the_model() {
        let outcome = interpret(&empty(), DRAWING, "en-US");
        assert!(
            outcome.reactions.contains(&reactions::APPLIED),
            "got: {outcome:?}"
        );
        assert!(outcome.needs_render);

        // One operation, so one undo step — exactly as a wall behaves.
        assert_eq!(outcome.ops.len(), 1);
        let mut history = empty();
        for op in outcome.ops {
            history.push(op, "111");
        }
        let model = history.materialize();
        assert_eq!(model.len(), 6, "four blue and two red");
        assert_eq!(model.count_matching(|p| p.color == 1), 4);
        assert_eq!(model.count_matching(|p| p.color == 4), 2);
        // And every part is individually addressable afterwards.
        let ids: std::collections::BTreeSet<_> = model.placements.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn a_drawing_is_one_undo_step() {
        let history = run(&[DRAWING]);
        assert_eq!(history.materialize().len(), 6);
        let after = run(&[DRAWING, "Undo."]);
        assert!(after.materialize().is_empty());
    }

    #[test]
    fn a_code_block_with_no_header_sentence_is_ignored() {
        // A pasted code sample is not a command, and a build thread has to
        // stay usable for talking.
        let outcome = interpret(&empty(), "```\n.bb.\nrbbr\n```", "en-US");
        assert!(outcome.is_silent(), "got: {outcome:?}");
    }

    #[test]
    fn chat_around_a_drawing_only_acts_on_the_drawing() {
        let text = format!("here's my idea\n{DRAWING}\nwhat do you reckon?");
        let outcome = interpret(&empty(), &text, "en-US");
        assert_eq!(outcome.ops.len(), 1);
        assert!(outcome.reactions.contains(&reactions::APPLIED));
    }

    #[test]
    fn a_grid_that_cannot_be_read_is_reported_not_ignored() {
        // A drawing opens with a verb, so its problems are worth reporting —
        // unlike a question, which falls back to silence.
        for (text, hint) in [
            ("Build from top:\n```\nb = blue 1x1 brick\n\n.bz.\n```", "z"),
            (
                "Build from top:\n```\nb = blue 1x1 brick\n\nbbb\nbb\n```",
                "wide",
            ),
        ] {
            let outcome = interpret(&empty(), text, "en-US");
            assert!(outcome.ops.is_empty(), "nothing should be placed: {text}");
            assert!(outcome.reactions.contains(&reactions::PROBLEM));
            assert_eq!(outcome.replies.len(), 1);
            assert!(
                outcome.replies[0].contains(hint),
                "expected {hint:?} in: {}",
                outcome.replies[0]
            );
        }
    }

    #[test]
    fn a_drawing_needing_one_part_lookup_pauses_with_its_block_intact() {
        // 1x7 is not in the built-in catalogue, so it has to be looked up.
        // Says where it goes, so the replay applies it rather than asking —
        // a drawing with no location is offered choices instead, which
        // `a_drawing_with_no_location_asks_where_it_goes` covers.
        let text = "Build from top at origin:\n```\nq = 1x7 brick\n\nqqqqqqq\n```";
        let outcome = interpret(&empty(), text, "en-US");

        let pending = outcome
            .pending_lookup
            .as_ref()
            .expect("an unknown part should pause");
        // The parked sentence must carry the whole drawing, or replaying it
        // would be a header with nothing to build from.
        assert!(pending.sentence.contains("q = 1x7 brick"));
        assert!(pending.sentence.contains("qqqqqqq"));

        // And the replay, with a part chosen, finishes the drawing.
        let chosen = crate::build::PartRef::new("3008", "Brick 1 x 7", (1, 7), PartKind::Brick);
        let resumed = process_message_with_part(
            &empty(),
            &pending.sentence,
            "en-US",
            &builders(),
            "111",
            &chosen,
            MAX_MODEL_SIZE,
        );
        assert_eq!(resumed.ops.len(), 1);
        let mut history = empty();
        for op in resumed.ops {
            history.push(op, "111");
        }
        assert_eq!(history.materialize().len(), 1, "one 1x7 brick");
    }

    #[test]
    fn two_unknown_legend_parts_are_refused_rather_than_looping() {
        // The dropdown answers one question, and a replay hands its answer to
        // whichever entry asks first — so a second would resolve the wrong one
        // forever.
        let text = "Build from top:\n```\nq = 1x7 brick\nz = 1x9 plate\n\nqz\n```";
        let outcome = interpret(&empty(), text, "en-US");
        assert!(outcome.pending_lookup.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
    }

    #[test]
    fn a_drawing_survives_a_round_trip_through_the_view() {
        // The headline: what the bot prints back must rebuild the same model.
        let history = run(&[DRAWING]);
        let model = history.materialize();

        let drawn =
            crate::build::textview::render(&model, crate::build::units::GridView::Top, None)
                .expect("a model should draw");
        let block = crate::build::textview::to_block(&drawn);

        let rebuilt = run(&[&block]);
        assert_eq!(
            rebuilt.materialize().placements.len(),
            model.placements.len(),
            "round trip changed the part count\n{block}"
        );
        // Same parts, same colours, same places.
        let describe = |m: &crate::build::Model| {
            let mut rows: Vec<_> = m
                .placements
                .iter()
                .map(|p| (p.part.ldraw_id.clone(), p.color, p.pos))
                .collect();
            rows.sort();
            rows
        };
        assert_eq!(
            describe(&rebuilt.materialize()),
            describe(&model),
            "round trip moved something\n{block}"
        );
    }

    #[test]
    fn nth_sentence_carries_the_block_it_was_written_with() {
        // What gets parked when a drawing pauses on an unknown part. Without
        // the fence the replayed sentence would be a header with nothing to
        // build from.
        let text = "Build from top:\n```\nb = blue 1x1 brick\n\n..bb..\n```";
        let parked = nth_sentence(text, 0);
        assert!(parked.starts_with("Build from top:"));
        assert!(parked.contains("b = blue 1x1 brick"));
        assert!(parked.contains("..bb.."));
        // And it must mean the same thing when it comes back through.
        assert_eq!(nth_sentence(&parked, 0), parked);
    }

    #[test]
    fn a_fence_does_not_shift_the_sentences_after_it() {
        // Sentence indices are what `pending_lookup` is recorded against, so a
        // captured block must not count as one.
        let text = "Build from top:\n```\nbb\n```\nUndo.";
        assert_eq!(nth_sentence(text, 1), "Undo");
    }
    #[test]
    fn ordinary_chat_is_recognised_without_a_model() {
        for chat in ["how's everyone doing?", "that looks great", "brb"] {
            assert!(is_ordinary_chat(chat), "should be chat: {chat}");
        }

        for command in [
            "Add a red 2x4 brick at origin.",
            "Undo.",
            "How tall is the tower?",
        ] {
            assert!(!is_ordinary_chat(command), "should be a command: {command}");
        }
    }
}

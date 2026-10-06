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

    // Extract the final, net operations from the history buffer
    if let Some(start) = outcome.ops_start {
        outcome.ops = working
            .ops
            .get(start..working.cursor)
            .unwrap_or(&[])
            .iter()
            .map(|entry| entry.op.clone())
            .collect();
    }

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
mod tests;

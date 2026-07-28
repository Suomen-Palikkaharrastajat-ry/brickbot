//! The pure heart of the thread listener.
//!
//! One function takes a chat message and the session's history, and returns
//! everything the Discord layer needs to do: which operations to record, what
//! to say, which reactions to add, whether to re-render. Keeping the decisions
//! here means the serenity code is a thin shell and the behaviour is tested
//! without a network, a database or a mock.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::{Meta, Statement};
use super::dsl::parser::{parse_message, SentenceOutcome};
use super::lower::{lower_command, LowerError};
use super::model::PartKind;
use super::ops::{apply, History, Op};
use super::{queries, report};

/// A part the bot could not resolve, which the `/part` picker will answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLookup {
    pub query: String,
    pub kind: PartKind,
    /// The sentence to retry once a part has been chosen.
    pub sentence: String,
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
    /// How the history cursor moved, when `Undo`/`Redo`/`Revert` was used.
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

/// Interprets one chat message against the session's history.
///
/// Sentences are processed in order. A failure stops the rest of the message
/// rather than silently applying the remainder of a sequence whose earlier
/// steps did not happen.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn process_message(history: &History, text: &str, locale: &str) -> Outcome {
    let mut outcome = Outcome::default();
    // A working copy, so each sentence sees the effect of the previous one.
    let mut working = history.clone();

    for (index, sentence) in parse_message(text).into_iter().enumerate() {
        let statement = match sentence {
            // Ordinary chat: contribute nothing whatsoever.
            SentenceOutcome::Ignored => continue,
            SentenceOutcome::Error(error) => {
                outcome.problem(report::describe_parse_error(&error, locale));
                return outcome;
            }
            SentenceOutcome::Statement(statement) => statement,
        };

        match statement {
            Statement::Comment => {
                if !outcome.reactions.contains(&reactions::COMMENT) {
                    outcome.reactions.push(reactions::COMMENT);
                }
            }

            Statement::Meta(meta) => {
                if !apply_meta(&mut outcome, &mut working, &meta, locale) {
                    return outcome;
                }
            }

            Statement::Query(query) => {
                let model = working.materialize();
                match queries::answer(&model, &working, &query) {
                    Ok(answer) => outcome
                        .replies
                        .push(report::describe_answer(&answer, &model, locale)),
                    Err(error) => {
                        outcome.problem(report::describe_error(&error, locale));
                        return outcome;
                    }
                }
            }

            Statement::Command(command) => {
                let model = working.materialize();
                match lower_command(&model, &command) {
                    Ok(ops) => {
                        for op in ops {
                            let mut next = working.materialize();
                            let effect = apply(&mut next, working.next_rev(), &op);
                            outcome
                                .replies
                                .push(report::describe_effect(&op, &effect, locale));
                            working.push(op.clone(), "");
                            outcome.ops.push(op);
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
                        return outcome;
                    }
                    Err(error) => {
                        outcome.problem(report::describe_error(&error, locale));
                        return outcome;
                    }
                }
            }
        }
    }

    // Only the last reply is worth posting when several commands succeeded;
    // a running commentary on every brick would drown the thread.
    if outcome.ops.len() > 1 && outcome.replies.len() > 1 {
        let last = outcome.replies.pop().unwrap_or_default();
        outcome.replies.clear();
        outcome.replies.push(last);
    }

    outcome
}

/// Applies a session-level command. Returns false when processing should stop.
fn apply_meta(outcome: &mut Outcome, working: &mut History, meta: &Meta, locale: &str) -> bool {
    use rust_i18n::t;

    match meta {
        Meta::Undo => {
            if working.undo() {
                outcome.cursor = Some(working.revision());
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
        Meta::UndoBy(author) => {
            if working.undo_last_by(author) {
                outcome.skip_last_by = Some(author.clone());
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
                outcome.cursor = Some(working.revision());
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
                outcome.cursor = Some(working.revision());
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
        Meta::Rename(name) => {
            let op = Op::Rename { name: name.clone() };
            working.push(op.clone(), "");
            outcome.ops.push(op);
            outcome
                .replies
                .push(t!("command.build.renamed", locale = locale).to_string());
            outcome.applied();
        }
        Meta::Export => {
            outcome.needs_export = true;
            outcome.applied();
        }
        Meta::Render => {
            outcome.needs_render = true;
            outcome.applied();
        }
        Meta::Help => outcome.replies.push(report::help_text(locale)),
        Meta::Finish => {
            outcome.finished = true;
            outcome.applied();
        }
    }
    true
}

/// Recovers the nth sentence of a message, for retrying after a part lookup.
fn nth_sentence(text: &str, index: usize) -> String {
    super::dsl::lexer::split_sentences(&super::dsl::lexer::strip_discord_noise(text))
        .into_iter()
        .nth(index)
        .unwrap_or_else(|| text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> History {
        History::new("Castle")
    }

    /// Runs a script, returning the resulting history.
    fn run(script: &[&str]) -> History {
        let mut history = empty();
        for text in script {
            let outcome = process_message(&history, text, "en-US");
            for op in outcome.ops {
                history.push(op, "alice");
            }
            if let Some(cursor) = outcome.cursor {
                history.cursor = cursor;
            }
        }
        history
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
            let outcome = process_message(&empty(), chat, "en-US");
            assert!(
                outcome.is_silent(),
                "{chat:?} should have been silent, got {outcome:?}"
            );
        }
    }

    #[test]
    fn a_command_produces_an_operation_a_reply_and_a_render() {
        let outcome = process_message(&empty(), "Add a red 2x4 brick at origin.", "en-US");
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
    fn several_commands_produce_one_summary_not_a_running_commentary() {
        let outcome = process_message(
            &empty(),
            "Add a red brick. Add a blue brick. Add a yellow brick.",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 3);
        assert_eq!(outcome.replies.len(), 1, "the thread should not be spammed");
    }

    #[test]
    fn a_failure_midway_stops_the_rest() {
        let outcome = process_message(
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
        let outcome = process_message(
            &empty(),
            "ooh nice idea! Add a red 2x4 brick at origin. love it",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 1);
        assert_eq!(outcome.replies.len(), 1);
    }

    #[test]
    fn a_comment_is_acknowledged_but_changes_nothing() {
        let outcome = process_message(&empty(), "Note: this is temporary support.", "en-US");
        assert!(outcome.ops.is_empty());
        assert!(outcome.replies.is_empty());
        assert_eq!(outcome.reactions, vec![reactions::COMMENT]);
        assert!(!outcome.needs_render);
        assert!(!outcome.is_silent(), "the reaction is still feedback");
    }

    #[test]
    fn a_malformed_command_is_reported_not_ignored() {
        let outcome = process_message(&empty(), "Add the", "en-US");
        assert!(outcome.ops.is_empty());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert_eq!(outcome.replies.len(), 1);
    }

    #[test]
    fn an_unknown_part_asks_for_a_lookup() {
        let outcome = process_message(&empty(), "Add a red 3x12 brick at origin.", "en-US");
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
        let outcome = process_message(
            &empty(),
            "Add a red 2x4 brick at origin. Add a 3x12 brick. Add a blue brick.",
            "en-US",
        );
        assert_eq!(outcome.ops.len(), 1, "only the first sentence ran");
        assert!(outcome.pending_lookup.is_some());
    }

    #[test]
    fn undo_and_redo_move_the_history() {
        let mut history = run(&["Add a red 2x4 brick at origin."]);
        assert_eq!(history.materialize().len(), 1);

        let outcome = process_message(&history, "Undo.", "en-US");
        assert_eq!(outcome.cursor, Some(0));
        assert!(outcome.needs_render);
        history.cursor = outcome.cursor.unwrap();
        assert!(history.materialize().is_empty());

        let outcome = process_message(&history, "Redo.", "en-US");
        assert_eq!(outcome.cursor, Some(1));
        history.cursor = outcome.cursor.unwrap();
        assert_eq!(history.materialize().len(), 1);
    }

    #[test]
    fn undo_with_nothing_to_undo_says_so() {
        let outcome = process_message(&empty(), "Undo.", "en-US");
        assert!(outcome.cursor.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
        assert!(!outcome.needs_render);
    }

    #[test]
    fn reverting_to_an_impossible_revision_is_reported() {
        let history = run(&["Add a red 2x4 brick at origin."]);
        let outcome = process_message(&history, "Revert to revision 99.", "en-US");
        assert!(outcome.cursor.is_none());
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
    }

    #[test]
    fn undoing_one_persons_edit_is_recorded_separately() {
        let mut history = empty();
        history.push(
            Op::Rename {
                name: "Castle".to_string(),
            },
            "alice",
        );
        let outcome = process_message(&history, "Undo alice's last edit.", "en-US");
        assert_eq!(outcome.skip_last_by.as_deref(), Some("alice"));
        assert!(outcome.needs_render);
    }

    #[test]
    fn export_render_help_and_finish_set_their_flags() {
        let outcome = process_message(&empty(), "Export.", "en-US");
        assert!(outcome.needs_export);

        let outcome = process_message(&empty(), "Render.", "en-US");
        assert!(outcome.needs_render);

        let outcome = process_message(&empty(), "Help.", "en-US");
        assert_eq!(outcome.replies.len(), 1);
        assert!(outcome.replies[0].contains("Add a red 2x4 brick"));

        let outcome = process_message(&empty(), "Finish.", "en-US");
        assert!(outcome.finished);
    }

    #[test]
    fn a_query_answers_without_changing_anything() {
        let history = run(&["Add a red 2x4 brick at origin.", "Add a blue 2x4 brick."]);
        let outcome = process_message(&history, "How tall is it?", "en-US");

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
    fn a_query_about_nothing_is_reported() {
        let outcome = process_message(&empty(), "Where is the roof?", "en-US");
        assert!(outcome.reactions.contains(&reactions::PROBLEM));
    }

    #[test]
    fn replies_are_localised() {
        let english = process_message(&empty(), "Add a red 2x4 brick at origin.", "en-US");
        let finnish = process_message(&empty(), "Add a red 2x4 brick at origin.", "fi-FI");
        assert_ne!(english.replies, finnish.replies);
        assert_eq!(english.ops, finnish.ops, "the model is language-neutral");
    }

    #[test]
    fn the_design_documents_example_session_runs_end_to_end() {
        let history = run(&[
            "Add a light bluish gray 8x8 plate at origin.",
            "Build a wall 8 studs long 4 bricks high.",
            "Build a tower 6 bricks high.",
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

    #[test]
    fn nth_sentence_recovers_the_original_text() {
        let text = "Add a brick. Add a 3x12 brick. Undo.";
        assert_eq!(nth_sentence(text, 0), "Add a brick");
        assert_eq!(nth_sentence(text, 1), "Add a 3x12 brick");
        assert_eq!(nth_sentence(text, 9), text.trim());
    }
}

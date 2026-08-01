//! Optional JSONL trace of every `/build` thread message, enabled with
//! `--build-debug-log <path>`.
//!
//! Meant to be read back after a real session to see where `BrickTalk` falls
//! short: which known-verb sentences ended in a ❓, which sentences the
//! silence policy swallowed as chat, which questions came back empty. Off by
//! default — this is a development aid for iterating on the grammar, not a
//! data collection feature, and a write failure never interrupts building.

use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::dispatch::Outcome;

pub struct BuildDebugLog {
    file: Mutex<tokio::fs::File>,
}

/// The fields identifying one thread message, gathered in one place so
/// [`BuildDebugLog::record`] stays under clippy's argument-count lint.
pub struct MessageContext<'a> {
    pub session_id: &'a str,
    pub channel_id: u64,
    pub author_id: &'a str,
    pub locale: &'a str,
    pub text: &'a str,
}

impl BuildDebugLog {
    pub async fn open(path: &str) -> anyhow::Result<Self> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// Appends one line. `outcome` is `None` for the fast-path `Ignored`
    /// case, which never builds one.
    pub async fn record(
        &self,
        message: MessageContext<'_>,
        classification: Classification,
        outcome: Option<&Outcome>,
    ) {
        let entry = Entry {
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: message.session_id,
            channel_id: message.channel_id,
            author_id: message.author_id,
            locale: message.locale,
            message: message.text,
            classification: classification.as_str(),
            reactions: outcome.map_or_else(Vec::new, |o| o.reactions.clone()),
            replies: outcome.map_or_else(Vec::new, |o| o.replies.clone()),
            ops_applied: outcome.map_or(0, |o| o.ops.len()),
            pending_lookup: outcome.is_some_and(|o| o.pending_lookup.is_some()),
        };

        let Ok(mut line) = serde_json::to_string(&entry) else {
            return;
        };
        line.push('\n');

        let mut file = self.file.lock().await;
        if let Err(e) = file.write_all(line.as_bytes()).await {
            tracing::warn!("build debug log write failed: {e}");
        }
    }
}

/// How a message was resolved, coarse enough to `grep`/`jq` for the
/// interesting cases (`problem`, and `ignored` messages that look like they
/// meant something).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Filtered out before parsing reached the model: the silence policy read
    /// it as ordinary chat.
    Ignored,
    /// Parsed further but resolved to nothing worth reporting — typically an
    /// unanswerable question, which the silence policy also keeps quiet.
    Silent,
    /// Applied, at least in part.
    Ok,
    /// The message stopped on a parse or lowering error.
    Problem,
}

impl Classification {
    /// Reads `outcome` the same way the session layer already does, so the
    /// log agrees with what the thread actually saw.
    #[must_use]
    pub fn of(outcome: &Outcome) -> Self {
        if outcome
            .reactions
            .contains(&super::dispatch::reactions::PROBLEM)
        {
            Self::Problem
        } else {
            Self::Ok
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Silent => "silent",
            Self::Ok => "ok",
            Self::Problem => "problem",
        }
    }
}

#[derive(Serialize)]
struct Entry<'a> {
    ts: String,
    session_id: &'a str,
    channel_id: u64,
    author_id: &'a str,
    locale: &'a str,
    message: &'a str,
    classification: &'static str,
    reactions: Vec<&'static str>,
    replies: Vec<String>,
    ops_applied: usize,
    pending_lookup: bool,
}

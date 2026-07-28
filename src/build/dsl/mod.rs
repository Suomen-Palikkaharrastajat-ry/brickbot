//! The BrickTalk build language.
//!
//! A controlled natural language: it reads like English, is quick to type in
//! chat, and has deterministic semantics. Three pure stages:
//!
//! ```text
//! text --tokenize--> Vec<Sentence> --parse--> Vec<Statement> --lower--> Vec<Op>
//!       lexer.rs                    parser.rs                 lower.rs
//!      (no model)                  (no model)              (needs model)
//! ```
//!
//! Splitting parsing from lowering is what keeps the grammar testable as a
//! pure function of a string, and confines everything that needs to know about
//! the current model to one place.

#![allow(clippy::doc_markdown)]

pub mod ast;
pub mod lexer;
pub mod parser;

pub use ast::{Command, Meta, ObjectExpr, PartSpec, Query, Ref, Statement, Structure};
pub use lexer::{tokenize, Sentence, Token, Verb};
pub use parser::{parse_message, parse_sentence, ParseError, SentenceOutcome};

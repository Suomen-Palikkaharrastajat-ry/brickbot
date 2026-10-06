//! Parsing of queries, placement proposals and view requests.

use super::{
    Arrangement, CodeBlock, Direction, GridLayer, GridSpec, GridView, Keyword, LegendEntry, Parser,
    Query, Statement, Token, block, parse_part_spec,
};

/// Parses one sentence.
/// Reads a verbless sentence that names a part, e.g. `2 2x2 bricks stacked`.
///
/// Returns nothing unless the *whole* sentence is accounted for. A sentence we
/// only half understand has to stay silent rather than guess at it: this runs
/// on every message a build thread receives, and most of them are chat.
pub(super) fn parse_proposal(parser: &mut Parser) -> Option<Statement> {
    let spec = parser.part_spec();
    // The gate lets a bare count or colour through; only a size, a kind or a
    // part number actually names an element.
    if spec.size.is_none() && spec.kind.is_none() && spec.part_num.is_none() {
        return None;
    }

    // Whatever follows may only say how they sit relative to each other.
    let mut arrangement = None;
    while let Some(token) = parser.peek() {
        match token {
            Token::Word(word) => match Arrangement::parse(word) {
                Some(found) => arrangement = Some(found),
                // Filler people put around an arrangement: "on top of each
                // other", "side by side", "stacked together".
                None if matches!(
                    word.as_str(),
                    "the" | "and" | "together" | "each" | "other" | "another"
                ) => {}
                None => return None,
            },
            // "in a row", "side by side", "on top of each other".
            // ... and "a" in "in a row", which lexes as the number 1.
            Token::Keyword(Keyword::In | Keyword::By | Keyword::On | Keyword::Of)
            | Token::Number(1) => {}
            Token::Direction(direction) => {
                arrangement = Some(match direction {
                    Direction::Up | Direction::Down => Arrangement::Stacked,
                    Direction::North | Direction::South => Arrangement::RowSouth,
                    Direction::East | Direction::West => Arrangement::RowEast,
                });
            }
            _ => return None,
        }
        parser.pos += 1;
    }

    Some(Statement::Propose { spec, arrangement })
}

pub(super) fn parse_query(parser: &mut Parser<'_>) -> Option<Query> {
    let start = parser.pos;

    if parser.eat_word("how") {
        // "tall" and "high" lex as keywords, because they also end an extent
        // modifier ("6 bricks high").
        if parser.eat_keyword(Keyword::Tall) || parser.eat_keyword(Keyword::High) {
            parser.eat_word("is");
            let reference = parser.reference();
            return Some(Query::HowTall(reference));
        }
        if parser.eat_word("many") {
            let spec = parser.part_spec();
            return Some(Query::Count(Box::new(spec)));
        }
        parser.pos = start;
        return None;
    }

    // "where" and "who" need the rest of their shape before they count as a
    // question about the model. Without that, "who wants to help?" parses as
    // "who added <a thing called 'wants'>" and the thread gets an error about
    // a group nobody named.
    if parser.eat_word("where") {
        if !(parser.eat_word("is") || parser.eat_word("are")) {
            parser.pos = start;
            return None;
        }
        let Some(reference) = parser.reference() else {
            parser.pos = start;
            return None;
        };
        return Some(Query::WhereIs(reference));
    }

    if parser.eat_word("who") {
        if !(parser.eat_word("added")
            || parser.eat_word("built")
            || parser.eat_word("made")
            || parser.eat_word("placed"))
        {
            parser.pos = start;
            return None;
        }
        let Some(reference) = parser.reference() else {
            parser.pos = start;
            return None;
        };
        return Some(Query::WhoAdded(reference));
    }

    if parser.eat_word("what") {
        if parser.eat_word("changed") {
            return Some(Query::WhatChanged);
        }
        parser.pos = start;
        return None;
    }

    parser.pos = start;
    None
}

/// Parses `Show top view.` and its rearrangements.
///
/// Runs after the other `Show` forms, none of whose words name a view, so
/// there is nothing for it to shadow.
pub(super) fn parse_view_query(parser: &mut Parser<'_>) -> Option<Query> {
    let start = parser.pos;
    parser.skip_filler();

    // "Show top view." and "Show the view from the east." both occur; neither
    // reads naturally as the other.
    let view = if parser.eat_word("view") {
        parser.eat_keyword(Keyword::From);
        parser.skip_filler();
        parser.grid_view()
    } else {
        let view = parser.grid_view();
        if view.is_some() {
            let _ = parser.eat_word("view");
        }
        view
    };

    let Some(view) = view else {
        parser.pos = start;
        return None;
    };
    Some(Query::ShowView {
        view,
        layer: parser.layer_index(),
    })
}

/// Turns a captured block into a drawing.
///
/// The legend's right-hand sides go through the ordinary part grammar, so
/// `blue 1x1 brick`, `3 tan plates` and a bare part number all mean here what
/// they mean in `Add …`.
pub(super) fn grid_spec(view: GridView, block: &CodeBlock) -> GridSpec {
    let shaped = block::shape(block);
    GridSpec {
        view,
        legend: shaped
            .legend
            .into_iter()
            .map(|(symbol, text)| LegendEntry {
                symbol,
                spec: parse_part_spec(&text),
            })
            .collect(),
        layers: shaped
            .layers
            .into_iter()
            .map(|rows| GridLayer { rows })
            .collect(),
        bad_legend_key: shaped.bad_legend_key,
    }
}

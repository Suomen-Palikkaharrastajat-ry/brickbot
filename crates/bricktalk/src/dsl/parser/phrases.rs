//! Sub-phrases of a sentence: part descriptions, references, locations and modifiers.

use super::{
    Direction, ExtentAxis, IVec3, Keyword, Location, Modifier, Parser, PartKind, PartSpec, Place,
    Ref, Token, Unit,
};

impl Parser<'_> {
    /// Parses a part description: `[count] [colour] [size] [kind]`, in any
    /// sensible order, plus a bare part number.
    pub(super) fn part_spec(&mut self) -> PartSpec {
        let mut spec = PartSpec {
            quantity: 1,
            ..PartSpec::default()
        };
        let mut saw_something = false;

        loop {
            self.skip_filler_keeping_counts();
            match self.peek() {
                Some(Token::Number(n)) => {
                    // A number here is a count, unless a dimension follows in
                    // the shape "2 x 4".
                    let value = *n;
                    if matches!(self.peek_at(1), Some(Token::Word(w)) if w == "x")
                        && matches!(self.peek_at(2), Some(Token::Number(_)))
                    {
                        self.pos += 2;
                        if let Some(Token::Number(d)) = self.peek() {
                            spec.size = Some((clamp_dim(value), clamp_dim(*d)));
                            self.pos += 1;
                            saw_something = true;
                            continue;
                        }
                    }
                    // Four digits or more is a part number, not a count:
                    // nobody asks for two thousand bricks in one sentence,
                    // and LEGO design ids are all four digits or longer.
                    if value >= PART_NUMBER_THRESHOLD && spec.part_num.is_none() {
                        spec.part_num = Some(value.to_string());
                        self.pos += 1;
                        saw_something = true;
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    {
                        spec.quantity = value.clamp(1, 1000) as u32;
                    }
                    self.pos += 1;
                }
                Some(Token::Color(code)) => {
                    spec.color = Some(*code);
                    self.pos += 1;
                    saw_something = true;
                }
                Some(Token::Dim(w, d)) => {
                    spec.size = Some((*w, *d));
                    self.pos += 1;
                    saw_something = true;
                }
                Some(Token::Kind(kind)) => {
                    spec.kind = Some(*kind);
                    self.pos += 1;
                    saw_something = true;
                }
                // A bare alphanumeric word containing a digit is very likely a
                // part number, e.g. "3001" or "4073b".
                Some(Token::Word(w))
                    if spec.part_num.is_none()
                        && w.chars().any(|c| c.is_ascii_digit())
                        && w.chars().all(|c| c.is_ascii_alphanumeric()) =>
                {
                    spec.part_num = Some(w.clone());
                    self.pos += 1;
                    saw_something = true;
                }
                _ => break,
            }
        }

        if !saw_something && spec.quantity == 1 {
            return PartSpec {
                quantity: 1,
                ..PartSpec::default()
            };
        }
        spec
    }

    /// Like [`Self::skip_filler`] but leaves numbers alone, for use inside a
    /// part spec where a leading count is meaningful.
    pub(super) fn skip_filler_keeping_counts(&mut self) {
        while self.peek().and_then(Token::as_word) == Some("the") {
            self.pos += 1;
        }
    }

    /// Parses a reference to something already in the model.
    pub(super) fn reference(&mut self) -> Option<Ref> {
        self.skip_filler();
        match self.peek() {
            Some(Token::Keyword(Keyword::That | Keyword::It | Keyword::This)) => {
                self.pos += 1;
                Some(Ref::Last)
            }
            Some(Token::Keyword(Keyword::Them | Keyword::Those | Keyword::These)) => {
                self.pos += 1;
                Some(Ref::LastPlural)
            }
            Some(Token::Keyword(Keyword::Selection)) => {
                self.pos += 1;
                Some(Ref::Selection)
            }
            Some(Token::Keyword(Keyword::Everything)) => {
                self.pos += 1;
                Some(Ref::Everything)
            }
            Some(Token::Keyword(Keyword::All)) => {
                self.pos += 1;
                let spec = self.part_spec();
                if spec.is_empty() {
                    Some(Ref::Everything)
                } else {
                    Some(Ref::AllMatching(Box::new(spec)))
                }
            }
            Some(Token::Keyword(Keyword::Last | Keyword::Previous)) => {
                self.pos += 1;
                let spec = self.part_spec();
                Some(Ref::LastOfKind(Box::new(spec)))
            }
            Some(Token::Word(w)) if w == "highest" || w == "tallest" || w == "topmost" => {
                self.pos += 1;
                let _ = self.part_spec();
                Some(Ref::Highest)
            }
            Some(Token::Color(_) | Token::Dim(_, _) | Token::Kind(_)) => {
                let spec = self.part_spec();
                if spec.is_empty() {
                    None
                } else {
                    Some(Ref::LastOfKind(Box::new(spec)))
                }
            }
            // A bare word is a group or landmark name — unless it is an
            // adverb that belongs to a trailing modifier, which would
            // otherwise be swallowed here and never reach `modifiers()`.
            Some(Token::Word(name)) if !is_modifier_word(name) => {
                let name = name.clone();
                self.pos += 1;
                Some(Ref::Named(name))
            }
            Some(Token::Quoted(name)) => {
                let name = name.clone();
                self.pos += 1;
                Some(Ref::Named(name))
            }
            _ => None,
        }
    }

    /// Parses `at ...`, `on ...`, `under ...` or `<direction> of ...`.
    pub(super) fn location(&mut self) -> Option<Location> {
        let start = self.pos;
        if self.eat_keyword(Keyword::At) {
            if let Some(place) = self.place() {
                return Some(Location::At(place));
            }
            self.pos = start;
            return None;
        }

        if self.eat_keyword(Keyword::On) {
            // "on top of X" and "on X" mean the same thing.
            self.skip_filler();
            if self.eat_word("top") {
                self.eat_keyword(Keyword::Of);
            }
            if let Some(reference) = self.reference() {
                return Some(Location::On(reference));
            }
            self.pos = start;
            return None;
        }

        if self.eat_keyword(Keyword::Under) {
            if let Some(reference) = self.reference() {
                return Some(Location::Under(reference));
            }
            self.pos = start;
            return None;
        }

        if let Some(Token::Direction(direction)) = self.peek() {
            let direction = *direction;
            // Only a location when followed by "of" or "the ...".
            if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Of))) {
                self.pos += 2;
                if let Some(reference) = self.reference() {
                    return Some(Location::Direction {
                        direction,
                        of: reference,
                    });
                }
            }
            self.pos = start;
            return None;
        }

        None
    }

    pub(super) fn place(&mut self) -> Option<Place> {
        self.skip_filler();
        if self.eat_keyword(Keyword::Origin) {
            return Some(Place::Origin);
        }

        // Bare coordinates: "at 12 5 3".
        if let (Some(Token::Number(x)), Some(Token::Number(y)), Some(Token::Number(z))) =
            (self.peek(), self.peek_at(1), self.peek_at(2))
        {
            #[allow(clippy::cast_possible_truncation)]
            let place = Place::Coords(IVec3::new(*x as i32, *y as i32, *z as i32));
            self.pos += 3;
            return Some(place);
        }

        // "at x=12 y=5 z=3", where each pair lexes as one word.
        if let Some(coords) = self.axis_coords() {
            return Some(Place::Coords(coords));
        }

        match self.peek() {
            Some(Token::Word(name) | Token::Quoted(name)) => {
                let name = name.clone();
                self.pos += 1;
                Some(Place::Landmark(name))
            }
            _ => None,
        }
    }

    pub(super) fn axis_coords(&mut self) -> Option<IVec3> {
        let start = self.pos;
        let mut coords = IVec3::ZERO;
        let mut found = false;
        while let Some(Token::Word(w)) = self.peek() {
            let Some((axis, value)) = w.split_once('=') else {
                break;
            };
            let Ok(value) = value.parse::<i32>() else {
                break;
            };
            match axis {
                "x" => coords.x = value,
                "y" => coords.y = value,
                "z" => coords.z = value,
                _ => break,
            }
            found = true;
            self.pos += 1;
        }
        if found {
            Some(coords)
        } else {
            self.pos = start;
            None
        }
    }

    /// Parses every trailing modifier it can find.
    pub(super) fn modifiers(&mut self) -> Vec<Modifier> {
        let mut modifiers = Vec::new();
        let mut guard = 0;

        while !self.at_end() && guard < 64 {
            guard += 1;
            let before = self.pos;

            // "by one plate", "by two bricks"
            if self.eat_keyword(Keyword::By) {
                if let Some(m) = self.distance_or_extent(None) {
                    modifiers.push(m);
                    continue;
                }
            }

            // "upside down"
            if self.eat_keyword(Keyword::Upside) {
                self.eat_word("down");
                modifiers.push(Modifier::Flip);
                continue;
            }

            // "90 degrees", "3 times"
            if let Some(Token::Number(n)) = self.peek() {
                let value = *n;
                if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Degrees))) {
                    self.pos += 2;
                    // Parts only sit on quarter turns, so anything else was a
                    // misunderstanding. Rounding it to zero used to fall
                    // through to the bare-`Rotate` branch and quietly turn the
                    // part 90 degrees instead.
                    if value % 90 != 0 {
                        self.bad_angle = Some(value);
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    modifiers.push(Modifier::Turn((value / 90) as i32));
                    continue;
                }
                if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Times))) {
                    self.pos += 2;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    modifiers.push(Modifier::Quantity(value.clamp(1, 1000) as u32));
                    continue;
                }
                // "10 studs long", "6 bricks high"
                if let Some(m) = self.distance_or_extent(None) {
                    modifiers.push(m);
                    continue;
                }
            }

            // "clockwise" / "anticlockwise"
            if self.eat_word("clockwise") {
                modifiers.push(Modifier::Turn(1));
                continue;
            }
            if self.eat_word("anticlockwise") || self.eat_word("counterclockwise") {
                modifiers.push(Modifier::Turn(-1));
                continue;
            }

            // "across the centre", "about the north-south axis" — the axis a
            // mirror reflects in. Without this the direction is swallowed by
            // the skip below and every mirror silently used the default.
            if self.eat_any_keyword(&[Keyword::Across]).is_some() {
                self.skip_filler();
                // "across the centre" names the pivot, not an axis, and the
                // default axis is already the model's centre line.
                if self.eat_keyword(Keyword::Origin) {
                    continue;
                }
                if let Some(Token::Direction(direction)) = self.peek() {
                    let direction = *direction;
                    self.pos += 1;
                    let _ = self.eat_word("axis");
                    modifiers.push(Modifier::Distance {
                        direction,
                        count: 1,
                        unit: direction.default_unit(),
                    });
                }
                continue;
            }

            // "east 3 studs", "north"
            if let Some(Token::Direction(direction)) = self.peek() {
                let direction = *direction;
                self.pos += 1;
                if let Some(m) = self.distance_or_extent(Some(direction)) {
                    modifiers.push(m);
                } else {
                    modifiers.push(Modifier::Distance {
                        direction,
                        count: 1,
                        unit: direction.default_unit(),
                    });
                }
                continue;
            }

            // "using tan plates", "with white bricks"
            // The lexer folds "using" onto `With`.
            if self.eat_any_keyword(&[Keyword::With]).is_some() {
                let spec = self.part_spec();
                if !spec.is_empty() {
                    modifiers.push(Modifier::Using(Box::new(spec)));
                    continue;
                }
            }

            if self.pos == before {
                // Nothing matched; skip the token so we always terminate.
                self.pos += 1;
            }
        }

        modifiers
    }

    /// Parses `<count> <unit> [long|high|wide|deep]`, producing either a
    /// distance along `direction` or an extent.
    pub(super) fn distance_or_extent(&mut self, direction: Option<Direction>) -> Option<Modifier> {
        let start = self.pos;
        let Some(count) = self.number() else {
            self.pos = start;
            return None;
        };

        let unit = match self.peek() {
            Some(Token::Unit(u)) => {
                let u = *u;
                self.pos += 1;
                Some(u)
            }
            // "6 bricks high" lexes "bricks" as a part kind, not a unit.
            Some(Token::Kind(PartKind::Brick)) => {
                self.pos += 1;
                Some(Unit::Brick)
            }
            Some(Token::Kind(PartKind::Plate)) => {
                self.pos += 1;
                Some(Unit::Plate)
            }
            _ => None,
        };

        let axis = match self.peek() {
            Some(Token::Keyword(Keyword::Long)) => Some(ExtentAxis::Length),
            Some(Token::Keyword(Keyword::High | Keyword::Tall)) => Some(ExtentAxis::Height),
            Some(Token::Keyword(Keyword::Wide)) => Some(ExtentAxis::Width),
            Some(Token::Keyword(Keyword::Deep)) => Some(ExtentAxis::Depth),
            _ => None,
        };

        if let Some(axis) = axis {
            self.pos += 1;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return Some(Modifier::Extent {
                axis,
                count: count.clamp(1, 1000) as u32,
                unit: unit.unwrap_or(match axis {
                    ExtentAxis::Height => Unit::Brick,
                    _ => Unit::Stud,
                }),
            });
        }

        // A direction after the count also works: "3 studs east".
        let direction = direction.or_else(|| match self.peek() {
            Some(Token::Direction(d)) => {
                let d = *d;
                self.pos += 1;
                Some(d)
            }
            _ => None,
        });

        match direction {
            Some(direction) =>
            {
                #[allow(clippy::cast_possible_truncation)]
                Some(Modifier::Distance {
                    direction,
                    count: count as i32,
                    unit: unit.unwrap_or_else(|| direction.default_unit()),
                })
            }
            None if unit.is_some() => {
                // A bare "by one plate" with no direction means upwards, which
                // is what "raise"/"lower" supply from the verb.
                #[allow(clippy::cast_possible_truncation)]
                Some(Modifier::Distance {
                    direction: Direction::Up,
                    count: count as i32,
                    unit: unit.unwrap_or(Unit::Brick),
                })
            }
            None => {
                self.pos = start;
                None
            }
        }
    }
}

/// Parses a bare part description, with no verb in front of it.
///
/// The right-hand side of a legend line — `b = blue 1x1 brick` — describes a
/// part in exactly the words `Add a blue 1x1 brick.` uses, so it goes through
/// the same grammar rather than a second one that could drift from it.
///
/// The caller must have split the line on its `=` first: `split_words` keeps
/// `=` inside a word and `Parser::axis_coords` reads `x=12` as a coordinate,
/// so an unsplit legend line would parse as something else entirely.
#[must_use]
pub fn parse_part_spec(text: &str) -> PartSpec {
    let sentence = crate::dsl::lexer::tokenize_sentence(text);
    let mut parser = Parser::new(&sentence);
    parser.part_spec()
}

/// Numbers at or above this are read as part numbers rather than counts.
pub(super) const PART_NUMBER_THRESHOLD: i64 = 1000;

/// Words that only ever belong to a trailing modifier, so a reference must
/// never consume them as a name.
pub(super) fn is_modifier_word(word: &str) -> bool {
    matches!(
        word,
        "clockwise" | "anticlockwise" | "counterclockwise" | "down" | "around"
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) const fn clamp_dim(value: i64) -> u8 {
    if value < 1 {
        1
    } else if value > 255 {
        255
    } else {
        value as u8
    }
}

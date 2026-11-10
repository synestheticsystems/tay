//! Values as definitions write them: a number, true or false, or text that may
//! hold references. References are found here, on the parsed structure, and
//! resolved later by the planner; nothing is ever pasted into YAML.

pub mod reference;

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::source::{Loc, SourceId};
use crate::yaml::{Kind, Node, Scalar};

pub use reference::{EnvField, Part, Reference, Target, UnitRef, View};

/// The types a value can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Text,
    Number,
    Bool,
}

impl Type {
    pub fn describe(self) -> &'static str {
        match self {
            Type::Text => "text",
            Type::Number => "a whole number",
            Type::Bool => "true or false",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub loc: Loc,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Number(i64),
    Bool(bool),
    /// Text, perhaps with references inside it. It is text whatever the
    /// references are.
    Text(Vec<Piece>),
    /// A string that is one reference and nothing else. It keeps the type of
    /// what it refers to.
    Ref(Reference),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Lit(String),
    Ref(Reference),
}

impl Expr {
    pub fn text(loc: Loc, text: &str) -> Expr {
        Expr {
            loc,
            kind: ExprKind::Text(vec![Piece::Lit(text.to_string())]),
        }
    }

    /// Every reference in the value.
    pub fn references(&self) -> Vec<&Reference> {
        match &self.kind {
            ExprKind::Ref(reference) => vec![reference],
            ExprKind::Text(pieces) => pieces
                .iter()
                .filter_map(|piece| match piece {
                    Piece::Ref(reference) => Some(reference),
                    Piece::Lit(_) => None,
                })
                .collect(),
            ExprKind::Number(_) | ExprKind::Bool(_) => Vec::new(),
        }
    }

    /// The type of a value whose type does not wait on a reference.
    pub fn written_type(&self) -> Option<Type> {
        match &self.kind {
            ExprKind::Number(_) => Some(Type::Number),
            ExprKind::Bool(_) => Some(Type::Bool),
            ExprKind::Text(_) => Some(Type::Text),
            ExprKind::Ref(_) => None,
        }
    }

    /// The text of a value with no references in it.
    pub fn literal_text(&self) -> Option<String> {
        match &self.kind {
            ExprKind::Text(pieces) => pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Lit(text) => Some(text.as_str()),
                    Piece::Ref(_) => None,
                })
                .collect(),
            _ => None,
        }
    }
}

/// Reads a scalar in a place where a value goes. Anything but text, a whole
/// number or true/false is an error.
pub fn from_node(node: &Node, d: &mut Diagnostics) -> Option<Expr> {
    let kind = match &node.kind {
        Kind::Scalar(Scalar::Int(n)) => ExprKind::Number(*n),
        Kind::Scalar(Scalar::Bool(b)) => ExprKind::Bool(*b),
        Kind::Scalar(Scalar::Str(s)) => match parse_string(&s.text, node.loc, s.content_column) {
            Ok(kind) => kind,
            Err(error) => {
                d.push(error);
                return None;
            }
        },
        _ => {
            d.error(
                node.loc,
                format!(
                    "a value is text, a whole number or true or false; this is {}",
                    node.describe()
                ),
            );
            return None;
        }
    };
    Some(Expr {
        loc: node.loc,
        kind,
    })
}

/// Reads the references in a string that sits at `loc`. When the string's
/// characters are in the file as they are (`content_column`), each reference
/// is placed at its own column; otherwise at the string's start.
pub fn parse_string(
    text: &str,
    loc: Loc,
    content_column: Option<u32>,
) -> Result<ExprKind, Diagnostic> {
    let place = |offset: usize| match content_column {
        Some(column) => Loc { column, ..loc }.shifted(offset),
        None => loc,
    };
    let pieces = split(text, &place)?;
    Ok(match pieces.as_slice() {
        [Piece::Ref(reference)] => ExprKind::Ref(reference.clone()),
        _ => ExprKind::Text(pieces),
    })
}

/// Reads the references in the whole text of a file, placing each at its own
/// line and column.
pub fn parse_document(text: &str, source: SourceId) -> Result<Vec<Piece>, Diagnostic> {
    let mut starts = Vec::new();
    let (mut line, mut column) = (1u32, 1u32);
    for c in text.chars() {
        starts.push((line, column));
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    starts.push((line, column));
    split(text, &|offset| {
        let (line, column) = starts[offset];
        Loc::new(source, line, column)
    })
}

/// Splits text into literal pieces and references. `$${` writes a literal `${`.
fn split(text: &str, place: &dyn Fn(usize) -> Loc) -> Result<Vec<Piece>, Diagnostic> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&['$', '$', '{']) {
            literal.push_str("${");
            i += 3;
        } else if chars[i..].starts_with(&['$', '{']) {
            let Some(close) = chars[i + 2..].iter().position(|c| *c == '}') else {
                return Err(Diagnostic::new(
                    place(i),
                    "`${` is not closed with `}` (write `$${` for a literal `${`)",
                ));
            };
            let inner: String = chars[i + 2..i + 2 + close].iter().collect();
            let target = reference::parse_target(&inner)
                .map_err(|message| Diagnostic::new(place(i), message))?;
            if !literal.is_empty() {
                pieces.push(Piece::Lit(std::mem::take(&mut literal)));
            }
            pieces.push(Piece::Ref(Reference {
                loc: place(i),
                inner,
                target,
            }));
            i += close + 3;
        } else {
            literal.push(chars[i]);
            i += 1;
        }
    }
    if !literal.is_empty() || pieces.is_empty() {
        pieces.push(Piece::Lit(literal));
    }
    Ok(pieces)
}

/// Writes pieces back as definition text, escaping literal `${`. A literal `$`
/// right before a reference cannot be written (`$${` would read as the
/// escape), so that gives `None`.
pub fn write_pieces(pieces: &[Piece]) -> Option<String> {
    let mut merged: Vec<Piece> = Vec::new();
    for piece in pieces {
        match (merged.last_mut(), piece) {
            (Some(Piece::Lit(previous)), Piece::Lit(text)) => previous.push_str(text),
            _ => merged.push(piece.clone()),
        }
    }
    let mut out = String::new();
    for (i, piece) in merged.iter().enumerate() {
        match piece {
            Piece::Lit(text) => {
                if text.ends_with('$') && matches!(merged.get(i + 1), Some(Piece::Ref(_))) {
                    return None;
                }
                out.push_str(&text.replace("${", "$${"));
            }
            Piece::Ref(reference) => out.push_str(&reference.written()),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Sources;

    fn loc() -> Loc {
        let mut sources = Sources::default();
        Loc::new(sources.add_argument("test"), 3, 7)
    }

    fn kind(text: &str) -> ExprKind {
        parse_string(text, loc(), Some(10)).unwrap()
    }

    #[test]
    fn a_whole_string_reference_keeps_its_type_and_text_around_one_is_text() {
        assert!(matches!(kind("${web-port.number}"), ExprKind::Ref(_)));
        let ExprKind::Text(pieces) = kind("port ${web-port.number}") else {
            panic!("expected text");
        };
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0], Piece::Lit("port ".into()));
    }

    #[test]
    fn a_doubled_dollar_writes_a_literal_reference_opening() {
        assert_eq!(
            kind("cost $${amount} and $5"),
            ExprKind::Text(vec![Piece::Lit("cost ${amount} and $5".into())])
        );
    }

    #[test]
    fn each_reference_is_placed_at_its_column() {
        let ExprKind::Text(pieces) = kind("ab${x.y}") else {
            panic!("expected text");
        };
        let Piece::Ref(reference) = &pieces[1] else {
            panic!("expected a reference");
        };
        assert_eq!((reference.loc.line, reference.loc.column), (3, 12));
    }

    #[test]
    fn an_unclosed_or_malformed_reference_is_an_error_at_its_column() {
        let error = parse_string("x ${a.b", loc(), Some(10)).unwrap_err();
        assert_eq!(error.loc.column, 12);
        let error = parse_string("${}", loc(), None).unwrap_err();
        assert_eq!(error.loc.column, 7);
    }

    #[test]
    fn a_document_places_references_by_line_and_column() {
        let mut sources = Sources::default();
        let id = sources.add_argument("doc");
        let pieces = parse_document("a\n  b ${x.y}\n", id).unwrap();
        let Piece::Ref(reference) = &pieces[1] else {
            panic!("expected a reference");
        };
        assert_eq!((reference.loc.line, reference.loc.column), (2, 5));
    }

    #[test]
    fn pieces_are_written_back_escaped() {
        let ExprKind::Text(pieces) = kind("a ${x.y}") else {
            panic!("expected text");
        };
        let mut spliced = vec![Piece::Lit("$".into()), Piece::Lit("{z}".into())];
        spliced.extend(pieces);
        assert_eq!(write_pieces(&spliced).as_deref(), Some("$${z}a ${x.y}"));
        let ExprKind::Ref(reference) = kind("${x.y}") else {
            panic!("expected a reference");
        };
        assert_eq!(
            write_pieces(&[Piece::Lit("$".into()), Piece::Ref(reference)]),
            None
        );
    }
}

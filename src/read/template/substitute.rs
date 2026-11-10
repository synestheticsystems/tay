//! Putting a unit's parameters in place in its template's unit, on the
//! parsed structure.

use std::collections::HashMap;

use crate::diagnostic::Diagnostics;
use crate::expr::{self, ExprKind, Piece, Target};
use crate::yaml::{Entry, Kind, Node, Scalar, Str};

/// The template's unit with each `${params.x}` replaced. A string that is the
/// reference alone becomes the parameter's value, keeping its type; a
/// reference inside text is replaced inside the text.
pub fn substitute(
    node: &Node,
    values: &HashMap<String, Node>,
    d: &mut Diagnostics,
) -> Option<Node> {
    let kind = match &node.kind {
        Kind::Map(entries) => Kind::Map(
            entries
                .iter()
                .map(|entry| {
                    Some(Entry {
                        key: entry.key.clone(),
                        value: substitute(&entry.value, values, d)?,
                    })
                })
                .collect::<Option<_>>()?,
        ),
        Kind::Seq(items) => Kind::Seq(
            items
                .iter()
                .map(|item| substitute(item, values, d))
                .collect::<Option<_>>()?,
        ),
        Kind::Scalar(Scalar::Str(s)) => return substitute_text(node, s, values, d),
        Kind::Scalar(_) => node.kind.clone(),
    };
    Some(Node {
        loc: node.loc,
        kind,
    })
}

fn substitute_text(
    node: &Node,
    s: &Str,
    values: &HashMap<String, Node>,
    d: &mut Diagnostics,
) -> Option<Node> {
    let param = |piece: &Piece| match piece {
        Piece::Ref(reference) => match &reference.target {
            Target::Param(name) => values.get(name),
            _ => None,
        },
        Piece::Lit(_) => None,
    };
    let pieces = match expr::parse_string(&s.text, node.loc, s.content_column) {
        Ok(ExprKind::Ref(reference)) => {
            let whole = Piece::Ref(reference);
            return Some(param(&whole).cloned().unwrap_or_else(|| node.clone()));
        }
        Ok(ExprKind::Text(pieces)) if pieces.iter().any(|p| param(p).is_some()) => pieces,
        _ => return Some(node.clone()),
    };
    let mut filled = Vec::new();
    for piece in &pieces {
        let Some(value) = param(piece) else {
            filled.push(piece.clone());
            continue;
        };
        match &value.kind {
            Kind::Scalar(Scalar::Str(v)) => {
                match expr::parse_string(&v.text, value.loc, v.content_column) {
                    Ok(ExprKind::Text(inner)) => filled.extend(inner),
                    Ok(ExprKind::Ref(reference)) => filled.push(Piece::Ref(reference)),
                    Ok(_) => unreachable!("a string parses as text or a reference"),
                    Err(error) => {
                        d.push(error);
                        return None;
                    }
                }
            }
            Kind::Scalar(Scalar::Int(n)) => filled.push(Piece::Lit(n.to_string())),
            Kind::Scalar(Scalar::Bool(b)) => filled.push(Piece::Lit(b.to_string())),
            _ => return None,
        }
    }
    let Some(text) = expr::write_pieces(&filled) else {
        d.error(
            node.loc,
            "with the parameters put in, this text has a `$` right before a reference, which cannot be written (`$${` writes a literal `${`)",
        );
        return None;
    };
    Some(Node {
        loc: node.loc,
        kind: Kind::Scalar(Scalar::Str(Str {
            text,
            content_column: None,
        })),
    })
}

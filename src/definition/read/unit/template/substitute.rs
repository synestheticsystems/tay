//! Putting a unit's parameters in place in its template's unit, on the
//! parsed structure.

use std::collections::HashMap;

use crate::diagnostic::Diagnostics;
use crate::syntax::expr::{self, ExprKind, Piece, Reference, Target};
use crate::syntax::yaml::{Entry, Kind, Node, Placement, Scalar, Str};

/// The template's unit with each `${params.x}` replaced. A string that is the
/// reference alone becomes the parameter's value, whatever YAML it is; a
/// reference inside text is replaced inside the text, and its value must
/// then be text, a whole number or true or false.
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
    let param = |reference: &Reference| match &reference.target {
        Target::Param(name) => values.get(name),
        _ => None,
    };
    let pieces = match expr::parse_string(&s.text, node.loc, &s.placement) {
        Ok(ExprKind::Ref(reference)) => {
            return Some(param(&reference).cloned().unwrap_or_else(|| node.clone()));
        }
        Ok(ExprKind::Text(pieces))
            if pieces
                .iter()
                .any(|p| matches!(p, Piece::Ref(r) if param(r).is_some())) =>
        {
            pieces
        }
        _ => return Some(node.clone()),
    };
    let mut filled = Vec::new();
    // The text stands where what it begins with was written, in the
    // template or in a parameter's value, since a path that begins `./` is
    // relative to that file's folder.
    let mut begins_at = node.loc;
    for (index, piece) in pieces.iter().enumerate() {
        let named = match piece {
            Piece::Ref(reference) => param(reference).map(|value| (reference, value)),
            Piece::Lit(_) => None,
        };
        let Some((reference, value)) = named else {
            filled.push(piece.clone());
            continue;
        };
        if index == 0 {
            begins_at = value.loc;
        }
        match &value.kind {
            Kind::Scalar(Scalar::Str(v)) => {
                match expr::parse_pieces(&v.text, value.loc, &v.placement) {
                    Ok(inner) => filled.extend(inner),
                    Err(error) => {
                        d.push(error);
                        return None;
                    }
                }
            }
            Kind::Scalar(Scalar::Int(n)) => filled.push(Piece::Lit(n.to_string())),
            Kind::Scalar(Scalar::Bool(b)) => filled.push(Piece::Lit(b.to_string())),
            _ => {
                d.error(
                    reference.loc,
                    format!(
                        "`{}` is inside text, so its value must be text, a whole number or true or false; this is {}",
                        reference.written(),
                        value.describe()
                    ),
                );
                return None;
            }
        }
    }
    let Some(text) = expr::write_pieces(&filled) else {
        d.error(
            node.loc,
            "with the parameters put in, this text has a `$` right before a reference, which cannot be written (`$${` writes a literal `${`)",
        );
        return None;
    };
    // Each reference keeps where it was written: in the template, or in the
    // parameter's value in the unit.
    let written = filled
        .iter()
        .filter_map(|piece| match piece {
            Piece::Ref(reference) => Some(reference.loc),
            Piece::Lit(_) => None,
        })
        .collect();
    Some(Node {
        loc: begins_at,
        kind: Kind::Scalar(Scalar::Str(Str {
            text,
            placement: Placement::References(written),
        })),
    })
}

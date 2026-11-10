//! Settings: what the stack declares, and the values places give them.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::{Literal, Type};
use crate::syntax::names::NameAt;

#[derive(Clone, Debug)]
pub struct SettingDecl {
    pub name: NameAt,
    pub default: Option<Written>,
    pub ty: Type,
    pub secret: bool,
}

/// A literal and where it is written.
#[derive(Clone, Debug)]
pub struct Written {
    pub value: Literal,
    pub loc: Loc,
}

/// A setting's value as one place gives it.
#[derive(Clone, Debug)]
pub enum SettingValue {
    /// Written in a file, where YAML gives it its type.
    Written(Written),
    /// The text of a `--set`, which takes the setting's type.
    Argument { text: String, loc: Loc },
}

impl SettingValue {
    pub fn loc(&self) -> Loc {
        match self {
            SettingValue::Written(written) => written.loc,
            SettingValue::Argument { loc, .. } => *loc,
        }
    }

    /// Whether the value is empty text.
    pub fn is_empty(&self) -> bool {
        match self {
            SettingValue::Written(written) => written.value == Literal::Text(String::new()),
            SettingValue::Argument { text, .. } => text.is_empty(),
        }
    }

    /// The value as a setting of type `ty` holds it; `None` when a `--set`
    /// cannot be read as that type, or a written value has another type.
    pub fn typed(&self, ty: Type) -> Option<Literal> {
        match self {
            SettingValue::Written(written) => {
                (written.value.ty() == ty).then(|| written.value.clone())
            }
            SettingValue::Argument { text, .. } => match ty {
                Type::Text => Some(Literal::Text(text.clone())),
                Type::Number => text.parse().ok().map(Literal::Number),
                Type::Bool => match text.as_str() {
                    "true" => Some(Literal::Bool(true)),
                    "false" => Some(Literal::Bool(false)),
                    _ => None,
                },
            },
        }
    }
}

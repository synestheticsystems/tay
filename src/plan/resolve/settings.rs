//! Settings: the value in force, typed as declared, and kept secret when
//! declared so.

use crate::definition::SettingValue;
use crate::diagnostic::Diagnostics;
use crate::expr::Type;
use crate::plan::value::{Hole, Resolved, Shape};
use crate::source::Loc;

use super::{Asker, Fail, Key, Res, Resolver, Scope};

impl Resolver<'_> {
    pub(super) fn setting(&mut self, name: &str, at: Loc, asker: Asker) -> Res {
        let Some(decl) = self.stack.setting(name) else {
            return self.fail(at, format!("no setting `{name}` is declared in tay.yaml"));
        };
        let value = match (self.layered.settings.get(name), &decl.default) {
            (Some(value), _) => value.clone(),
            (None, Some(default)) => SettingValue::Written(default.clone()),
            (None, None) => {
                self.unset.entry(name.to_string()).or_default().push(at);
                return Err(Fail::Reported);
            }
        };
        let scope = Scope { unit: None, asker };
        let resolved = self.keyed(Key::Setting(name.to_string()), None, at, asker, |this| {
            this.resolve(value.expr(), scope)
        })?;
        let ty = match decl.ty {
            Some(ty) => Some(ty),
            None => match &decl.default {
                Some(default) => self.resolve(default, scope)?.ty(),
                None => None,
            },
        };
        let mut resolved = match (&value, ty) {
            (SettingValue::Argument(expr), Some(ty)) => {
                argument(resolved, ty, name, expr.loc, &mut self.d)?
            }
            _ => resolved,
        };
        if let (Some(ty), Some(got)) = (ty, resolved.ty())
            && ty != got
        {
            return self.fail(
                value.expr().loc,
                format!(
                    "setting `{name}` is {}; this value is {}",
                    ty.describe(),
                    got.describe()
                ),
            );
        }
        if decl.secret {
            resolved.shape = Shape::Opaque {
                ty: resolved.ty(),
                hole: Hole::Secret,
            };
            resolved.secrets.insert(format!("settings.{name}"));
        }
        Ok(resolved)
    }
}

/// The text of a `--set`, typed as its setting is.
fn argument(resolved: Resolved, ty: Type, name: &str, loc: Loc, d: &mut Diagnostics) -> Res {
    let Some(text) = resolved
        .exact_text()
        .filter(|_| resolved.producers.is_empty())
    else {
        return Ok(resolved);
    };
    let shape = match ty {
        Type::Text => return Ok(resolved),
        Type::Number => text.parse().ok().map(Shape::Number),
        Type::Bool => match text.as_str() {
            "true" => Some(Shape::Bool(true)),
            "false" => Some(Shape::Bool(false)),
            _ => None,
        },
    };
    match shape {
        Some(shape) => Ok(Resolved { shape, ..resolved }),
        // The message does not quote the value: it may be a secret.
        None => {
            d.error(
                loc,
                format!(
                    "setting `{name}` is {}, and this value is not",
                    ty.describe()
                ),
            );
            Err(Fail::Reported)
        }
    }
}

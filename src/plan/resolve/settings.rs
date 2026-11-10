//! Settings: the value in force, typed as declared, and kept secret when
//! declared so. A setting's value is literal, so it is built from nothing.

use crate::definition::{Literal, SettingValue};
use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::plan::resolve::value::{Hole, Resolved, Shape};

use super::{Fail, Res, Resolver};

impl Resolver<'_> {
    /// One error for each setting with no value and no default that the plan
    /// uses, at its declaration and naming every use.
    pub fn report_unset_settings(&mut self) {
        let stack = self.stack;
        for (name, mut uses) in std::mem::take(&mut self.unset) {
            let Some(decl) = stack.setting(&name) else {
                continue;
            };
            uses.sort();
            uses.dedup();
            let mut error = Diagnostic::new(
                decl.name.loc,
                format!("setting `{name}` has no value and no default, and the plan uses it"),
            );
            for loc in uses {
                error = error.with(loc, "it is used here");
            }
            self.d.push(error);
        }
    }

    pub(super) fn setting(&mut self, name: &str, at: Loc) -> Res {
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
        // A value of the wrong type was reported where it is written.
        let literal = value.typed(decl.ty).ok_or(Fail::Reported)?;
        let mut resolved = match literal {
            Literal::Text(text) => Resolved::text(text),
            Literal::Number(n) => Resolved::known(Shape::Number(n)),
            Literal::Bool(b) => Resolved::known(Shape::Bool(b)),
        };
        resolved.begins_with_setting = true;
        if decl.secret {
            resolved.shape = Shape::Opaque {
                ty: Some(decl.ty),
                hole: Hole::Secret,
            };
            resolved.secrets.insert(format!("settings.{name}"));
        }
        Ok(resolved)
    }
}

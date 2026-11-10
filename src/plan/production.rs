//! What a production profile refuses, checked after every choice is applied,
//! so that switching a unit with `--set` cannot slip past it.

use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::module::{Concern, Runs, UnitKind};
use crate::syntax::expr::Expr;
use crate::syntax::names::NameAt;

use super::Planner;
use super::reach::Reached;
use super::resolve::paths::normalized;
use super::resolve::{Asker, Scope};

impl Planner<'_> {
    pub fn refuse_for_production(&mut self, profile: &NameAt, marked_at: Loc, reached: &Reached) {
        let stack = self.resolver.stack;
        for (index, visit) in &reached.visits {
            let unit = &stack.units[*index];
            let way = &unit.ways[visit.way];
            let Some(module) = &way.module else {
                continue;
            };
            let mut refusals: Vec<(Loc, String)> = Vec::new();
            if matches!(module.shape.kind, UnitKind::Runs(Runs::OnHost)) {
                refusals.push((module.uses, "it runs on the host".to_string()));
            }
            for concern in &module.shape.production {
                match concern {
                    Concern::BuildsImage(loc) => {
                        refusals.push((*loc, "it builds an image".to_string()))
                    }
                    Concern::MountsHostPath(from) => {
                        let asker = Asker::of(&module.shape.kind);
                        if let Some(why) = self.mount_outside(*index, from, asker) {
                            refusals.push((from.loc, why));
                        }
                    }
                }
            }
            if let Some(first) = way.watch.first() {
                refusals.push((first.loc, "it watches files or folders".to_string()));
            }
            for (loc, why) in refusals {
                let mut error = Diagnostic::new(
                    loc,
                    format!(
                        "the production profile `{}` refuses unit `{}`: {why}",
                        profile.name, unit.name.name
                    ),
                )
                .with(marked_at, "the profile is marked production here");
                if let (Some(at), Some(name)) = (visit.chosen_at, &way.name) {
                    error = error.with(at, format!("way `{}` is chosen here", name.name));
                }
                self.resolver.d.push(error);
            }
        }
    }

    /// Why a mount's source is refused, if it is: it is refused when it is
    /// neither under the stack's root (which holds the state folder) nor in
    /// the shared folder, and does not begin with a setting's value, which
    /// is where a production environment says what of its machine to mount.
    fn mount_outside(&mut self, unit: usize, from: &Expr, asker: Asker) -> Option<String> {
        let scope = Scope {
            unit: Some(unit),
            asker,
        };
        // The source is also one of the unit's slots, so a failure to resolve
        // it has been reported where the unit's values were used.
        let value = self.resolver.resolve(from, scope).ok()?;
        if value.begins_with_setting {
            return None;
        }
        // Where a source leads is known from its start: what follows a part
        // known before start stays under the folder that part names.
        let value = self.resolver.host_path(value, from.loc);
        let leading = value.leading_text();
        if leading.is_empty() {
            return Some("it mounts a source that is not known before start".to_string());
        }
        let path = Path::new(&leading);
        let inside =
            normalized(path).starts_with(&self.root) || self.resolver.in_shared_folder(path);
        (!inside).then(|| {
            format!(
                "it mounts `{}`, which is neither under the stack's root nor in the shared folder, and does not begin with a setting's value",
                value.shown()
            )
        })
    }
}

//! What a production profile refuses, checked after every choice is applied,
//! so that switching a unit with `--set` cannot slip past it.

use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::module::{Concern, Runs, UnitKind};
use crate::syntax::expr::Expr;
use crate::syntax::names::NameAt;
use crate::value::Hole;

use super::Planner;
use super::reach::Reached;
use super::resolve::paths::normalized;
use super::resolve::{Asker, Chosen, Scope};

impl Planner<'_> {
    pub fn refuse_for_production(
        &mut self,
        profile: &NameAt,
        marked_at: Loc,
        reached: &Reached<'_>,
    ) {
        let stack = self.resolver.stack;
        for (index, visit) in &reached.visits {
            let unit = &stack.units[*index];
            let Chosen::Mechanism { mechanism, way } = visit.chosen else {
                continue;
            };
            let mut refusals: Vec<(Loc, String)> = Vec::new();
            if matches!(mechanism.description.kind, UnitKind::Runs(Runs::OnHost)) {
                refusals.push((mechanism.uses, "it runs on the host".to_string()));
            }
            for concern in &mechanism.description.production {
                match concern {
                    Concern::BuildsImage(loc) => {
                        refusals.push((*loc, "it builds an image".to_string()))
                    }
                    Concern::MountsHostPath(from) => {
                        let asker = Asker::of(&mechanism.description.kind);
                        if let Some(why) = self.mount_outside(*index, from, asker) {
                            refusals.push((from.loc, why));
                        }
                    }
                }
            }
            if let Some(first) = mechanism.watch.first() {
                refusals.push((first.expr.loc, "it watches files or folders".to_string()));
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
                if let Some((name, at)) = way {
                    error = error.with(at, format!("way `{}` is chosen here", name.name));
                }
                self.resolver.d.push(error);
            }
        }
    }

    /// Why a mount's source is refused, if it is: it is refused when it
    /// continues with a value produced by running or an action's argument,
    /// or when it is neither
    /// under the stack's root (which holds the state folder) nor in the
    /// shared folder and does not begin with a setting's value, which is
    /// where a production environment says what of its machine to mount.
    fn mount_outside(&mut self, unit: usize, from: &Expr, asker: Asker) -> Option<String> {
        let scope = Scope {
            unit: Some(unit),
            asker,
        };
        // The source is also one of the unit's slots, so a failure to resolve
        // it has been reported where the unit's values were used.
        let value = self.resolver.resolve(from, scope).ok()?;
        // What fills these holes can be any text, `..` included.
        for hole in value.value.holes() {
            match hole {
                Hole::Produced { .. } => return Some(
                    "it mounts a source that continues with a value produced by running, so where it leads is not known before anything starts"
                        .to_string(),
                ),
                Hole::Arg { .. } => return Some(
                    "it mounts a source that continues with an action's argument, so where it leads is not known until the action is asked for"
                        .to_string(),
                ),
                _ => {}
            }
        }
        if value.begins_with_setting {
            return None;
        }
        // Where the source leads is judged by the text it begins with, every
        // part known before start put in and `..` worked out, up to the first
        // part that is not.
        let value = self.resolver.host_path(value);
        let leading = value.value.leading_text();
        if leading.is_empty() {
            return Some("it mounts a source that is not known before start".to_string());
        }
        let path = Path::new(&leading);
        let inside =
            normalized(path).starts_with(&self.root) || self.resolver.in_shared_folder(path);
        (!inside).then(|| {
            format!(
                "it mounts `{}`, which is neither under the stack's root nor in the shared folder, and does not begin with a setting's value",
                value.value.shown()
            )
        })
    }
}

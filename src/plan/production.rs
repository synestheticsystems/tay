//! What a production profile refuses, checked after every choice is applied,
//! so that switching a unit with `--set` cannot slip past it.

use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::module::{Concern, Runs, UnitKind};
use crate::syntax::expr::{Expr, ExprKind, Piece, Target};
use crate::syntax::names::NameAt;

use super::Planner;
use super::resolve::paths::normalized;
use super::resolve::{Asker, Scope};

/// A unit in the plan, with the way it runs and where that was chosen.
pub struct Chosen {
    pub unit: usize,
    pub way: usize,
    pub chosen_at: Option<Loc>,
}

impl Planner<'_> {
    pub fn refuse_for_production(&mut self, profile: &NameAt, marked_at: Loc, units: &[Chosen]) {
        let stack = self.resolver.stack;
        for chosen in units {
            let unit = &stack.units[chosen.unit];
            let way = &unit.ways[chosen.way];
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
                        if let Some(why) = self.mount_outside(chosen.unit, from, asker) {
                            refusals.push((from.loc, why));
                        }
                    }
                }
            }
            if let Some(first) = way.watch.first() {
                refusals.push((first.loc, "it watches folders".to_string()));
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
                if let (Some(at), Some(name)) = (chosen.chosen_at, &way.name) {
                    error = error.with(at, format!("way `{}` is chosen here", name.name));
                }
                self.resolver.d.push(error);
            }
        }
    }

    /// Why a mount's source is refused, if it is: it is refused when it is
    /// outside the stack's own folders and not named by a setting.
    fn mount_outside(&mut self, unit: usize, from: &Expr, asker: Asker) -> Option<String> {
        let named_by_setting = match &from.kind {
            ExprKind::Ref(reference) => matches!(reference.target, Target::Setting(_)),
            ExprKind::Text(pieces) => matches!(
                pieces.first(),
                Some(Piece::Ref(reference)) if matches!(reference.target, Target::Setting(_))
            ),
            ExprKind::Number(_) | ExprKind::Bool(_) => false,
        };
        if named_by_setting {
            return None;
        }
        let scope = Scope {
            unit: Some(unit),
            asker,
        };
        // The source is also one of the unit's slots, so a failure to resolve
        // it has been reported where the unit's values were used.
        let value = self.resolver.resolve(from, scope).ok()?;
        let value = self.resolver.host_path(value, from.loc);
        let Some(text) = value.exact_text() else {
            return Some("it mounts a source that is not known before start".to_string());
        };
        let path = normalized(Path::new(&text));
        (!path.starts_with(&self.root)).then(|| {
            format!("it mounts `{text}`, which is outside the stack's own folders and not named by a setting")
        })
    }
}

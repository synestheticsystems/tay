//! Visiting one unit that comes up: every value it uses resolved where it runs,
//! what it needs and why, how it is known to be ready, and what it gives.

use crate::definition::{Check, Variant};
use crate::diagnostic::source::Loc;
use crate::module::{Place, RestartDefault, Runs, Want};

use super::usage::{ProducedUse, User};
use crate::plan::Planner;
use crate::plan::model::{Because, PlannedUnit, Readiness, RestartPolicy, Running, Shown, Way};
use crate::plan::resolve::Asker;
use crate::plan::resolve::value::Resolved;

const READINESS: Place = Place::Readable("a readiness check");
const WATCHED: Place = Place::Readable("a watched folder");

/// A unit `needs` another: as written, or because it uses a value the other
/// produces by running.
pub struct NeedEdge {
    pub unit: usize,
    pub loc: Loc,
    pub because: Because,
}

/// What visiting one unit adds to the plan.
pub struct Visit {
    /// `None` when one of its values could not be resolved; that is reported.
    pub planned: Option<PlannedUnit>,
    pub needs: Vec<NeedEdge>,
    pub after: Vec<(usize, Loc)>,
}

impl Planner<'_> {
    pub fn visit(&mut self, unit: usize, variant: usize) -> Visit {
        let stack = self.resolver.stack;
        let definition = &stack.units[unit];
        let variant = &definition.variants[variant];
        let mut produced = Vec::new();
        let (way, ready, restart, watch, gives) = match &variant.module {
            None => (
                Way::Group,
                Some(Readiness::Group),
                Some(None),
                Vec::new(),
                Vec::new(),
            ),
            Some(module) => {
                let runs = &module.shape.runs;
                let user = User {
                    unit,
                    name: &definition.name.name,
                    asker: Asker::of(runs),
                    readiness: false,
                };
                for slot in &module.shape.slots {
                    self.use_value(user, &slot.expr, slot.want, slot.place, &mut produced);
                }
                let running = self.running(runs);
                let ready = self.readiness(
                    user,
                    variant,
                    module.shape.ready_without_check,
                    &mut produced,
                );
                let restart = self.restart(
                    user,
                    variant,
                    module.shape.restart_by_default,
                    &mut produced,
                );
                let watch = self.watch(user, variant, &mut produced);
                let gives = self.gives(unit, variant);
                let way = Way::Module {
                    module: module.module.name(),
                    alternative: variant.alternative.as_ref().map(|alt| alt.name.clone()),
                    runs: running,
                };
                (way, ready, restart.map(Some), watch, gives)
            }
        };
        let needs = self.needs(variant, produced);
        let after = variant
            .after
            .iter()
            .filter_map(|name| Some((self.resolver.unit_index(&name.name)?, name.loc)))
            .collect();
        let planned = (|| {
            Some(PlannedUnit {
                name: definition.name.name.clone(),
                doc: definition.doc.clone(),
                way,
                needs: Vec::new(),
                after: Vec::new(),
                ready: ready?,
                restart: restart?,
                watch: watch.into_iter().collect::<Option<_>>()?,
                gives: gives.into_iter().collect::<Option<_>>()?,
            })
        })();
        Visit {
            planned,
            needs,
            after,
        }
    }

    fn running(&mut self, runs: &Runs) -> Running {
        match runs {
            Runs::OnHost => Running::OnHost,
            Runs::OwnContainer { .. } => Running::OwnContainer,
            Runs::OneOffContainer => Running::OneOffContainer,
            Runs::ByTay => Running::ByTay,
            Runs::InsideUnit(inner) => {
                if let Some(index) = self.resolver.unit_index(&inner.name)
                    && !matches!(self.resolver.runs(index), Some(Runs::OwnContainer { .. }))
                {
                    self.resolver.d.error(
                        inner.loc,
                        format!(
                            "`in` names unit `{}`, which does not run in a container of its own",
                            inner.name
                        ),
                    );
                }
                Running::InsideUnit(inner.name.clone())
            }
        }
    }

    fn readiness(
        &mut self,
        user: User<'_>,
        variant: &Variant,
        without_check: &'static str,
        produced: &mut Vec<ProducedUse>,
    ) -> Option<Readiness> {
        let Some(ready) = &variant.ready else {
            return Some(Readiness::Default(without_check));
        };
        // tay makes `http` and `tcp` checks itself, from the host.
        let from_host = User {
            asker: Asker::Host,
            readiness: true,
            ..user
        };
        let (check, target): (&'static str, Vec<Option<Resolved>>) = match &ready.check {
            Check::Http(url) => (
                "http",
                vec![self.use_value(from_host, url, Want::Text, READINESS, produced)],
            ),
            Check::Tcp(address) => (
                "tcp",
                vec![self.use_value(from_host, address, Want::Text, READINESS, produced)],
            ),
            Check::Log(line) => (
                "log",
                vec![self.use_value(user, line, Want::Text, READINESS, produced)],
            ),
            Check::Command(command) => (
                "command",
                command
                    .iter()
                    .map(|word| self.use_value(user, word, Want::Scalar, READINESS, produced))
                    .collect(),
            ),
        };
        let within = self.use_value(user, &ready.within, Want::Duration, READINESS, produced);
        let target: Option<Vec<Resolved>> = target.into_iter().collect();
        Some(Readiness::Check {
            check,
            target: target?.iter().map(Shown::of).collect(),
            within: Shown::of(&within?),
        })
    }

    fn restart(
        &mut self,
        user: User<'_>,
        variant: &Variant,
        default: RestartDefault,
        produced: &mut Vec<ProducedUse>,
    ) -> Option<RestartPolicy> {
        const RESTART: Place = Place::Readable("a restart policy");
        let written = variant.restart.as_ref();
        let mut on = Shown::word(default.on);
        if let Some(expr) = written.and_then(|r| r.on.as_ref()) {
            let want = Want::OneOf(&["failure", "always", "never"]);
            on = Shown::of(&self.use_value(user, expr, want, RESTART, produced)?);
        }
        let attempts = match written.and_then(|r| r.attempts.as_ref()) {
            Some(expr) => Some(Shown::of(&self.use_value(
                user,
                expr,
                Want::Number,
                RESTART,
                produced,
            )?)),
            None if on.text == "never" => None,
            None => Some(Shown::number(
                default.attempts.unwrap_or(RestartDefault::ATTEMPTS),
            )),
        };
        Some(RestartPolicy { on, attempts })
    }

    fn watch(
        &mut self,
        user: User<'_>,
        variant: &Variant,
        produced: &mut Vec<ProducedUse>,
    ) -> Vec<Option<Shown>> {
        variant
            .watch
            .iter()
            .map(|folder| {
                let value = self.use_value(user, folder, Want::Text, WATCHED, produced)?;
                // A production profile refuses any watch, so where it points does not matter there.
                if !self.production
                    && let Some(path) = value.exact_text()
                    && !self.root.join(&path).is_dir()
                {
                    self.resolver.d.error(
                        folder.loc,
                        format!("the folder `{path}` to watch does not exist"),
                    );
                    return None;
                }
                Some(Shown::of(&value))
            })
            .collect()
    }

    /// The needs as written, then each unit whose produced value is used.
    fn needs(&mut self, variant: &Variant, produced: Vec<ProducedUse>) -> Vec<NeedEdge> {
        let mut needs: Vec<NeedEdge> = variant
            .needs
            .iter()
            .filter_map(|name| {
                Some(NeedEdge {
                    unit: self.resolver.unit_index(&name.name)?,
                    loc: name.loc,
                    because: Because::Written,
                })
            })
            .collect();
        for used in produced {
            let Some(unit) = self.resolver.unit_index(&used.producer) else {
                continue;
            };
            if needs.iter().any(|need| need.unit == unit) {
                continue;
            }
            needs.push(NeedEdge {
                unit,
                loc: used.loc,
                because: Because::Uses(used.value),
            });
        }
        needs
    }
}

//! Visiting one unit that comes up: every value it uses resolved where it runs,
//! what it needs and why, how it is known to be ready, and what it gives.

use crate::definition::{Check, Way};
use crate::diagnostic::source::Loc;
use crate::module::{Place, RestartDefault, Runs, UnitKind, Want};

use super::usage::{Opening, User, Uses};
use crate::plan::Planner;
use crate::plan::model::{
    Because, Mechanism, PlannedUnit, Readiness, RestartPolicy, Running, Shown,
};
use crate::plan::resolve::Asker;
use crate::plan::resolve::value::Resolved;

const READINESS: Place = Place::Readable("a readiness check");
const WATCHED: Place = Place::Readable("a watched folder");

/// What `restart.on` may be.
const RESTART_ON: &[&str] = &["failure", "always", "never"];

/// The attempts a unit that restarts gets when its `restart` does not say.
const RESTART_ATTEMPTS: i64 = 5;

/// A unit needs another: as written, because it runs inside the other's
/// container, or because it uses a value the other makes or produces.
pub struct NeedEdge {
    pub unit: usize,
    pub loc: Loc,
    pub because: Because,
}

/// What visiting one unit adds to the plan.
pub struct Visit {
    pub way: usize,
    /// `None` when one of its values could not be resolved; that is reported.
    pub planned: Option<PlannedUnit>,
    pub needs: Vec<NeedEdge>,
    pub after: Vec<(usize, Loc)>,
    /// The published ports of other units, or its own, that its uses open.
    pub opens: Vec<Opening>,
    /// For each of its own published ports, the address and host port it is
    /// opened at, once something opens it and its values are used; an
    /// error in them has been reported.
    pub published: Vec<Option<(Option<Shown>, Option<Shown>)>>,
}

impl Planner<'_> {
    pub fn visit(&mut self, unit: usize, way_index: usize) -> Visit {
        let stack = self.resolver.stack;
        let definition = &stack.units[unit];
        let way = &definition.ways[way_index];
        let mut uses = Uses::default();
        let (mechanism, ready, restart, watch) = match &way.module {
            None => (
                Mechanism::Group,
                Some(Readiness::Group),
                Some(None),
                Vec::new(),
            ),
            Some(module) => {
                let kind = &module.shape.kind;
                let user = User {
                    unit,
                    name: &definition.name.name,
                    asker: Asker::of(kind),
                    readiness: false,
                };
                for slot in &module.shape.slots {
                    self.use_value(user, &slot.expr, slot.want, slot.place, &mut uses);
                }
                let runs = self.running(kind);
                let ready = self.readiness(user, way, module.shape.ready_without_check, &mut uses);
                let restart = self.restart(user, way, module.shape.restart_by_default, &mut uses);
                let watch = self.watch(user, way, &mut uses);
                let mechanism = Mechanism::Module {
                    module: module.module.name(),
                    way: way.name.as_ref().map(|name| name.name.clone()),
                    runs,
                };
                (mechanism, ready, restart.map(Some), watch)
            }
        };
        let published = match way.module.as_ref().map(|module| &module.shape.kind) {
            Some(UnitKind::Runs(Runs::OwnContainer { published })) => vec![None; published.len()],
            _ => Vec::new(),
        };
        let needs = self.needs(unit, way, uses.needs);
        let after = way
            .after
            .iter()
            .filter_map(|name| Some((self.resolver.unit_index(&name.name)?, name.loc)))
            .collect();
        let planned = (|| {
            Some(PlannedUnit {
                name: definition.name.name.clone(),
                doc: definition.doc.clone(),
                mechanism,
                needs: Vec::new(),
                after: Vec::new(),
                ready: ready?,
                restart: restart?,
                watch: watch.into_iter().collect::<Option<_>>()?,
                publishes: Vec::new(),
                gives: Vec::new(),
            })
        })();
        Visit {
            way: way_index,
            planned,
            needs,
            after,
            opens: uses.opens,
            published,
        }
    }

    /// Where the unit runs, as the plan says it; `None` for a unit that is
    /// made. A unit run inside another's container needs that container to
    /// be one of its own.
    fn running(&mut self, kind: &UnitKind) -> Option<Running> {
        let UnitKind::Runs(runs) = kind else {
            return None;
        };
        Some(match runs {
            Runs::OnHost => Running::OnHost,
            Runs::OwnContainer { .. } => Running::OwnContainer,
            Runs::OneOffContainer => Running::OneOffContainer,
            Runs::InsideUnit(inner) => {
                if let Some(index) = self.resolver.unit_index(&inner.name)
                    && !matches!(
                        self.resolver.kind(index),
                        Some(UnitKind::Runs(Runs::OwnContainer { .. }))
                    )
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
        })
    }

    fn readiness(
        &mut self,
        user: User<'_>,
        way: &Way,
        without_check: &'static str,
        uses: &mut Uses,
    ) -> Option<Readiness> {
        let Some(ready) = &way.ready else {
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
                vec![self.use_value(from_host, url, Want::Text, READINESS, uses)],
            ),
            Check::Tcp(address) => (
                "tcp",
                vec![self.use_value(from_host, address, Want::Text, READINESS, uses)],
            ),
            Check::Log(line) => (
                "log",
                vec![self.use_value(user, line, Want::Text, READINESS, uses)],
            ),
            Check::Command(command) => (
                "command",
                command
                    .iter()
                    .map(|word| self.use_value(user, word, Want::Scalar, READINESS, uses))
                    .collect(),
            ),
        };
        let within = self.use_value(user, &ready.within, Want::Duration, READINESS, uses);
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
        way: &Way,
        default: RestartDefault,
        uses: &mut Uses,
    ) -> Option<RestartPolicy> {
        const RESTART: Place = Place::Readable("a restart policy");
        let written = way.restart.as_ref();
        let mut on = Shown::word(default.word());
        if let Some(expr) = written.and_then(|r| r.on.as_ref()) {
            let want = Want::OneOf(RESTART_ON);
            on = Shown::of(&self.use_value(user, expr, want, RESTART, uses)?);
        }
        let attempts = match written.and_then(|r| r.attempts.as_ref()) {
            Some(expr) => Some(Shown::of(&self.use_value(
                user,
                expr,
                Want::Number,
                RESTART,
                uses,
            )?)),
            None if on.text == RestartDefault::Never.word() => None,
            None => Some(Shown::number(RESTART_ATTEMPTS)),
        };
        Some(RestartPolicy { on, attempts })
    }

    fn watch(&mut self, user: User<'_>, way: &Way, uses: &mut Uses) -> Vec<Option<Shown>> {
        way.watch
            .iter()
            .map(|folder| {
                let value = self.use_value(user, folder, Want::Text, WATCHED, uses)?;
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

    /// The needs as written, then the container a command runs inside, then
    /// each unit whose value is used, each unit once.
    fn needs(&mut self, unit: usize, way: &Way, uses: Vec<NeedEdge>) -> Vec<NeedEdge> {
        let mut needs: Vec<NeedEdge> = way
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
        if let Some(module) = &way.module
            && let UnitKind::Runs(Runs::InsideUnit(inner)) = &module.shape.kind
            && let Some(container) = self.resolver.unit_index(&inner.name)
        {
            needs.push(NeedEdge {
                unit: container,
                loc: inner.loc,
                because: Because::In,
            });
        }
        add_needs(&mut needs, unit, uses);
        needs
    }
}

/// Adds to `unit`'s needs those its uses make, each unit once. Using a value
/// of its own is no need for a unit that is made. Gives the units newly
/// needed.
pub fn add_needs(needs: &mut Vec<NeedEdge>, unit: usize, uses: Vec<NeedEdge>) -> Vec<usize> {
    let mut added = Vec::new();
    for used in uses {
        let own_made = used.unit == unit && matches!(used.because, Because::UsesMade(_));
        if own_made || needs.iter().any(|need| need.unit == used.unit) {
            continue;
        }
        added.push(used.unit);
        needs.push(used);
    }
    added
}

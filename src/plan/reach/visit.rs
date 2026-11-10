//! Visiting one unit that comes up: its settings and every other value it
//! uses resolved where it runs, what it needs and why, and how it is known
//! to be ready and restarted.

use crate::definition::{Check, Mechanism};
use crate::diagnostic::source::Loc;
use crate::module::{Duration, RestartOn, Runs, Slot, UnitKind};
use crate::syntax::names::NameAt;
use crate::value::Value;

use super::usage::{OpeningUse, User, Uses};
use crate::plan::Planner;
use crate::plan::model::{Because, PlannedMechanism, PlannedUnit, Readiness, Restart};
use crate::plan::resolve::{Asker, Chosen};

/// The attempts a unit that restarts gets when its `restart` does not say.
const RESTART_ATTEMPTS: u64 = 5;

/// A unit needs another: as written, because it runs inside the other's
/// container, or because it uses a value the other makes or produces.
pub struct NeedEdge {
    pub unit: usize,
    pub loc: Loc,
    pub because: Because,
}

/// What visiting one unit adds to the plan.
pub struct Visit<'s> {
    pub chosen: Chosen<'s>,
    /// `None` when one of its values could not be resolved; that is reported.
    pub planned: Option<PlannedUnit>,
    pub needs: Vec<NeedEdge>,
    pub after: Vec<(usize, Loc)>,
    /// The published ports of other units, or its own, that its uses open.
    pub opens: Vec<OpeningUse>,
    /// For each of its own `publish` entries, where it is opened, once
    /// something opens it and its values are used.
    pub openings: Vec<Option<OpeningPlace>>,
}

/// Where a published port is opened on the host, while the plan is made.
/// `None` for a value that could not be used, which has been reported.
#[derive(Clone)]
pub struct OpeningPlace {
    pub address: Option<Value>,
    pub host_port: Option<Value>,
}

impl<'s> Planner<'s> {
    pub fn visit(&mut self, unit: usize, chosen: Chosen<'s>) -> Visit<'s> {
        let definition = &self.resolver.stack.units[unit];
        // What it needs, waits for and gives is added once every unit that
        // comes up has been visited.
        let planned = |mechanism| PlannedUnit {
            name: definition.name.name.clone(),
            doc: definition.doc.clone(),
            mechanism,
            needs: Vec::new(),
            after: Vec::new(),
            gives: Vec::new(),
        };
        match chosen {
            Chosen::Group(needs) => Visit {
                chosen,
                planned: Some(planned(None)),
                needs: self.written_needs(needs),
                after: Vec::new(),
                opens: Vec::new(),
                openings: Vec::new(),
            },
            Chosen::Mechanism { mechanism, way } => {
                let user = User {
                    unit,
                    name: &definition.name.name,
                    asker: Asker::of(&mechanism.description.kind),
                    readiness: false,
                };
                let mut uses = Uses::default();
                let planned_mechanism = self.plan_mechanism(user, mechanism, way, &mut uses);
                let after = mechanism
                    .after
                    .iter()
                    .filter_map(|name| Some((self.resolver.unit_index(&name.name)?, name.loc)))
                    .collect();
                Visit {
                    chosen,
                    planned: planned_mechanism.map(|mechanism| planned(Some(mechanism))),
                    needs: self.needs(unit, mechanism, uses.needs),
                    after,
                    opens: uses.opens,
                    openings: vec![None; mechanism.description.publish.len()],
                }
            }
        }
    }

    /// The mechanism with its settings and fields resolved for `user`, its
    /// unit; `None` when a value cannot be, which is reported. Every value
    /// is tried, so that every error is found.
    fn plan_mechanism(
        &mut self,
        user: User<'_>,
        mechanism: &'s Mechanism,
        way: Option<(&NameAt, Loc)>,
        uses: &mut Uses,
    ) -> Option<PlannedMechanism> {
        let description = &mechanism.description;
        let settings = description.settings.resolve(&mut |slot| {
            self.use_value(user, slot, uses)
                .map(|resolved| resolved.value)
        });
        self.check_inside(&description.kind);
        let ready = self.readiness(user, mechanism, uses);
        let restart = self.restart(user, mechanism, uses);
        let watch: Vec<Option<Value>> = mechanism
            .watch
            .iter()
            .map(|watched| self.watched(user, watched, uses))
            .collect();
        Some(PlannedMechanism {
            module: mechanism.module,
            way: way.map(|(name, _)| name.name.clone()),
            kind: description.kind.clone(),
            settings: settings?,
            ready: ready?,
            restart: restart?,
            watch: watch.into_iter().collect::<Option<_>>()?,
            publishes: Vec::new(),
        })
    }

    /// A unit run inside another's container needs that container to be one
    /// of its own.
    fn check_inside(&mut self, kind: &UnitKind) {
        if let UnitKind::Runs(Runs::InsideUnit(inner)) = kind
            && let Some(index) = self.resolver.unit_index(&inner.name)
            && !matches!(
                self.resolver.kind(index),
                Some(UnitKind::Runs(Runs::OwnContainer))
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
    }

    fn readiness(
        &mut self,
        user: User<'_>,
        mechanism: &Mechanism,
        uses: &mut Uses,
    ) -> Option<Readiness> {
        let Some(ready) = &mechanism.ready else {
            return Some(Readiness::WithoutCheck(
                mechanism.description.ready_without_check,
            ));
        };
        // tay makes `http` and `tcp` checks itself, from the host.
        let from_host = User {
            asker: Asker::Host,
            readiness: true,
            ..user
        };
        let mut use_value = |user, slot: &Slot| {
            self.use_value(user, slot, uses)
                .map(|resolved| resolved.value)
        };
        let check = match &ready.check {
            Check::Http(url) => use_value(from_host, url).map(Check::Http),
            Check::Tcp(address) => use_value(from_host, address).map(Check::Tcp),
            Check::Log(line) => use_value(user, line).map(Check::Log),
            Check::Command(command) => {
                let words: Vec<Option<Value>> =
                    command.iter().map(|word| use_value(user, word)).collect();
                words.into_iter().collect::<Option<_>>().map(Check::Command)
            }
        };
        let within = use_value(user, &ready.within).map(|within| {
            let text = within.exact_text();
            Duration::parse(text.as_deref().unwrap_or_default())
                .expect("a value `Want::Duration` passes is a duration")
        });
        Some(Readiness::Check {
            check: check?,
            within: within?,
        })
    }

    fn restart(
        &mut self,
        user: User<'_>,
        mechanism: &Mechanism,
        uses: &mut Uses,
    ) -> Option<Restart> {
        let written = mechanism.restart.as_ref();
        let on = match written.and_then(|r| r.on.as_ref()) {
            Some(slot) => self.use_value(user, slot, uses).map(|on| {
                let word = on.value.exact_text();
                RestartOn::of_word(word.as_deref().unwrap_or_default())
                    .expect("a value `Want::OneOf` passes is one of its words")
            }),
            None => Some(mechanism.description.restart_by_default),
        };
        let attempts = match written.and_then(|r| r.attempts.as_ref()) {
            Some(slot) => self
                .use_value(user, slot, uses)
                .map(|attempts| match attempts.value {
                    Value::Number(n) => {
                        Some(u64::try_from(n).expect("a value `Want::Count` passes is above 0"))
                    }
                    _ => unreachable!("a value `Want::Count` passes is a number"),
                }),
            None => Some(None),
        };
        let on = on?;
        let attempts = attempts?.or((on != RestartOn::Never).then_some(RESTART_ATTEMPTS));
        Some(Restart { on, attempts })
    }

    fn watched(&mut self, user: User<'_>, watched: &Slot, uses: &mut Uses) -> Option<Value> {
        let value = self.use_value(user, watched, uses)?.value;
        // A production profile refuses any watch, so where it points does not matter there.
        if !self.production
            && let Some(path) = value.exact_text()
            && !std::path::Path::new(&path).exists()
        {
            self.resolver.d.error(
                watched.expr.loc,
                format!("`{path}`, which this unit watches, does not exist"),
            );
            return None;
        }
        Some(value)
    }

    /// The needs as written, then the container a command runs inside, then
    /// each unit whose value is used, each unit once.
    fn needs(&mut self, unit: usize, mechanism: &Mechanism, uses: Vec<NeedEdge>) -> Vec<NeedEdge> {
        let mut needs = self.written_needs(&mechanism.needs);
        if let UnitKind::Runs(Runs::InsideUnit(inner)) = &mechanism.description.kind
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

    fn written_needs(&self, written: &[NameAt]) -> Vec<NeedEdge> {
        written
            .iter()
            .filter_map(|name| {
                Some(NeedEdge {
                    unit: self.resolver.unit_index(&name.name)?,
                    loc: name.loc,
                    because: Because::Written,
                })
            })
            .collect()
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

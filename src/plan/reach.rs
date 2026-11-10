//! What comes up: the units in `up`, everything they need, and everything
//! those need, under the ways chosen. `after` brings nothing in, and
//! a unit that is an action comes up only when `up` names it.

mod given;
mod usage;
mod visit;

use std::collections::{HashMap, HashSet, VecDeque};

use crate::definition::{Profile, Stack};
use crate::diagnostic::Diagnostic;
use crate::syntax::names::NameAt;

use super::Planner;
use super::model::{After, Need, OpenedPort, Plan, PublishedPort, Shown};
use super::order::{self, Edge, EdgeKind};
use super::production::Chosen;
use super::resolve::Asker;
use crate::module::{Runs, UnitKind};
use usage::{User, Uses};
use visit::Visit;

/// The address a published port is opened on when its entry names none.
const LOOPBACK: &str = "127.0.0.1";

/// The published ports something in the plan opens, by container unit and
/// container port.
fn opened_ports(plans: &[(usize, Visit)]) -> HashSet<(String, i64)> {
    plans
        .iter()
        .flat_map(|(_, visit)| &visit.opens)
        .map(|opening| (opening.container.clone(), opening.port))
        .collect()
}

pub struct Reached {
    /// The units that come up, in the order they were found.
    pub chosen: Vec<Chosen>,
    plans: Vec<(usize, Visit)>,
}

impl Planner<'_> {
    pub fn reach(&mut self, up: &[NameAt]) -> Reached {
        let named: Vec<usize> = up
            .iter()
            .filter_map(|name| self.resolver.unit_index(&name.name))
            .collect();
        let mut queue: VecDeque<usize> = named.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut reached = Reached {
            chosen: Vec::new(),
            plans: Vec::new(),
        };
        loop {
            while let Some(unit) = queue.pop_front() {
                if !seen.insert(unit) {
                    continue;
                }
                let Some((way, chosen_at)) = self.resolver.way(unit) else {
                    continue;
                };
                let visit = self.visit(unit, way);
                let needed: Vec<usize> = visit.needs.iter().map(|need| need.unit).collect();
                self.queue_needs(unit, &visit, &needed, &named, &mut queue);
                reached.chosen.push(Chosen {
                    unit,
                    way,
                    chosen_at,
                });
                reached.plans.push((unit, visit));
            }
            // What a published port's values use comes into the plan once the
            // port is opened, and that may open more.
            if !self.open_published(&mut reached, &named, &mut queue) {
                return reached;
            }
        }
    }

    /// Queues the units `unit` newly needs. An action comes up only when
    /// `up` names it, so needing one that it does not name is an error.
    fn queue_needs(
        &mut self,
        unit: usize,
        visit: &Visit,
        needed: &[usize],
        named: &[usize],
        queue: &mut VecDeque<usize>,
    ) {
        let stack = self.resolver.stack;
        for need in visit
            .needs
            .iter()
            .filter(|need| needed.contains(&need.unit))
        {
            let needed = &stack.units[need.unit];
            if needed.action.is_some() && !named.contains(&need.unit) {
                self.resolver.d.push(Diagnostic::new(
                    need.loc,
                    format!(
                        "unit `{}` needs `{}`, which is an action: it comes up only when `up` names it",
                        stack.units[unit].name.name, needed.name.name
                    ),
                ));
            }
            queue.push_back(need.unit);
        }
    }

    /// Uses the values of each published port of a unit in the plan that
    /// something in the plan now opens, or that is opened always. Gives
    /// whether any port was newly opened.
    fn open_published(
        &mut self,
        reached: &mut Reached,
        named: &[usize],
        queue: &mut VecDeque<usize>,
    ) -> bool {
        let stack = self.resolver.stack;
        let opened = opened_ports(&reached.plans);
        let mut any = false;
        for index in 0..reached.plans.len() {
            let unit = reached.plans[index].0;
            let way = &stack.units[unit].ways[reached.plans[index].1.way];
            let Some(UnitKind::Runs(Runs::OwnContainer { published })) =
                way.module.as_ref().map(|module| &module.shape.kind)
            else {
                continue;
            };
            let name = &stack.units[unit].name.name;
            for (entry_index, entry) in published.iter().enumerate() {
                let visit = &reached.plans[index].1;
                let is_opened =
                    entry.always || opened.contains(&(name.clone(), entry.container_port));
                if visit.published[entry_index].is_some() || !is_opened {
                    continue;
                }
                let user = User {
                    unit,
                    name,
                    asker: Asker::Container,
                    readiness: false,
                };
                let mut uses = Uses::default();
                let slot = &entry.host_port;
                let host_port = self
                    .use_value(user, &slot.expr, slot.want, slot.place, &mut uses)
                    .map(|value| Shown::of(&value));
                let address = match &entry.address {
                    Some(slot) => self
                        .use_value(user, &slot.expr, slot.want, slot.place, &mut uses)
                        .map(|value| Shown::of(&value)),
                    None => Some(Shown::word(LOOPBACK)),
                };
                let visit = &mut reached.plans[index].1;
                visit.published[entry_index] = Some((address, host_port));
                let added = visit::add_needs(&mut visit.needs, unit, uses.needs);
                visit.opens.extend(uses.opens);
                let visit = &reached.plans[index].1;
                self.queue_needs(unit, visit, &added, named, queue);
                any = true;
            }
        }
        any
    }

    /// The plan, with the units in an order they could start in; `None` when
    /// there is none or a unit could not be planned, which is reported.
    pub fn finish(
        &mut self,
        reached: Reached,
        stack: &Stack,
        stack_name: &str,
        profile: Option<&Profile>,
    ) -> Option<Plan> {
        let name = |unit: usize| stack.units[unit].name.name.clone();
        let in_plan: HashSet<usize> = reached.plans.iter().map(|(unit, _)| *unit).collect();
        let mut edges = Vec::new();
        for (unit, plan) in &reached.plans {
            for need in &plan.needs {
                edges.push(Edge {
                    from: *unit,
                    to: need.unit,
                    kind: EdgeKind::Need(need.because.clone()),
                    loc: need.loc,
                });
            }
            for (waited, loc) in &plan.after {
                edges.push(Edge {
                    from: *unit,
                    to: *waited,
                    kind: EdgeKind::After,
                    loc: *loc,
                });
            }
        }
        let units: Vec<usize> = reached.plans.iter().map(|(unit, _)| *unit).collect();
        let made: HashSet<usize> = reached
            .chosen
            .iter()
            .filter(|chosen| {
                stack.units[chosen.unit].ways[chosen.way]
                    .module
                    .as_ref()
                    .is_some_and(|module| matches!(module.shape.kind, UnitKind::Made))
            })
            .map(|chosen| chosen.unit)
            .collect();
        let is_made = |unit: usize| made.contains(&unit);
        let order = order::order(&units, &name, &is_made, &edges, &mut self.resolver.d)?;
        // What each unit gives is shown as the plan leaves it, so an address
        // through a published port that nothing opens is not shown as one.
        self.resolver.opened = Some(opened_ports(&reached.plans));
        let mut openers: HashMap<(String, i64), Vec<_>> = HashMap::new();
        for opening in reached.plans.iter().flat_map(|(_, visit)| &visit.opens) {
            openers
                .entry((opening.container.clone(), opening.port))
                .or_default()
                .push(opening.opener.clone());
        }
        let mut plans: HashMap<usize, Visit> = reached.plans.into_iter().collect();
        let mut planned = Vec::new();
        for unit in order {
            let plan = plans.remove(&unit)?;
            let way = &stack.units[unit].ways[plan.way];
            let gives: Option<Vec<_>> = self.gives(unit, way).into_iter().collect();
            let mut unit_plan = plan.planned?;
            unit_plan.gives = gives?;
            if let Some(module) = &way.module
                && let UnitKind::Runs(Runs::OwnContainer { published }) = &module.shape.kind
            {
                for (entry, used) in published.iter().zip(plan.published) {
                    let opened = match used {
                        None => None,
                        Some((address, host_port)) => {
                            let mut by = openers
                                .remove(&(name(unit), entry.container_port))
                                .unwrap_or_default();
                            by.sort();
                            by.dedup();
                            Some(OpenedPort {
                                address: address?,
                                host_port: host_port?,
                                always: entry.always,
                                by,
                            })
                        }
                    };
                    unit_plan.publishes.push(PublishedPort {
                        container_port: entry.container_port,
                        opened,
                    });
                }
            }
            unit_plan.needs = plan
                .needs
                .into_iter()
                .map(|need| Need {
                    unit: name(need.unit),
                    because: need.because,
                })
                .collect();
            unit_plan.after = plan
                .after
                .iter()
                .map(|(waited, _)| After {
                    unit: name(*waited),
                    comes_up: in_plan.contains(waited),
                })
                .collect();
            planned.push(unit_plan);
        }
        Some(Plan {
            stack: stack_name.to_string(),
            root: self.resolver.environment.root.clone(),
            profile: profile.map(|profile| profile.name.name.clone()),
            production: self.production,
            units: planned,
        })
    }
}

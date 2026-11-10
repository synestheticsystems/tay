//! What comes up: the units in `up`, everything they need, and everything
//! those need, under the ways chosen. `after` brings nothing in, and
//! a unit that is an action comes up only when `up` names it.

pub mod usage;
pub mod visit;

use std::collections::{HashSet, VecDeque};

use crate::diagnostic::Diagnostic;
use crate::module::{Runs, UnitKind};
use crate::syntax::names::NameAt;

use super::Planner;
use super::model::Shown;
use super::resolve::Asker;
use super::resolve::endpoint::LOOPBACK;
use usage::{User, Uses};
use visit::Visit;

/// The published ports something in the plan opens, by container unit and
/// container port.
pub fn opened_ports(visits: &[(usize, Visit)]) -> HashSet<(String, i64)> {
    visits
        .iter()
        .flat_map(|(_, visit)| &visit.opens)
        .map(|opening| (opening.container.clone(), opening.port))
        .collect()
}

/// The units that come up, each with its visit, in the order they were
/// found.
pub struct Reached {
    pub visits: Vec<(usize, Visit)>,
}

impl Planner<'_> {
    pub fn reach(&mut self, up: &[NameAt]) -> Reached {
        let named: Vec<usize> = up
            .iter()
            .filter_map(|name| self.resolver.unit_index(&name.name))
            .collect();
        let mut queue: VecDeque<usize> = named.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut reached = Reached { visits: Vec::new() };
        loop {
            while let Some(unit) = queue.pop_front() {
                if !seen.insert(unit) {
                    continue;
                }
                let Some((way, chosen_at)) = self.resolver.way(unit) else {
                    continue;
                };
                let visit = self.visit(unit, way, chosen_at);
                let needed: Vec<usize> = visit.needs.iter().map(|need| need.unit).collect();
                self.queue_needs(unit, &visit, &needed, &named, &mut queue);
                reached.visits.push((unit, visit));
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
        let opened = opened_ports(&reached.visits);
        let mut any = false;
        for index in 0..reached.visits.len() {
            let unit = reached.visits[index].0;
            let way = &stack.units[unit].ways[reached.visits[index].1.way];
            let Some(UnitKind::Runs(Runs::OwnContainer { published })) =
                way.module.as_ref().map(|module| &module.shape.kind)
            else {
                continue;
            };
            let name = &stack.units[unit].name.name;
            for (entry_index, entry) in published.iter().enumerate() {
                let visit = &reached.visits[index].1;
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
                let visit = &mut reached.visits[index].1;
                visit.published[entry_index] = Some((address, host_port));
                let added = visit::add_needs(&mut visit.needs, unit, uses.needs);
                visit.opens.extend(uses.opens);
                let visit = &reached.visits[index].1;
                self.queue_needs(unit, visit, &added, named, queue);
                any = true;
            }
        }
        any
    }
}

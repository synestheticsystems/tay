//! The plan as it is printed, assembled once every unit that comes up has
//! been visited: the units in an order they could start in, the ports they
//! open, and the values they give.

mod given;

use std::collections::{HashMap, HashSet};

use crate::definition::{Profile, Stack};
use crate::module::{Runs, UnitKind};

use super::Planner;
use super::model::{After, Need, OpenedPort, Plan, PublishedPort};
use super::order::{self, Edge, EdgeKind};
use super::reach::visit::Visit;
use super::reach::{Reached, opened_ports};

impl Planner<'_> {
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
        let in_plan: HashSet<usize> = reached.visits.iter().map(|(unit, _)| *unit).collect();
        let mut edges = Vec::new();
        for (unit, visit) in &reached.visits {
            for need in &visit.needs {
                edges.push(Edge {
                    from: *unit,
                    to: need.unit,
                    kind: EdgeKind::Need(need.because.clone()),
                    loc: need.loc,
                });
            }
            for (waited, loc) in &visit.after {
                edges.push(Edge {
                    from: *unit,
                    to: *waited,
                    kind: EdgeKind::After,
                    loc: *loc,
                });
            }
        }
        let units: Vec<usize> = reached.visits.iter().map(|(unit, _)| *unit).collect();
        let made: HashSet<usize> = reached
            .visits
            .iter()
            .filter(|(unit, visit)| {
                stack.units[*unit].ways[visit.way]
                    .module
                    .as_ref()
                    .is_some_and(|module| matches!(module.shape.kind, UnitKind::Made))
            })
            .map(|(unit, _)| *unit)
            .collect();
        let is_made = |unit: usize| made.contains(&unit);
        let order = order::order(&units, &name, &is_made, &edges, &mut self.resolver.d)?;
        // What each unit gives is shown as the plan leaves it, so an address
        // through a published port that nothing opens is not shown as one.
        self.resolver.opened = Some(opened_ports(&reached.visits));
        let mut openers: HashMap<(String, i64), Vec<_>> = HashMap::new();
        for opening in reached.visits.iter().flat_map(|(_, visit)| &visit.opens) {
            openers
                .entry((opening.container.clone(), opening.port))
                .or_default()
                .push(opening.opener.clone());
        }
        let mut visits: HashMap<usize, Visit> = reached.visits.into_iter().collect();
        let mut planned = Vec::new();
        for unit in order {
            let visit = visits.remove(&unit)?;
            let way = &stack.units[unit].ways[visit.way];
            let gives: Option<Vec<_>> = self.gives(unit, way).into_iter().collect();
            let mut unit_plan = visit.planned?;
            unit_plan.gives = gives?;
            if let Some(module) = &way.module
                && let UnitKind::Runs(Runs::OwnContainer { published }) = &module.shape.kind
            {
                for (entry, used) in published.iter().zip(visit.published) {
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
            unit_plan.needs = visit
                .needs
                .into_iter()
                .map(|need| Need {
                    unit: name(need.unit),
                    because: need.because,
                })
                .collect();
            unit_plan.after = visit
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

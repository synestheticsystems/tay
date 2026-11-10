//! The plan as it is printed, assembled once every unit that comes up has
//! been visited: the units in an order they could start in, the ports they
//! open, and the values they give.

mod given;

use std::collections::{HashMap, HashSet};

use crate::definition::{Profile, Stack};
use crate::module::UnitKind;

use super::Planner;
use super::model::{After, Need, OpenedPort, Opener, Plan, PlannedUnit, PublishedPort};
use super::order::{self, Edge, EdgeKind};
use super::reach::visit::{OpenedAt, Visit};
use super::reach::{Reached, opened_ports};
use super::resolve::Chosen;

impl Planner<'_> {
    /// The plan, with the units in an order they could start in; `None` when
    /// there is none or a unit could not be planned, which is reported.
    pub fn finish(
        &mut self,
        reached: Reached<'_>,
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
            .filter(|(_, visit)| {
                matches!(visit.chosen, Chosen::Mechanism { mechanism, .. }
                    if matches!(mechanism.description.kind, UnitKind::Made))
            })
            .map(|(unit, _)| *unit)
            .collect();
        let is_made = |unit: usize| made.contains(&unit);
        let order = order::order(&units, &name, &is_made, &edges, &mut self.resolver.d);
        // What each unit gives is shown as the plan leaves it, so an address
        // through a published port that nothing opens is not shown as one.
        let opened = opened_ports(&reached.visits);
        let mut openers: HashMap<(String, i64), Vec<Opener>> = HashMap::new();
        for opening in reached.visits.iter().flat_map(|(_, visit)| &visit.opens) {
            openers
                .entry((opening.container.clone(), opening.port))
                .or_default()
                .push(opening.opener.clone());
        }
        // Every unit is assembled, whatever the order and whether another
        // could not be, so that every error is reported.
        let mut assembled: HashMap<usize, Option<PlannedUnit>> = HashMap::new();
        for (unit, visit) in reached.visits {
            let planned = self.assemble(unit, visit, &in_plan, &opened, &mut openers);
            assembled.insert(unit, planned);
        }
        let units = order?
            .into_iter()
            .map(|unit| {
                assembled
                    .remove(&unit)
                    .expect("every unit in the order was visited")
            })
            .collect::<Option<_>>()?;
        Some(Plan {
            stack: stack_name.to_string(),
            root: self.resolver.environment.root.clone(),
            profile: profile.map(|profile| profile.name.name.clone()),
            production: self.production,
            units,
        })
    }

    /// One unit of the plan, with what it needs and waits for, the ports it
    /// publishes and the values it gives; `None` when one of its values could
    /// not be resolved, which is reported.
    fn assemble(
        &mut self,
        unit: usize,
        visit: Visit<'_>,
        in_plan: &HashSet<usize>,
        opened: &HashSet<(String, i64)>,
        openers: &mut HashMap<(String, i64), Vec<Opener>>,
    ) -> Option<PlannedUnit> {
        let stack = self.resolver.stack;
        let name = |unit: usize| stack.units[unit].name.name.clone();
        let mechanism = match visit.chosen {
            Chosen::Mechanism { mechanism, .. } => Some(mechanism),
            Chosen::Group(_) => None,
        };
        let gives: Option<Vec<_>> = match mechanism {
            Some(mechanism) => self.gives(unit, mechanism, opened).into_iter().collect(),
            None => Some(Vec::new()),
        };
        let publishes: Option<Vec<PublishedPort>> = mechanism
            .map(|mechanism| &mechanism.description.published[..])
            .unwrap_or_default()
            .iter()
            .zip(visit.published)
            .map(|(entry, used)| {
                let Some(OpenedAt { address, host_port }) = used else {
                    return Some(PublishedPort {
                        container_port: entry.container_port,
                        opened: None,
                    });
                };
                let mut by = openers
                    .remove(&(name(unit), entry.container_port))
                    .unwrap_or_default();
                by.sort();
                by.dedup();
                Some(PublishedPort {
                    container_port: entry.container_port,
                    opened: Some(OpenedPort {
                        address: address?,
                        host_port: host_port?,
                        always: entry.always,
                        by,
                    }),
                })
            })
            .collect();
        let mut planned = visit.planned?;
        planned.gives = gives?;
        if let Some(mechanism) = &mut planned.mechanism {
            mechanism.publishes = publishes?;
        }
        planned.needs = visit
            .needs
            .into_iter()
            .map(|need| Need {
                unit: name(need.unit),
                because: need.because,
            })
            .collect();
        planned.after = visit
            .after
            .iter()
            .map(|(waited, _)| After {
                unit: name(*waited),
                comes_up: in_plan.contains(waited),
            })
            .collect();
        Some(planned)
    }
}

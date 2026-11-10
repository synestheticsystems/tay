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
use super::model::{After, Because, Need, Plan};
use super::order::{self, Edge, EdgeKind};
use super::production::Chosen;
use visit::Visit;

pub struct Reached {
    /// The units that come up, in the order they were found.
    pub chosen: Vec<Chosen>,
    plans: Vec<(usize, Visit)>,
}

impl Planner<'_> {
    pub fn reach(&mut self, up: &[NameAt]) -> Reached {
        let stack = self.resolver.stack;
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
        while let Some(unit) = queue.pop_front() {
            if !seen.insert(unit) {
                continue;
            }
            let Some((way, chosen_at)) = self.resolver.way(unit) else {
                continue;
            };
            let plan = self.visit(unit, way);
            for need in &plan.needs {
                let needed = &stack.units[need.unit];
                if needed.action && !named.contains(&need.unit) {
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
            reached.chosen.push(Chosen {
                unit,
                way,
                chosen_at,
            });
            reached.plans.push((unit, plan));
        }
        reached
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
                    kind: match &need.because {
                        Because::Written => EdgeKind::Needs,
                        Because::Uses(value) => EdgeKind::Uses(value.clone()),
                    },
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
        let order = order::order(&units, &name, &edges, &mut self.resolver.d)?;
        let mut plans: HashMap<usize, Visit> = reached.plans.into_iter().collect();
        let mut planned = Vec::new();
        for unit in order {
            let plan = plans.remove(&unit)?;
            let mut unit_plan = plan.planned?;
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

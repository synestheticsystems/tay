//! The plan as it is printed, assembled once every unit that comes up has
//! been visited: the units in an order they could start in, the ports they
//! open, the values they give, and the units that run that each needs.

mod given;

use std::collections::{HashMap, HashSet};

use crate::definition::{Profile, Stack};
use crate::module::UnitKind;

use super::Planner;
use super::model::{After, Need, Opener, Opening, Plan, PlannedUnit, PublishedPort};
use super::order::{self, Edge, EdgeKind};
use super::reach::visit::{OpeningPlace, Visit};
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
        up: Vec<String>,
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
        for opening_use in reached.visits.iter().flat_map(|(_, visit)| &visit.opens) {
            openers
                .entry((opening_use.container.clone(), opening_use.port))
                .or_default()
                .push(opening_use.opener.clone());
        }
        // Every unit is assembled, whatever the order and whether another
        // could not be, so that every error is reported.
        let mut assembled: HashMap<usize, Option<PlannedUnit>> = HashMap::new();
        for (unit, visit) in reached.visits {
            let planned = self.assemble(unit, visit, &in_plan, &opened, &mut openers);
            assembled.insert(unit, planned);
        }
        let mut units: Vec<PlannedUnit> = order?
            .into_iter()
            .map(|unit| {
                assembled
                    .remove(&unit)
                    .expect("every unit in the order was visited")
            })
            .collect::<Option<_>>()?;
        give_needs_that_run(&mut units);
        Some(Plan {
            stack: stack_name.to_string(),
            root: self.resolver.environment.root.clone(),
            profile: profile.map(|profile| profile.name.name.clone()),
            production: self.production,
            up,
            units,
            uses_host_listen: self.resolver.uses_host_listen,
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
            .map(|mechanism| &mechanism.description.publish[..])
            .unwrap_or_default()
            .iter()
            .zip(visit.openings)
            .map(|(entry, place)| {
                let Some(OpeningPlace { address, host_port }) = place else {
                    return Some(PublishedPort {
                        container_port: entry.container_port,
                        opening: None,
                    });
                };
                let mut port_openers = openers
                    .remove(&(name(unit), entry.container_port))
                    .unwrap_or_default();
                port_openers.sort();
                port_openers.dedup();
                Some(PublishedPort {
                    container_port: entry.container_port,
                    opening: Some(Opening {
                        address: address?,
                        host_port: host_port?,
                        always: entry.always,
                        openers: port_openers,
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

/// Gives the mechanism of each of `units`, which are in the order units
/// start in, the units that run that it needs, a group standing for those
/// it needs in turn, in that order.
fn give_needs_that_run(units: &mut [PlannedUnit]) {
    let by_name: HashMap<&str, &PlannedUnit> = (units.iter())
        .map(|planned| (planned.name.as_str(), planned))
        .collect();
    let needs_that_run: Vec<Vec<String>> = (units.iter())
        .map(|planned| {
            let mut reached: HashSet<&str> = HashSet::new();
            let mut through = vec![planned];
            while let Some(needing) = through.pop() {
                for need in &needing.needs {
                    let Some(&needed) = by_name.get(need.unit.as_str()) else {
                        continue;
                    };
                    if reached.insert(needed.name.as_str()) && needed.mechanism.is_none() {
                        through.push(needed);
                    }
                }
            }
            (units.iter())
                .filter(|unit| reached.contains(unit.name.as_str()))
                .filter(|unit| {
                    (unit.mechanism.as_ref())
                        .is_some_and(|mechanism| matches!(mechanism.kind, UnitKind::Runs(_)))
                })
                .map(|unit| unit.name.clone())
                .collect()
        })
        .collect();
    for (planned, needs) in units.iter_mut().zip(needs_that_run) {
        if let Some(mechanism) = &mut planned.mechanism {
            mechanism.needs_that_run = needs;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::Diagnostics;
    use crate::diagnostic::source::Sources;
    use crate::plan::{self, Options, Outcome};

    #[test]
    fn a_unit_needs_what_runs_that_it_needs_a_group_standing_for_its_own_needs() {
        let root = tempfile::tempdir().unwrap();
        let stack = r#"format: 1
stack: needing
up: [undoer]
units:
  box: { uses: container, with: { image: alpine } }
  service: { uses: process, with: { command: [sleep, "1"] } }
  inner: { uses: process, with: { command: [sleep, "1"] } }
  core: { needs: [inner, port] }
  port: { uses: port }
  password: { uses: secret, with: { kind: password } }
  undoer:
    uses: command
    needs: [service, core]
    with: { in: box, run: ["true"], secret_env: { PASSWORD: "${password.value}" } }
"#;
        std::fs::write(root.path().join("tay.yaml"), stack).unwrap();
        let options = Options {
            asked: crate::disk::Asking::default(),
            home: None,
            shared_folders: None,
            action: None,
        };
        let (mut sources, mut d) = (Sources::default(), Diagnostics::default());
        let Outcome::Plan(plan) = plan::plan(root.path(), &options, &mut sources, &mut d) else {
            panic!("the stack plans: {:?}", d.into_sorted());
        };
        let needs_that_run = |unit: &str| {
            let planned = plan.units.iter().find(|planned| planned.name == unit);
            let mechanism = planned.unwrap().mechanism.as_ref().unwrap();
            mechanism.needs_that_run.clone()
        };
        // The container it runs in, what it needs, and what the group it
        // needs needs, but not the port, the group, or the secret, in the
        // order units start in.
        let wanted = ["box", "service", "inner"];
        let in_order: Vec<&str> = (plan.units.iter())
            .map(|planned| planned.name.as_str())
            .filter(|unit| wanted.contains(unit))
            .collect();
        assert_eq!(needs_that_run("undoer"), in_order);
        assert!(needs_that_run("service").is_empty());
    }
}

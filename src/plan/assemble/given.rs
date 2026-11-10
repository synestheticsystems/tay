//! The values a unit gives, as the plan prints them: each seen from the host
//! and from a container, since an address depends on who asks.

use std::collections::HashSet;

use crate::definition::{GiveValue, Mechanism};
use crate::diagnostic::source::Loc;
use crate::syntax::expr::{Expr, ExprKind, Part, Reference, Target, UnitRef};

use crate::plan::Planner;
use crate::plan::model::{Given, Seen};
use crate::plan::resolve::value::Resolved;
use crate::plan::resolve::{Asker, Fail, Scope, ViewProblem};

use crate::plan::reach::usage::view_error;

impl Planner<'_> {
    /// Each value the unit gives, as the host and as a container would see
    /// it. An address through a published port is one the host can have only
    /// if the port is opened: by `always`, or by a use in `opened`.
    pub(super) fn gives(
        &mut self,
        unit: usize,
        mechanism: &Mechanism,
        opened: &HashSet<(String, i64)>,
    ) -> Vec<Option<Given>> {
        let unit_name = &self.resolver.stack.units[unit].name.name;
        let mut lines = Vec::new();
        for give in &mechanism.gives {
            let endpoint = matches!(give.value, GiveValue::Endpoint(_));
            let has_scheme = matches!(&give.value, GiveValue::Endpoint(e) if e.scheme.is_some());
            let target = Target::Value {
                unit: UnitRef::Named(unit_name.clone()),
                name: give.name.name.clone(),
                part: has_scheme.then_some(Part::Url),
                view: None,
            };
            lines.push(self.given_line(
                unit_name,
                give.name.name.clone(),
                give.name.loc,
                target,
                endpoint,
                opened,
            ));
        }
        for output in mechanism.outputs() {
            let target = Target::Output {
                unit: UnitRef::Named(unit_name.clone()),
                name: output.name.name.clone(),
                view: None,
            };
            lines.push(self.given_line(
                unit_name,
                format!("outputs.{}", output.name.name),
                output.name.loc,
                target,
                false,
                opened,
            ));
        }
        lines
    }

    fn given_line(
        &mut self,
        unit_name: &str,
        name: String,
        loc: Loc,
        target: Target,
        endpoint: bool,
        opened: &HashSet<(String, i64)>,
    ) -> Option<Given> {
        let reference = Expr {
            loc,
            kind: ExprKind::Ref(Reference {
                loc,
                inner: format!("{unit_name}.{name}"),
                target,
            }),
        };
        let mut seen = |asker: Asker| -> Option<(Seen, Option<Resolved>)> {
            let scope = Scope { unit: None, asker };
            let problems = match self.resolver.resolve(&reference, scope) {
                Ok(value) => {
                    let closed: Vec<String> = value
                        .published_uses
                        .iter()
                        .filter(|used| {
                            !used.always && !opened.contains(&(used.container.clone(), used.port))
                        })
                        .map(|used| format!(
                            "its port {} is published, but nothing in the plan opens it on the host",
                            used.port
                        ))
                        .collect();
                    return Some(match closed.is_empty() {
                        true => (Seen::Value(value.value.clone()), Some(value)),
                        false => (Seen::Unreachable(closed.join("; ")), None),
                    });
                }
                Err(Fail::Reported) => return None,
                Err(Fail::View(problems)) => problems,
            };
            let mut unreachable = Vec::new();
            for problem in problems {
                match &problem {
                    ViewProblem::NotPublished { port, .. } => {
                        unreachable.push(format!("its port {port} is not published to the host"));
                    }
                    // An address with no view in a value of a unit that is
                    // made has no form for anyone.
                    ViewProblem::NeedsView { .. } => {
                        let who = "a value of a unit that is made, which runs nowhere, holds it";
                        self.resolver.d.push(view_error(who, problem.clone()));
                    }
                }
            }
            match unreachable.is_empty() {
                true => None,
                false => Some((Seen::Unreachable(unreachable.join("; ")), None)),
            }
        };
        let (from_host, host_value) = seen(Asker::Host)?;
        let (from_containers, container_value) = seen(Asker::Container)?;
        let value = host_value.or(container_value);
        Some(Given {
            name,
            endpoint,
            producers: value
                .as_ref()
                .map(|v| v.producers.keys().cloned().collect())
                .unwrap_or_default(),
            secret: value.as_ref().is_some_and(|value| value.value.is_secret()),
            from_host,
            from_containers,
        })
    }
}

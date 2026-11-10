//! The values a unit gives, as the plan prints them: each seen from the host
//! and from a container, since an address depends on who asks.

use crate::definition::{GiveValue, Way};
use crate::diagnostic::source::Loc;
use crate::syntax::expr::{Expr, ExprKind, Part, Reference, Target, UnitRef};

use crate::plan::Planner;
use crate::plan::model::{GivenLine, Seen, Shown};
use crate::plan::resolve::value::Resolved;
use crate::plan::resolve::{Asker, Fail, Scope, ViewProblem};

impl Planner<'_> {
    /// Each value the unit gives, as the host and as a container would see it.
    pub(super) fn gives(&mut self, unit: usize, way: &Way) -> Vec<Option<GivenLine>> {
        let unit_name = &self.resolver.stack.units[unit].name.name;
        let mut lines = Vec::new();
        for give in &way.gives {
            let endpoint = matches!(give.value, GiveValue::Endpoint(_));
            let has_scheme = matches!(&give.value, GiveValue::Endpoint(e) if e.scheme.is_some());
            let target = Target::Value {
                unit: UnitRef::Named(unit_name.clone()),
                name: give.name.name.clone(),
                part: has_scheme.then_some(Part::Url),
                view: None,
            };
            lines.push(self.given_line(give.name.name.clone(), give.name.loc, target, endpoint));
        }
        for output in way.outputs() {
            let target = Target::Output {
                unit: UnitRef::Named(unit_name.clone()),
                name: output.name.name.clone(),
                view: None,
            };
            lines.push(self.given_line(
                format!("outputs.{}", output.name.name),
                output.name.loc,
                target,
                false,
            ));
        }
        lines
    }

    fn given_line(
        &mut self,
        name: String,
        loc: Loc,
        target: Target,
        endpoint: bool,
    ) -> Option<GivenLine> {
        let reference = Expr {
            loc,
            kind: ExprKind::Ref(Reference {
                loc,
                inner: name.clone(),
                target,
            }),
        };
        let mut seen = |asker: Asker| -> Option<(Seen, Option<Resolved>)> {
            let scope = Scope { unit: None, asker };
            match self.resolver.resolve(&reference, scope) {
                Ok(value) => Some((Seen::Value(Shown::of(&value)), Some(value))),
                Err(Fail::View(ViewProblem::NotPublished { port, .. })) => Some((
                    Seen::Unreachable(format!("its port {port} is not published to the host")),
                    None,
                )),
                Err(Fail::View(ViewProblem::NotOpened { port, .. })) => Some((
                    Seen::Unreachable(format!(
                        "its port {port} is published, but nothing in the plan opens it on the host"
                    )),
                    None,
                )),
                Err(Fail::View(ViewProblem::NeedsView { .. })) | Err(Fail::Reported) => None,
            }
        };
        let (from_host, host_value) = seen(Asker::Host)?;
        let (from_containers, container_value) = seen(Asker::Container)?;
        let value = host_value.or(container_value);
        Some(GivenLine {
            name,
            endpoint,
            producers: value
                .as_ref()
                .map(|v| v.producers.keys().cloned().collect())
                .unwrap_or_default(),
            secret: value.as_ref().is_some_and(Resolved::is_secret),
            from_host,
            from_containers,
        })
    }
}

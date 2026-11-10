//! An endpoint seen by whoever asks for it.
//!
//! |                     | it belongs to a unit on the host | it belongs to a unit in a container |
//! |---------------------|----------------------------------|-------------------------------------|
//! | asked from the host | `127.0.0.1` and its port         | `127.0.0.1` and the published port  |
//! | asked from a container | the host's name for containers, and its port | the unit's name and the port inside |
//!
//! An endpoint that writes its `host` has no views: everyone gets that host
//! and that port.

use crate::definition::Endpoint;
use crate::diagnostic::Diagnostic;
use crate::module::{Form, HOST_IN_CONTAINERS, PublishEntry, Runs, UnitKind, Want};
use crate::syntax::expr::{Expr, Part, Reference};

use super::{Asker, Fail, Res, Resolver, Scope, ViewProblem};
use crate::plan::resolve::resolved::{ReachedEntry, Resolved};
use crate::value::{Bit, Exact, Value};

/// The host's own address, which a unit on the host reaches another at and
/// a published port is opened on unless its entry names another.
pub const LOOPBACK: &str = "127.0.0.1";

/// The schemes whose usual port a URL leaves out, and those ports. No other
/// scheme has one.
const USUAL_PORTS: [(&str, i64); 2] = [("http", 80), ("https", 443)];

impl<'s> Resolver<'s> {
    pub(super) fn endpoint(
        &mut self,
        owner: usize,
        endpoint: &Endpoint,
        part: Option<Part>,
        asker: Asker,
        reference: &Reference,
    ) -> Res {
        let kind = self.kind(owner).ok_or(Fail::Reported)?;
        // The endpoint's own fields are in its unit's terms: the port it listens on.
        let own = Scope {
            unit: Some(owner),
            asker: Asker::of(kind),
        };
        // Every part is resolved, so that an error in each is found.
        let port = self.part(&endpoint.port, Endpoint::PORT, own);
        let scheme = endpoint
            .scheme
            .as_ref()
            .map(|scheme| self.part(scheme, Endpoint::SCHEME, own))
            .transpose();
        let path = endpoint
            .path
            .as_ref()
            .map(|path| self.path(path, own))
            .transpose();
        let host = endpoint
            .host
            .as_ref()
            .map(|host| self.part(host, Endpoint::HOST, own))
            .transpose();
        let (port, scheme, path, host) = (port?, scheme?, path?, host?);
        let (host, seen_port) = match host {
            Some(host) => (host, port),
            None => self.seen(owner, endpoint, kind, asker, reference, port)?,
        };
        let mut value = Resolved::known(Value::Text(Vec::new()));
        for part in [
            Some(&host),
            Some(&seen_port),
            scheme.as_ref(),
            path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            value.absorb(part);
        }
        let mut bits = Vec::new();
        match part {
            None => {
                bits.extend(host.value.bits());
                bits.push(Bit::Text(":".to_string()));
                bits.extend(seen_port.value.bits());
            }
            Some(Part::Host) => bits.extend(host.value.bits()),
            Some(Part::Port) => {
                return Ok(Resolved {
                    value: seen_port.value.clone(),
                    ..value
                });
            }
            Some(Part::Url) => {
                let scheme = scheme
                    .as_ref()
                    .expect("a URL is asked for only of an endpoint with a scheme");
                bits.extend(scheme.value.bits());
                bits.push(Bit::Text("://".to_string()));
                bits.extend(host.value.bits());
                if !is_usual_port(scheme, &seen_port) {
                    bits.push(Bit::Text(":".to_string()));
                    bits.extend(seen_port.value.bits());
                }
                if let Some(path) = &path {
                    bits.extend(path.value.bits());
                }
            }
        }
        value.value = Value::from_bits(bits);
        Ok(value)
    }

    /// A part of an endpoint, resolved in its own unit's terms and checked
    /// against what it must be.
    fn part(&mut self, expr: &Expr, want: Want, own: Scope) -> Res {
        let value = self.resolve(expr, own)?;
        match want.problem(&Form::of(&value.value)) {
            Some(problem) => self.fail(Diagnostic::new(
                expr.loc,
                format!("{} {problem}", expr.subject()),
            )),
            None => Ok(value),
        }
    }

    /// An endpoint's `path`, which also starts with `/`.
    fn path(&mut self, expr: &Expr, own: Scope) -> Res {
        let value = self.part(expr, Endpoint::PATH, own)?;
        if let Value::Text(bits) = &value.value
            && let Some(Bit::Text(leading)) = bits.first()
            && let Some(problem) = Endpoint::path_problem(leading)
        {
            return self.fail(Diagnostic::new(expr.loc, problem));
        }
        Ok(value)
    }

    /// The host and port of an endpoint that writes no `host`, as `asker`
    /// sees them.
    fn seen(
        &mut self,
        owner: usize,
        endpoint: &Endpoint,
        kind: &UnitKind,
        asker: Asker,
        reference: &Reference,
        port: Resolved,
    ) -> Result<(Resolved, Resolved), Fail> {
        let owner_name = &self.stack.units[owner].name.name;
        // The reader refuses an endpoint with no `host` on a unit that is made.
        let UnitKind::Runs(runs) = kind else {
            return Err(Fail::Reported);
        };
        let not_published = |port: &Resolved| {
            Err(Fail::View(vec![ViewProblem::NotPublished {
                owner: owner_name.clone(),
                port: port.value.shown(),
                at: reference.loc,
                endpoint: endpoint.loc,
            }]))
        };
        let (host, port) = match (asker, runs) {
            (Asker::Nowhere, _) => {
                return Err(Fail::View(vec![ViewProblem::NeedsView {
                    reference: reference.written(),
                    at: reference.loc,
                    endpoint: endpoint.loc,
                }]));
            }
            (Asker::Host, Runs::OnHost) => (LOOPBACK.to_string(), port),
            (Asker::Host, Runs::OneOffContainer) => return not_published(&port),
            (Asker::Host, Runs::OwnContainer | Runs::InsideUnit(_)) => {
                let container = match runs {
                    Runs::InsideUnit(inner) => {
                        self.unit_index(&inner.name).ok_or(Fail::Reported)?
                    }
                    _ => owner,
                };
                let Form::Number(Some(number)) = Form::of(&port.value) else {
                    return not_published(&port);
                };
                let Some(entry) = self.publish_entry(container, number) else {
                    return not_published(&port);
                };
                let scope = Scope {
                    unit: Some(container),
                    asker: Asker::Container,
                };
                let mut host_port = self.resolve(&entry.host_port.expr, scope)?;
                // Whether something opens the port is known only once the
                // whole plan is: whoever uses the value judges that.
                host_port.reached_entries.insert(ReachedEntry {
                    container: self.stack.units[container].name.name.clone(),
                    port: number,
                    always: entry.always,
                });
                (LOOPBACK.to_string(), host_port)
            }
            (Asker::Container, Runs::OnHost) => (HOST_IN_CONTAINERS.to_string(), port),
            (Asker::Container, Runs::OwnContainer | Runs::OneOffContainer) => {
                (owner_name.clone(), port)
            }
            (Asker::Container, Runs::InsideUnit(inner)) => (inner.name.clone(), port),
        };
        Ok((Resolved::text(host), port))
    }

    /// The `publish` entry of the container `unit` for its port `port`, if any.
    fn publish_entry(&mut self, unit: usize, port: i64) -> Option<&'s PublishEntry> {
        self.mechanism(unit)?
            .description
            .publish
            .iter()
            .find(|entry| entry.container_port == port)
    }
}

/// Whether `port` is the usual one of `scheme`, which a URL leaves out.
/// Both must be known before start to tell.
fn is_usual_port(scheme: &Resolved, port: &Resolved) -> bool {
    let (Some(Exact::Text(scheme)), Some(Exact::Number(port))) =
        (scheme.value.exact(), port.value.exact())
    else {
        return false;
    };
    USUAL_PORTS.contains(&(scheme.as_str(), port))
}

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
use crate::module::{Form, Published, Runs, UnitKind};
use crate::syntax::expr::{Part, Reference};

use super::{Asker, Fail, Res, Resolver, Scope, ViewProblem};
use crate::plan::resolve::value::{Bit, Exact, Resolved, Shape};

const LOOPBACK: &str = "127.0.0.1";

/// The name Docker gives the host inside its containers.
const HOST_SEEN_FROM_CONTAINERS: &str = "host.docker.internal";

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
        let port = self.resolve(&endpoint.port, own)?;
        if !matches!(port.form(), Form::Number(_) | Form::Unknown) {
            return self.fail(
                endpoint.port.loc,
                format!("an endpoint's `port` is a number; this is {}", port.shown()),
            );
        }
        let scheme = match &endpoint.scheme {
            Some(scheme) => Some(self.resolve(scheme, own)?),
            None => None,
        };
        let path = match &endpoint.path {
            Some(path) => Some(self.resolve(path, own)?),
            None => None,
        };
        let (host, seen_port) = match &endpoint.host {
            Some(host) => (self.resolve(host, own)?, port),
            None => self.seen(owner, endpoint, kind, asker, reference, port)?,
        };
        let mut value = Resolved::known(Shape::Text(Vec::new()));
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
                bits.extend(host.bits());
                bits.push(Bit::Text(":".to_string()));
                bits.extend(seen_port.bits());
            }
            Some(Part::Host) => bits.extend(host.bits()),
            Some(Part::Port) => {
                return Ok(Resolved {
                    shape: seen_port.shape.clone(),
                    ..value
                });
            }
            Some(Part::Url) => {
                let Some(scheme) = &scheme else {
                    return self.fail(
                        reference.loc,
                        format!(
                            "`{}`: the endpoint has no `scheme`, so it has no URL",
                            reference.written()
                        ),
                    );
                };
                bits.extend(scheme.bits());
                bits.push(Bit::Text("://".to_string()));
                bits.extend(host.bits());
                if !is_usual_port(scheme, &seen_port) {
                    bits.push(Bit::Text(":".to_string()));
                    bits.extend(seen_port.bits());
                }
                if let Some(path) = &path {
                    bits.extend(path.bits());
                }
            }
        }
        value.shape = Shape::Text(bits);
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
                port: port.shown(),
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
            (Asker::Host, Runs::OwnContainer { .. } | Runs::InsideUnit(_)) => {
                let container = match runs {
                    Runs::InsideUnit(inner) => {
                        self.unit_index(&inner.name).ok_or(Fail::Reported)?
                    }
                    _ => owner,
                };
                let Form::Number(Some(number)) = port.form() else {
                    return not_published(&port);
                };
                let Some(entry) = self.published(container, number) else {
                    return not_published(&port);
                };
                let container_name = self.stack.units[container].name.name.clone();
                if let Some(opened) = &self.opened
                    && !entry.always
                    && !opened.contains(&(container_name.clone(), number))
                {
                    return Err(Fail::View(vec![ViewProblem::NotOpened {
                        owner: owner_name.clone(),
                        port: port.shown(),
                    }]));
                }
                let scope = Scope {
                    unit: Some(container),
                    asker: Asker::Container,
                };
                let mut host_port = self.resolve(&entry.host_port.expr, scope)?;
                host_port.published_uses.insert((container_name, number));
                (LOOPBACK.to_string(), host_port)
            }
            (Asker::Container, Runs::OnHost) => (HOST_SEEN_FROM_CONTAINERS.to_string(), port),
            (Asker::Container, Runs::OwnContainer { .. } | Runs::OneOffContainer) => {
                (owner_name.clone(), port)
            }
            (Asker::Container, Runs::InsideUnit(inner)) => (inner.name.clone(), port),
        };
        Ok((Resolved::text(host), port))
    }

    /// The `publish` entry of the container `unit` for its port `port`, if any.
    fn published(&mut self, unit: usize, port: i64) -> Option<&'s Published> {
        let Some(UnitKind::Runs(Runs::OwnContainer { published })) = self.kind(unit) else {
            return None;
        };
        published.iter().find(|entry| entry.container_port == port)
    }
}

/// Whether `port` is the usual one of `scheme`, which a URL leaves out.
/// Both must be known before start to tell.
fn is_usual_port(scheme: &Resolved, port: &Resolved) -> bool {
    let (Some(Exact::Text(scheme)), Some(Exact::Number(port))) = (scheme.exact(), port.exact())
    else {
        return false;
    };
    USUAL_PORTS.contains(&(scheme.as_str(), port))
}

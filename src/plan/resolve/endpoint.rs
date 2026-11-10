//! An endpoint seen by whoever asks for it.
//!
//! |                     | it belongs to a unit on the host | it belongs to a unit in a container |
//! |---------------------|----------------------------------|-------------------------------------|
//! | asked from the host | `127.0.0.1` and its port         | `127.0.0.1` and the published port  |
//! | asked from a container | the host's name for containers, and its port | the unit's name and the port inside |

use crate::definition::Endpoint;
use crate::expr::{Part, Reference};
use crate::module::{Form, Runs};

use super::{Asker, Fail, Res, Resolver, Scope, ViewProblem};
use crate::plan::value::{Bit, Resolved, Shape};

const LOOPBACK: &str = "127.0.0.1";

/// The name Docker gives the host inside its containers.
const HOST_SEEN_FROM_CONTAINERS: &str = "host.docker.internal";

impl Resolver<'_> {
    pub(super) fn endpoint(
        &mut self,
        owner: usize,
        endpoint: &Endpoint,
        part: Option<Part>,
        asker: Asker,
        reference: &Reference,
    ) -> Res {
        let stack = self.stack;
        let owner_name = &stack.units[owner].name.name;
        let runs = self.runs(owner).ok_or(Fail::Reported)?;
        // The endpoint's own fields are in its unit's terms: the port it listens on.
        let own = Scope {
            unit: Some(owner),
            asker: Asker::of(runs),
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
        let not_published = |port: &Resolved| {
            Err(Fail::View(ViewProblem::NotPublished {
                owner: owner_name.clone(),
                port: port.shown(),
                endpoint: endpoint.loc,
            }))
        };
        let (host, seen_port) = match (asker, runs) {
            (Asker::Unplaced, _) => {
                return Err(Fail::View(ViewProblem::NeedsView {
                    reference: reference.written(),
                    endpoint: endpoint.loc,
                }));
            }
            (_, Runs::ByTay) => {
                return self.fail(
                    endpoint.loc,
                    format!(
                        "unit `{owner_name}` runs nothing that listens, so an endpoint it gives has no address"
                    ),
                );
            }
            (Asker::Host, Runs::OnHost) => (LOOPBACK.to_string(), port.clone()),
            (Asker::Host, Runs::OneOffContainer) => return not_published(&port),
            (Asker::Host, Runs::OwnContainer { .. } | Runs::InsideUnit(_)) => {
                let container = match runs {
                    Runs::InsideUnit(inner) => {
                        self.unit_index(&inner.name).ok_or(Fail::Reported)?
                    }
                    _ => owner,
                };
                match self.published(container, &port)? {
                    Some(host_port) => (LOOPBACK.to_string(), host_port),
                    None => return not_published(&port),
                }
            }
            (Asker::Container, Runs::OnHost) => {
                (HOST_SEEN_FROM_CONTAINERS.to_string(), port.clone())
            }
            (Asker::Container, Runs::OwnContainer { .. } | Runs::OneOffContainer) => {
                (owner_name.clone(), port.clone())
            }
            (Asker::Container, Runs::InsideUnit(inner)) => (inner.name.clone(), port.clone()),
        };
        let mut value = Resolved::known(Shape::Text(Vec::new()));
        for part in [Some(&seen_port), scheme.as_ref(), path.as_ref()]
            .into_iter()
            .flatten()
        {
            value.absorb(part);
        }
        let address = |bits: &mut Vec<Bit>| {
            bits.push(Bit::Text(format!("{host}:")));
            bits.extend(seen_port.bits());
        };
        let mut bits = Vec::new();
        match part {
            None => address(&mut bits),
            Some(Part::Host) => bits.push(Bit::Text(host.clone())),
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
                address(&mut bits);
                if let Some(path) = &path {
                    bits.extend(path.bits());
                }
            }
        }
        value.shape = Shape::Text(bits);
        Ok(value)
    }

    /// The host port published for `port` of `container`'s container, if any.
    fn published(&mut self, container: usize, port: &Resolved) -> Result<Option<Resolved>, Fail> {
        let Some(Runs::OwnContainer { published }) = self.runs(container) else {
            return Ok(None);
        };
        let Form::Number(Some(port)) = port.form() else {
            return Ok(None);
        };
        let Some(entry) = published.iter().find(|p| p.container_port == port) else {
            return Ok(None);
        };
        let scope = Scope {
            unit: Some(container),
            asker: Asker::Container,
        };
        Ok(Some(self.resolve(&entry.host_port, scope)?))
    }
}

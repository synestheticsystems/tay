//! `port`: a host port held for this environment.

use crate::syntax::fields::Fields;
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::Value;

use crate::module::description::{Description, ModuleGive, ModuleValue, RestartDefault, UnitKind};
use crate::module::settings::{Destination, Settings, Slot, WrittenSettings};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Port;

/// The number a port unit is held at, a [`Slot`] as written or a [`Value`]
/// once resolved.
#[derive(Debug)]
enum PortSettings<V> {
    /// Any number tay chooses.
    Any,
    /// A number to try first.
    Prefer(V),
    /// A number that must be had.
    Fixed(V),
}

impl<V> PortSettings<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<PortSettings<W>> {
        Some(match self {
            PortSettings::Any => PortSettings::Any,
            PortSettings::Prefer(number) => PortSettings::Prefer(f(number)?),
            PortSettings::Fixed(number) => PortSettings::Fixed(f(number)?),
        })
    }
}

impl WrittenSettings for PortSettings<Slot> {
    fn slots(&self) -> Vec<&Slot> {
        let mut slots = Vec::new();
        self.map(&mut |slot| {
            slots.push(slot);
            Some(())
        });
        slots
    }

    fn resolve(
        &self,
        resolve: &mut dyn FnMut(&Slot) -> Option<Value>,
    ) -> Option<Box<dyn Settings>> {
        Some(Box::new(self.map(resolve)?))
    }
}

impl Settings for PortSettings<Value> {}

impl Module for Port {
    fn name(&self) -> &'static str {
        "port"
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        Some(Description {
            gives: vec![ModuleGive {
                name: NameAt {
                    name: "number".to_string(),
                    loc: cx.uses,
                },
                value: ModuleValue::Port,
            }],
            ..Description::new(
                UnitKind::Made,
                Box::new(settings),
                "when it is held",
                RestartDefault::Never,
            )
        })
    }
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<PortSettings<Slot>> {
    let Some(with) = with else {
        return Some(PortSettings::Any);
    };
    let errors_before = cx.d.len();
    let d = &mut *cx.d;
    let settings = Fields::of(
        with,
        "the settings of a `port` unit",
        &["prefer", "fixed"],
        d,
    )?;
    if let (Some(_), Some((key, _))) = (settings.entry("prefer"), settings.entry("fixed")) {
        d.error(
            key.loc,
            "a `port` unit has at most one of `prefer` and `fixed`; this one has both",
        );
    }
    let [prefer, fixed] = ["prefer", "fixed"].map(|field| {
        settings.get(field).and_then(|node| {
            Slot::read(
                node,
                Want::PortNumber,
                Destination::Readable("a port number"),
                d,
            )
        })
    });
    if cx.d.len() != errors_before {
        return None;
    }
    Some(match (prefer, fixed) {
        (Some(number), _) => PortSettings::Prefer(number),
        (None, Some(number)) => PortSettings::Fixed(number),
        (None, None) => PortSettings::Any,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::{self, check};

    #[test]
    fn at_most_one_of_prefer_and_fixed_and_each_a_port_number() {
        let (description, errors) = check("port", "prefer: 5173\n");
        assert!(errors.is_empty());
        assert_eq!(description.unwrap().gives[0].name.name, "number");
        let (_, errors) = check("port", "prefer: 1\nfixed: 2\n");
        assert_eq!(
            errors,
            ["2:1: a `port` unit has at most one of `prefer` and `fixed`; this one has both"]
        );
        let (_, errors) = check("port", "fixed: 0\n");
        assert_eq!(
            errors,
            ["1:8: this must be a port number from 1 to 65535; this is 0"]
        );
    }

    #[test]
    fn a_port_is_any_number_preferred_or_fixed() {
        let read = |with: &str| testing::read(with, |node, cx| read(Some(node), cx)).0;
        assert!(matches!(
            read("prefer: 5173\n"),
            Some(PortSettings::Prefer(_))
        ));
        assert!(matches!(read("fixed: 443\n"), Some(PortSettings::Fixed(_))));
        assert!(matches!(read("{}\n"), Some(PortSettings::Any)));
    }
}

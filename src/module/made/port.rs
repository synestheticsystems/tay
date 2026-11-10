//! `port`: a host port held for this environment.

use crate::syntax::fields::Fields;
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;

use crate::module::shape::{Given, GivenValue, Place, RestartDefault, Slot, UnitKind, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Port;

impl Module for Port {
    fn name(&self) -> &'static str {
        "port"
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
        let errors_before = cx.d.len();
        let mut slots = Vec::new();
        if let Some(with) = with {
            let settings = Fields::of(
                with,
                "the settings of a `port` unit",
                &["prefer", "fixed"],
                cx.d,
            )?;
            if let (Some(_), Some((key, _))) = (settings.entry("prefer"), settings.entry("fixed")) {
                cx.d.error(
                    key.loc,
                    "a `port` unit has at most one of `prefer` and `fixed`; this one has both",
                );
            }
            for field in ["prefer", "fixed"] {
                if let Some(node) = settings.get(field) {
                    slots.extend(Slot::read(
                        node,
                        Want::PortNumber,
                        Place::Readable("a port number"),
                        cx.d,
                    ));
                }
            }
        }
        (cx.d.len() == errors_before).then(|| UnitShape {
            kind: UnitKind::Made,
            slots,
            gives: vec![Given {
                name: NameAt {
                    name: "number".to_string(),
                    loc: cx.uses,
                },
                value: GivenValue::Port,
            }],
            outputs: Vec::new(),
            production: Vec::new(),
            ready_without_check: "when it is held",
            restart_by_default: RestartDefault::Never,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::module::testing::check;

    #[test]
    fn at_most_one_of_prefer_and_fixed_and_each_a_port_number() {
        let (shape, errors) = check("port", "prefer: 5173\n");
        assert!(errors.is_empty());
        assert_eq!(shape.unwrap().gives[0].name.name, "number");
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
}

//! The action `tay run` runs, and the arguments it is run with: it must be
//! a unit of the stack with `action: true`, each argument one it declares,
//! given once and of the type of its default, or text when it has none; and
//! each it declares with no default must be given. It comes into the plan
//! beside what `up` brings, with each `${args.NAME}` what it was given.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::definition::Stack;
use crate::diagnostic::source::{Loc, Sources};
use crate::diagnostic::{Diagnostics, quoted_list};
use crate::syntax::expr::{Literal, Type};
use crate::syntax::names::NameAt;

/// An action as `tay run` asks for it: its name, and each argument as
/// `NAME=VALUE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRun {
    pub action: String,
    pub args: Vec<String>,
}

/// The arguments of the action that is run, each as it was given or as its
/// default is.
pub struct Arguments {
    /// The action, by its index among the stack's units.
    pub unit: usize,
    pub values: HashMap<String, Literal>,
}

/// The action `run` asks for, where it is named, and its arguments; `None`
/// when either does not fit the stack, as reported.
pub fn asked(
    stack: &Stack,
    run: &ActionRun,
    sources: &mut Sources,
    d: &mut Diagnostics,
) -> Option<(NameAt, Arguments)> {
    let action = &run.action;
    let at = Loc::new(sources.add_argument(&format!("tay run {action}")), 1, 1);
    let Some(unit) = stack
        .units
        .iter()
        .position(|unit| unit.name.name == *action)
    else {
        d.error(at, format!("there is no unit `{action}`"));
        return None;
    };
    let Some(declared) = &stack.units[unit].action else {
        d.error(
            at,
            format!("`{action}` is not an action: `tay run` runs a unit that has `action: true`"),
        );
        return None;
    };
    let errors_before = d.len();
    let mut values = HashMap::new();
    for given in &run.args {
        let (name, text) = given.split_once('=').unwrap_or((given, ""));
        let at = Loc::new(
            sources.add_argument(&format!("tay run {action} {name}")),
            1,
            1,
        );
        let Some(arg) = declared.args.iter().find(|arg| arg.name.name == name) else {
            let names: Vec<&str> = (declared.args.iter())
                .map(|arg| arg.name.name.as_str())
                .collect();
            let declares = match names.as_slice() {
                [] => "it declares none".to_string(),
                names => format!("it declares {}", quoted_list(names)),
            };
            d.error(
                at,
                format!("`{action}` declares no argument `{name}`: {declares}"),
            );
            continue;
        };
        if !given.contains('=') {
            d.error(at, "an argument is given as NAME=VALUE");
            continue;
        }
        let ty = arg.ty().unwrap_or(Type::Text);
        let Some(value) = Literal::of_argument(text, ty) else {
            d.error(
                at,
                format!(
                    "`{name}` is {}, as its default is; `{text}` is not",
                    ty.describe()
                ),
            );
            continue;
        };
        if values.insert(name.to_string(), value).is_some() {
            d.error(at, format!("`{name}` is given twice"));
        }
    }
    for arg in &declared.args {
        let name = &arg.name.name;
        match &arg.default {
            _ if values.contains_key(name) => {}
            Some(default) => {
                values.insert(name.clone(), default.clone());
            }
            None => d.error(
                at,
                format!("`{action}` must be given the argument `{name}`, which has no default"),
            ),
        }
    }
    (d.len() == errors_before).then(|| {
        let named = NameAt {
            name: action.clone(),
            loc: at,
        };
        (named, Arguments { unit, values })
    })
}

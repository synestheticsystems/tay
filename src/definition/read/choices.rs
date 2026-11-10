//! Choices, written in the same shape in each place: a profile, the personal
//! file, the environment's `.tay/choices.yaml`, and `--set`.

use crate::definition::{Choices, Profile, SettingValue};
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::{Loc, Sources};
use crate::syntax::expr::{self, Expr};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

const CHOICES: &[&str] = &["up", "up_add", "up_remove", "run", "settings"];
const PROFILE: &[&str] = &["production", "up", "up_add", "up_remove", "run", "settings"];

pub fn profile(name: NameAt, node: &Node, d: &mut Diagnostics) -> Option<Profile> {
    let file = Fields::of(node, "a profile", PROFILE, d)?;
    let production = file
        .get("production")
        .and_then(|node| fields::boolean(node, "`production`", d)?.then_some(node.loc));
    Some(Profile {
        name,
        production,
        choices: choices(&file, d),
    })
}

/// The personal file or the environment's file. Only a profile can say
/// whether it is a production one.
pub fn choices_file(node: &Node, d: &mut Diagnostics) -> Option<Choices> {
    let file = Fields::of(node, "a choices file", CHOICES, d)?;
    Some(choices(&file, d))
}

fn choices(file: &Fields<'_>, d: &mut Diagnostics) -> Choices {
    let names = |key: &str, d: &mut Diagnostics| {
        file.get(key)
            .map(|node| fields::name_list(node, "a unit to bring up", names::name_problem, d))
    };
    let mut choices = Choices {
        up: names("up", d),
        up_add: names("up_add", d).unwrap_or_default(),
        up_remove: names("up_remove", d).unwrap_or_default(),
        ..Choices::default()
    };
    if let Some(run) = file
        .get("run")
        .and_then(|node| Fields::of(node, "`run`", &["default", "units"], d))
    {
        choices.run_default = run
            .get("default")
            .and_then(|node| fields::name(node, "a way", d));
        if let Some(units) = run.get("units") {
            for (unit, way) in fields::keyed(units, "a unit", names::name_problem, d) {
                if let Some(way) = fields::name(way, "a way", d) {
                    choices.run_units.push((unit, way));
                }
            }
        }
    }
    if let Some(settings) = file.get("settings") {
        for (name, value) in fields::keyed(settings, "a setting", names::setting_name_problem, d) {
            if let Some(value) = expr::from_node(value, d) {
                choices.settings.push((name, SettingValue::Written(value)));
            }
        }
    }
    choices
}

/// One `--set KEY=VALUE`, as a set of choices with that one choice in it. The
/// key is a path in the shape the files use: `up`, `up_add`, `up_remove`,
/// `run.default`, `run.units.UNIT`, `settings.NAME`; lists are written with
/// commas.
pub fn assignment(argument: &str, sources: &mut Sources, d: &mut Diagnostics) -> Option<Choices> {
    let (key, value) = argument.split_once('=').unwrap_or((argument, ""));
    let loc = Loc::new(sources.add_argument(&format!("--set {key}")), 1, 1);
    if !argument.contains('=') {
        d.error(loc, "`--set` is written KEY=VALUE");
        return None;
    }
    let name = |text: &str, rule: fn(&str) -> Option<String>, d: &mut Diagnostics| match rule(text)
    {
        None => Some(NameAt {
            name: text.to_string(),
            loc,
        }),
        Some(problem) => {
            d.error(loc, problem);
            None
        }
    };
    let list = |d: &mut Diagnostics| {
        value
            .split(',')
            .filter(|item| !item.is_empty())
            .map(|item| name(item, names::name_problem, d))
            .collect::<Option<Vec<_>>>()
    };
    let mut choices = Choices::default();
    match key {
        "up" => choices.up = Some(list(d)?),
        "up_add" => choices.up_add = list(d)?,
        "up_remove" => choices.up_remove = list(d)?,
        "run.default" => choices.run_default = Some(name(value, names::name_problem, d)?),
        _ => {
            if let Some(unit) = key.strip_prefix("run.units.") {
                choices.run_units.push((
                    name(unit, names::name_problem, d)?,
                    name(value, names::name_problem, d)?,
                ));
            } else if let Some(setting) = key.strip_prefix("settings.") {
                let setting = name(setting, names::setting_name_problem, d)?;
                // The error does not quote the value: it may be a secret.
                let Ok(kind) = expr::parse_string(value, loc, None) else {
                    d.error(
                        loc,
                        "the value holds a `${` that is not a reference tay can read (write `$${` for a literal `${`)",
                    );
                    return None;
                };
                let value = Expr { loc, kind };
                choices
                    .settings
                    .push((setting, SettingValue::Argument(value)));
            } else {
                d.error(
                    loc,
                    "`--set` sets `up`, `up_add`, `up_remove`, `run.default`, `run.units.UNIT` or `settings.NAME`",
                );
                return None;
            }
        }
    }
    Some(choices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(argument: &str) -> (Option<Choices>, Vec<String>) {
        let mut sources = Sources::default();
        let mut d = Diagnostics::default();
        let choices = assignment(argument, &mut sources, &mut d);
        let errors = d.into_sorted().iter().map(|e| e.render(&sources)).collect();
        (choices, errors)
    }

    #[test]
    fn set_writes_one_choice_in_the_shape_of_the_files() {
        let up = set("up=core,tools").0.unwrap().up.unwrap();
        assert_eq!(
            up.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["core", "tools"]
        );
        assert_eq!(set("up=").0.unwrap().up.unwrap().len(), 0);
        let run = set("run.units.server=image").0.unwrap().run_units;
        assert_eq!(
            (run[0].0.name.as_str(), run[0].1.name.as_str()),
            ("server", "image")
        );
        assert_eq!(
            set("run.default=source")
                .0
                .unwrap()
                .run_default
                .unwrap()
                .name,
            "source"
        );
        let settings = set("settings.log.web=debug").0.unwrap().settings;
        assert_eq!(settings[0].0.name, "log.web");
    }

    #[test]
    fn a_bad_set_names_its_key_and_never_its_value() {
        assert_eq!(
            set("colour=red").1,
            [
                "--set colour: `--set` sets `up`, `up_add`, `up_remove`, `run.default`, `run.units.UNIT` or `settings.NAME`"
            ]
        );
        assert_eq!(set("up").1, ["--set up: `--set` is written KEY=VALUE"]);
        let (_, errors) = set("settings.key=${unclosed");
        assert_eq!(errors.len(), 1);
        assert!(!errors[0].contains("unclosed"), "{errors:?}");
    }
}

//! Choices, written in the same shape in each place: the stack's defaults in
//! `tay.yaml`, a profile, the personal file, the environment's
//! `.tay/choices.yaml`, and `--set`.

use crate::definition::{Choices, Profile, SettingValue, Written};
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::{Loc, Sources};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

const PROFILE: &[&str] = &[
    "production",
    "require",
    "up",
    "up_add",
    "up_remove",
    "run",
    "settings",
    "label",
];

/// What only a profile may say.
const PROFILE_ONLY: &[&str] = &["production", "require"];

pub fn profile(name: NameAt, node: &Node, d: &mut Diagnostics) -> Option<Profile> {
    let file = Fields::of(node, "a profile", PROFILE, d)?;
    let production = file
        .get("production")
        .and_then(|node| fields::boolean(node, "`production`", d)?.then_some(node.loc));
    let require = file
        .get("require")
        .map(|node| fields::name_list(node, "a required setting", names::setting_name_problem, d))
        .unwrap_or_default();
    Some(Profile {
        name,
        production,
        require,
        choices: choices(&file, d),
    })
}

/// The personal file or the environment's file. They have a profile's
/// shape, but only a profile says whether it is a production one and what
/// it requires.
pub fn choices_file(node: &Node, d: &mut Diagnostics) -> Option<Choices> {
    let file = Fields::of(node, "a choices file", PROFILE, d)?;
    for (field, key) in file.present() {
        if PROFILE_ONLY.contains(&field) {
            d.error(
                key.loc,
                format!("`{field}` may be written only in a profile"),
            );
        }
    }
    Some(choices(&file, d))
}

/// The stack's own defaults: `up` and `run` in `tay.yaml`.
pub fn stack_defaults(file: &Fields<'_>, d: &mut Diagnostics) -> Choices {
    let mut choices = Choices {
        up: file.get("up").map(|node| up_list(node, d)),
        ..Choices::default()
    };
    if let Some(run) = file.get("run") {
        read_run(run, &mut choices, d);
    }
    choices
}

fn choices(file: &Fields<'_>, d: &mut Diagnostics) -> Choices {
    let mut choices = Choices {
        up: file.get("up").map(|node| up_list(node, d)),
        up_add: file
            .get("up_add")
            .map(|node| up_list(node, d))
            .unwrap_or_default(),
        up_remove: file
            .get("up_remove")
            .map(|node| up_list(node, d))
            .unwrap_or_default(),
        ..Choices::default()
    };
    if let Some(run) = file.get("run") {
        read_run(run, &mut choices, d);
    }
    if let Some(settings) = file.get("settings") {
        for (name, value) in fields::keyed(settings, "a setting", names::setting_name_problem, d) {
            if let Some(literal) = fields::literal(value, "a setting's value", d) {
                let written = Written {
                    value: literal,
                    loc: value.loc,
                };
                choices
                    .settings
                    .push((name, SettingValue::Written(written)));
            }
        }
    }
    if let Some(node) = file.get("label") {
        choices.label = label(node, d);
    }
    choices
}

fn up_list(node: &Node, d: &mut Diagnostics) -> Vec<NameAt> {
    fields::name_list(node, "a unit to bring up", names::name_problem, d)
}

/// `run`: `default`, one way's name or a list of them, and `units`, a way
/// for each unit named.
fn read_run(node: &Node, choices: &mut Choices, d: &mut Diagnostics) {
    let Some(run) = Fields::of(node, "`run`", &["default", "units"], d) else {
        return;
    };
    choices.run_default = run.get("default").map(|node| match node.as_seq() {
        Some(_) => fields::name_list(node, "a way", names::name_problem, d),
        None => fields::name(node, "a way", d).into_iter().collect(),
    });
    if let Some(units) = run.get("units") {
        for (unit, way) in fields::keyed(units, "a unit", names::name_problem, d) {
            if let Some(way) = fields::name(way, "a way", d) {
                choices.run_units.push((unit, way));
            }
        }
    }
}

fn label(node: &Node, d: &mut Diagnostics) -> Option<String> {
    let text = fields::text(node, "`label`", d)?;
    if let Some(problem) = names::label_problem(text) {
        d.error(node.loc, problem);
        return None;
    }
    Some(text.to_string())
}

/// One `--set PLACE=VALUE`, as a set of choices with that one choice in it.
/// The place is a path in the shape the files use: `up`, `up_add`,
/// `up_remove`, `run.default`, `run.units.UNIT`, `settings.NAME` or
/// `label`; lists are written with commas.
pub fn assignment(argument: &str, sources: &mut Sources, d: &mut Diagnostics) -> Option<Choices> {
    let (place, value) = argument.split_once('=').unwrap_or((argument, ""));
    let loc = Loc::new(sources.add_argument(&format!("--set {place}")), 1, 1);
    if !argument.contains('=') {
        d.error(loc, "`--set` is written PLACE=VALUE");
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
    match place {
        "up" => choices.up = Some(list(d)?),
        "up_add" => choices.up_add = list(d)?,
        "up_remove" => choices.up_remove = list(d)?,
        "run.default" => choices.run_default = Some(list(d)?),
        "label" => {
            if let Some(problem) = names::label_problem(value) {
                d.error(loc, problem);
                return None;
            }
            choices.label = Some(value.to_string());
        }
        _ => {
            if let Some(unit) = place.strip_prefix("run.units.") {
                choices.run_units.push((
                    name(unit, names::name_problem, d)?,
                    name(value, names::name_problem, d)?,
                ));
            } else if let Some(setting) = place.strip_prefix("settings.") {
                let setting = name(setting, names::setting_name_problem, d)?;
                let value = SettingValue::Argument {
                    text: value.to_string(),
                    loc,
                };
                choices.settings.push((setting, value));
            } else {
                d.error(
                    loc,
                    "`--set` sets `up`, `up_add`, `up_remove`, `run.default`, `run.units.UNIT`, `settings.NAME` or `label`",
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

    fn names(list: &[NameAt]) -> Vec<&str> {
        list.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn set_writes_one_choice_in_the_shape_of_the_files() {
        let up = set("up=core,tools").0.unwrap().up.unwrap();
        assert_eq!(names(&up), ["core", "tools"]);
        assert_eq!(set("up=").0.unwrap().up.unwrap().len(), 0);
        let run = set("run.units.server=image").0.unwrap().run_units;
        assert_eq!(
            (run[0].0.name.as_str(), run[0].1.name.as_str()),
            ("server", "image")
        );
        let default = set("run.default=source-in-box,source")
            .0
            .unwrap()
            .run_default;
        assert_eq!(names(&default.unwrap()), ["source-in-box", "source"]);
        let settings = set("settings.log.web=debug").0.unwrap().settings;
        assert_eq!(settings[0].0.name, "log.web");
        assert_eq!(
            set("label=fix-login").0.unwrap().label.unwrap(),
            "fix-login"
        );
        assert_eq!(set("label=").0.unwrap().label.unwrap(), "");
    }

    #[test]
    fn a_bad_set_names_its_place_and_never_its_value() {
        assert_eq!(
            set("colour=red").1,
            [
                "--set colour: `--set` sets `up`, `up_add`, `up_remove`, `run.default`, `run.units.UNIT`, `settings.NAME` or `label`"
            ]
        );
        assert_eq!(set("up").1, ["--set up: `--set` is written PLACE=VALUE"]);
        assert_eq!(
            set("label=Fix_Login").1,
            [
                "--set label: `Fix_Login` is not a label: lower-case letters, digits and hyphens, neither starting nor ending with a hyphen, at most 63 characters"
            ]
        );
    }

    #[test]
    fn a_set_value_is_literal_text_even_with_a_reference_in_it() {
        let (choices, errors) = set("settings.key=${unclosed");
        assert!(errors.is_empty(), "{errors:?}");
        let settings = choices.unwrap().settings;
        assert!(
            matches!(&settings[0].1, SettingValue::Argument { text, .. } if text == "${unclosed")
        );
    }
}

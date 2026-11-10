//! Choices from every place, layered; what they must give; and the way
//! each unit runs.

use std::collections::BTreeMap;

use crate::definition::{Choices, Profile, SettingValue, Stack, Unit};
use crate::diagnostic::source::Loc;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::names::NameAt;

/// The choices in force once every place has had its say.
#[derive(Debug, Default)]
pub struct Layered {
    pub up: Vec<NameAt>,
    /// Whether some place gives `up` at all.
    pub up_given: bool,
    pub run_default: Vec<NameAt>,
    /// By unit name: the unit as named, and the way chosen.
    pub run_units: BTreeMap<String, (NameAt, NameAt)>,
    pub settings: BTreeMap<String, SettingValue>,
    pub label: Option<String>,
}

/// Layers `places`, later ones winning. A later `up` replaces the list, and
/// `up_add` and `up_remove` adjust it; `run.default` and `label` are
/// replaced; `run.units` and `settings` are joined, later entries winning.
pub fn layer<'c>(places: impl IntoIterator<Item = &'c Choices>) -> Layered {
    let mut layered = Layered::default();
    for place in places {
        if let Some(up) = &place.up {
            layered.up.clear();
            layered.up_given = true;
            add(&mut layered.up, up);
        }
        add(&mut layered.up, &place.up_add);
        layered
            .up
            .retain(|name| !place.up_remove.iter().any(|r| r.name == name.name));
        if let Some(default) = &place.run_default {
            layered.run_default = default.clone();
        }
        for (unit, way) in &place.run_units {
            layered
                .run_units
                .insert(unit.name.clone(), (unit.clone(), way.clone()));
        }
        for (name, value) in &place.settings {
            layered.settings.insert(name.name.clone(), value.clone());
        }
        if let Some(label) = &place.label {
            layered.label = Some(label.clone());
        }
    }
    layered
}

fn add(list: &mut Vec<NameAt>, names: &[NameAt]) {
    for name in names {
        if !list.iter().any(|n| n.name == name.name) {
            list.push(name.clone());
        }
    }
}

/// Some place must say what comes up: tay does not guess what a stack's
/// whole is.
pub fn check_up_given(stack: &Stack, layered: &Layered, d: &mut Diagnostics) {
    if !layered.up_given {
        d.error(
            stack.loc,
            "nothing says what comes up: none of tay.yaml, the profile, the personal file, the environment's file and `--set` gives `up`",
        );
    }
}

/// Each setting the profile requires must be given a value that is not
/// empty by one of the choices the format counts for it: the profile, the
/// environment's file or a `--set`, the last of these that gives one. The
/// stack's default is not among them, nor is the personal file.
pub fn check_required(
    stack: &Stack,
    profile: &Profile,
    counting: &[&Choices],
    d: &mut Diagnostics,
) {
    for required in &profile.require {
        if stack.setting(&required.name).is_none() {
            d.error(
                required.loc,
                format!(
                    "`require` names setting `{}`, which is not declared in tay.yaml",
                    required.name
                ),
            );
            continue;
        }
        let given = counting.iter().rev().find_map(|place| {
            place
                .settings
                .iter()
                .rev()
                .find(|(name, _)| name.name == required.name)
                .map(|(_, value)| value)
        });
        match given {
            None => d.error(
                required.loc,
                format!(
                    "the profile `{}` requires setting `{}`, and no choice gives it a value: the profile, the environment's file or a `--set` must (its default does not count)",
                    profile.name.name, required.name
                ),
            ),
            Some(value) if value.is_empty() => d.push(
                Diagnostic::new(
                    value.loc(),
                    format!(
                        "the profile `{}` requires setting `{}`, and the value given here is empty",
                        profile.name.name, required.name
                    ),
                )
                .with(required.loc, "it is required here"),
            ),
            Some(_) => {}
        }
    }
}

/// The way `unit` runs: `run.units.<unit>` if given, otherwise the first
/// name in `run.default` that is one of its ways. Gives the way's index, and
/// where the choice was made if the unit has ways.
pub fn choose(unit: &Unit, layered: &Layered) -> Result<(usize, Option<Loc>), Diagnostic> {
    if !unit.has_ways() {
        return Ok((0, None));
    }
    if let Some((_, way)) = layered.run_units.get(&unit.name.name)
        && let Some(index) = unit.way(&way.name)
    {
        return Ok((index, Some(way.loc)));
    }
    if let Some((index, loc)) = layered
        .run_default
        .iter()
        .find_map(|default| Some((unit.way(&default.name)?, default.loc)))
    {
        return Ok((index, Some(loc)));
    }
    let quoted = |names: Vec<&str>| {
        names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let why = match layered.run_default.as_slice() {
        [] => "no `run.default` is chosen".to_string(),
        defaults => format!(
            "`run.default` ({}) names none of them",
            quoted(defaults.iter().map(|d| d.name.as_str()).collect())
        ),
    };
    let error = Diagnostic::new(
        unit.name.loc,
        format!(
            "unit `{}` has the ways {}, and no choice names one: `run.units` does not name it, and {why}",
            unit.name.name,
            quoted(unit.way_names())
        ),
    );
    Err(match layered.run_default.first() {
        Some(default) => error.with(default.loc, "`run.default` is chosen here"),
        None => error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::source::{Loc, Sources};

    fn names(list: &[&str]) -> Vec<NameAt> {
        let mut sources = Sources::default();
        let loc = Loc::new(sources.add_argument("test"), 1, 1);
        list.iter()
            .map(|name| NameAt {
                name: (*name).to_string(),
                loc,
            })
            .collect()
    }

    fn up(layered: &Layered) -> Vec<&str> {
        layered.up.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn a_later_up_replaces_and_up_add_and_up_remove_adjust() {
        let profile = Choices {
            up: Some(names(&["core", "tools"])),
            ..Choices::default()
        };
        let personal = Choices {
            up_add: names(&["docs", "core"]),
            up_remove: names(&["tools"]),
            ..Choices::default()
        };
        assert_eq!(up(&layer([&profile, &personal])), ["core", "docs"]);
        let environment = Choices {
            up: Some(names(&["web"])),
            ..Choices::default()
        };
        assert_eq!(up(&layer([&profile, &personal, &environment])), ["web"]);
    }

    #[test]
    fn run_default_is_replaced_and_run_units_joined() {
        let pair = |unit: &str, way: &str| (names(&[unit])[0].clone(), names(&[way])[0].clone());
        let profile = Choices {
            run_default: Some(names(&["source"])),
            run_units: vec![pair("server", "image"), pair("proxy", "image")],
            ..Choices::default()
        };
        let set = Choices {
            run_default: Some(names(&["image", "source"])),
            run_units: vec![pair("proxy", "source")],
            ..Choices::default()
        };
        let layered = layer([&profile, &set]);
        let defaults: Vec<&str> = layered
            .run_default
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(defaults, ["image", "source"]);
        assert_eq!(layered.run_units["server"].1.name, "image");
        assert_eq!(layered.run_units["proxy"].1.name, "source");
    }
}

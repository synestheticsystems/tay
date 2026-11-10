//! Choices from every place, layered, and the alternative each unit runs as.

use std::collections::BTreeMap;

use crate::definition::{Choices, SettingValue, Unit};
use crate::diagnostic::Diagnostic;
use crate::names::NameAt;
use crate::source::Loc;

/// The choices in force once every place has had its say.
#[derive(Debug, Default)]
pub struct Layered {
    pub up: Vec<NameAt>,
    pub run_default: Option<NameAt>,
    /// By unit name: the unit as named, and the alternative chosen.
    pub run_units: BTreeMap<String, (NameAt, NameAt)>,
    pub settings: BTreeMap<String, SettingValue>,
}

/// Layers `places`, later ones winning. A later `up` replaces the list, and
/// `up_add` and `up_remove` adjust it; `run.default` is replaced; `run.units`
/// and `settings` are joined, later entries winning.
pub fn layer<'c>(places: impl IntoIterator<Item = &'c Choices>) -> Layered {
    let mut layered = Layered::default();
    for place in places {
        if let Some(up) = &place.up {
            layered.up.clear();
            add(&mut layered.up, up);
        }
        add(&mut layered.up, &place.up_add);
        layered
            .up
            .retain(|name| !place.up_remove.iter().any(|r| r.name == name.name));
        if let Some(default) = &place.run_default {
            layered.run_default = Some(default.clone());
        }
        for (unit, alternative) in &place.run_units {
            layered
                .run_units
                .insert(unit.name.clone(), (unit.clone(), alternative.clone()));
        }
        for (name, value) in &place.settings {
            layered.settings.insert(name.name.clone(), value.clone());
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

/// The alternative `unit` runs as: `run.units.<unit>` if given, otherwise
/// `run.default` if the unit has an alternative of that name. Gives the
/// variant and where the choice was made, if the unit has alternatives.
pub fn choose(unit: &Unit, layered: &Layered) -> Result<(usize, Option<Loc>), Diagnostic> {
    if !unit.has_alternatives() {
        return Ok((0, None));
    }
    if let Some((_, alternative)) = layered.run_units.get(&unit.name.name)
        && let Some(index) = unit.alternative(&alternative.name)
    {
        return Ok((index, Some(alternative.loc)));
    }
    if let Some(default) = &layered.run_default
        && let Some(index) = unit.alternative(&default.name)
    {
        return Ok((index, Some(default.loc)));
    }
    let alternatives = unit
        .alternative_names()
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let why = match &layered.run_default {
        Some(default) => format!(
            "`run.default` is `{}`, which it does not have",
            default.name
        ),
        None => "no `run.default` is chosen".to_string(),
    };
    let error = Diagnostic::new(
        unit.name.loc,
        format!(
            "unit `{}` has the alternatives {alternatives}, and no choice names one: `run.units` does not name it, and {why}",
            unit.name.name
        ),
    );
    Err(match &layered.run_default {
        Some(default) => error.with(default.loc, "`run.default` is chosen here"),
        None => error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Loc, Sources};

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
        let pair = |unit: &str, alt: &str| (names(&[unit])[0].clone(), names(&[alt])[0].clone());
        let profile = Choices {
            run_default: names(&["source"]).pop(),
            run_units: vec![pair("server", "image"), pair("proxy", "image")],
            ..Choices::default()
        };
        let set = Choices {
            run_default: names(&["image"]).pop(),
            run_units: vec![pair("proxy", "source")],
            ..Choices::default()
        };
        let layered = layer([&profile, &set]);
        assert_eq!(layered.run_default.unwrap().name, "image");
        assert_eq!(layered.run_units["server"].1.name, "image");
        assert_eq!(layered.run_units["proxy"].1.name, "source");
    }
}

//! What may stand between `${` and `}`.

use crate::names;
use crate::source::Loc;

/// One `${...}`, where it is written and what it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub loc: Loc,
    /// The text between `${` and `}`, as written, for messages.
    pub inner: String,
    pub target: Target,
}

impl Reference {
    /// The reference as it is written in a definition.
    pub fn written(&self) -> String {
        format!("${{{}}}", self.inner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `${unit.name}`, `${unit.name.part}`, `${unit.name.part@view}`.
    Value {
        unit: UnitRef,
        name: String,
        part: Option<Part>,
        view: Option<View>,
    },
    /// `${unit.outputs.name}`: a file a unit builds.
    Output {
        unit: UnitRef,
        name: String,
    },
    Setting(String),
    Environment(EnvField),
    HostListen,
    Param(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UnitRef {
    /// `self`: the unit the reference is written in.
    This,
    Named(String),
}

/// The part of an endpoint a reference asks for. With none, it asks for `host:port`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Url,
    Host,
    Port,
}

/// `@host` or `@net`: the view of an endpoint asked for whoever uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Host,
    Net,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvField {
    Name,
    Id,
    Label,
    Root,
    State,
    Profile,
}

impl EnvField {
    const ALL: [(&'static str, EnvField); 6] = [
        ("name", EnvField::Name),
        ("id", EnvField::Id),
        ("label", EnvField::Label),
        ("root", EnvField::Root),
        ("state", EnvField::State),
        ("profile", EnvField::Profile),
    ];
}

/// Reads the text between `${` and `}`. The error says what is wrong with it.
pub fn parse_target(inner: &str) -> Result<Target, String> {
    let written = format!("${{{inner}}}");
    if inner.is_empty() {
        return Err("`${}` names nothing".to_string());
    }
    let (path, view) = match inner.split_once('@') {
        None => (inner, None),
        Some((path, view)) => match view {
            "host" => (path, Some(View::Host)),
            "net" => (path, Some(View::Net)),
            _ => {
                return Err(format!(
                    "`{written}` asks for the view `@{view}`; the views are `@host` and `@net`"
                ));
            }
        },
    };
    let parts: Vec<&str> = path.split('.').collect();
    let no_view = |target: Target| match view {
        None => Ok(target),
        Some(_) => Err(format!(
            "`{written}`: only a unit's value can be asked for with `@host` or `@net`"
        )),
    };
    match parts.as_slice() {
        ["settings", rest @ ..] => {
            let name = rest.join(".");
            if name.is_empty() {
                return Err(format!("`{written}` names no setting"));
            }
            if let Some(problem) = names::setting_name_problem(&name) {
                return Err(format!("`{written}`: {problem}"));
            }
            no_view(Target::Setting(name))
        }
        ["environment", field] => match EnvField::ALL.iter().find(|(n, _)| n == field) {
            Some((_, field)) => no_view(Target::Environment(*field)),
            None => Err(format!(
                "`{written}`: the environment has `name`, `id`, `label`, `root`, `state` and `profile`"
            )),
        },
        ["environment", ..] => Err(format!(
            "`{written}`: the environment has `name`, `id`, `label`, `root`, `state` and `profile`"
        )),
        ["host", "listen"] => no_view(Target::HostListen),
        ["host", ..] => Err(format!("`{written}`: the host has only `listen`")),
        ["params", name] => match names::value_name_problem(name) {
            None => no_view(Target::Param((*name).to_string())),
            Some(problem) => Err(format!("`{written}`: {problem}")),
        },
        ["params", ..] => Err(format!(
            "`{written}` must name one parameter: `${{params.name}}`"
        )),
        [unit, rest @ ..] => {
            let unit = if *unit == "self" {
                UnitRef::This
            } else if let Some(problem) = names::name_problem(unit) {
                return Err(format!(
                    "`{written}` refers to nothing tay knows: {problem}"
                ));
            } else {
                UnitRef::Named((*unit).to_string())
            };
            match rest {
                [] => Err(format!(
                    "`{written}` names a unit but not one of its values, as in `${{unit.value}}`"
                )),
                ["outputs", name] => no_view(Target::Output {
                    unit,
                    name: (*name).to_string(),
                }),
                ["outputs", ..] => Err(format!(
                    "`{written}` must name one output: `${{unit.outputs.name}}`"
                )),
                [name, part @ ..] => {
                    let part = match part {
                        [] => None,
                        ["url"] => Some(Part::Url),
                        ["host"] => Some(Part::Host),
                        ["port"] => Some(Part::Port),
                        _ => {
                            return Err(format!(
                                "`{written}`: after a value's name may come `.url`, `.host` or `.port`"
                            ));
                        }
                    };
                    Ok(Target::Value {
                        unit,
                        name: (*name).to_string(),
                        part,
                        view,
                    })
                }
            }
        }
        [] => Err("`${}` names nothing".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(unit: &str) -> UnitRef {
        UnitRef::Named(unit.to_string())
    }

    #[test]
    fn the_forms_of_reference() {
        assert_eq!(
            parse_target("web-port.number"),
            Ok(Target::Value {
                unit: named("web-port"),
                name: "number".into(),
                part: None,
                view: None
            })
        );
        assert_eq!(
            parse_target("api.http.url@net"),
            Ok(Target::Value {
                unit: named("api"),
                name: "http".into(),
                part: Some(Part::Url),
                view: Some(View::Net)
            })
        );
        assert_eq!(
            parse_target("self.site.url"),
            Ok(Target::Value {
                unit: UnitRef::This,
                name: "site".into(),
                part: Some(Part::Url),
                view: None
            })
        );
        assert_eq!(
            parse_target("build.outputs.server"),
            Ok(Target::Output {
                unit: named("build"),
                name: "server".into()
            })
        );
        assert_eq!(
            parse_target("settings.images.server"),
            Ok(Target::Setting("images.server".into()))
        );
        assert_eq!(
            parse_target("environment.label"),
            Ok(Target::Environment(EnvField::Label))
        );
        assert_eq!(parse_target("host.listen"), Ok(Target::HostListen));
        assert_eq!(
            parse_target("params.name"),
            Ok(Target::Param("name".into()))
        );
    }

    #[test]
    fn malformed_references_say_why() {
        for bad in [
            "",
            "web",
            "Web.x",
            "api.http.scheme",
            "api.http@there",
            "settings.x@host",
            "environment.home",
            "host.name",
            "params.a.b",
            "build.outputs",
        ] {
            assert!(parse_target(bad).is_err(), "{bad} should be refused");
        }
    }
}

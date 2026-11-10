//! `file`: a file tay writes from values, in the environment's state folder.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::{EnvField, Expr, ExprKind, Piece, Reference, Target};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;

use crate::module::shape::{
    Claim, Given, GivenValue, Place, RestartDefault, Slot, UnitKind, UnitShape,
};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct File;

const FIELDS: &[&str] = &["content", "template", "name", "folder", "mode", "overwrite"];

/// The folder under the state folder that file units write in.
const FILES: &str = "files";

impl Module for File {
    fn name(&self) -> &'static str {
        "file"
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
        let Some(with) = with else {
            cx.d.error(
                cx.uses,
                "a `file` unit needs `with` and in it a `content` or a `template`",
            );
            return None;
        };
        let errors_before = cx.d.len();
        let d = &mut *cx.d;
        let settings = Fields::of(with, "the settings of a `file` unit", FIELDS, d)?;
        let mut shape = UnitShape::new(
            UnitKind::Made,
            "when it has been written",
            RestartDefault::Never,
        );
        // A secret may go in the content: the file is then written for its
        // owner alone, unless a `mode` written here says otherwise.
        match (settings.entry("content"), settings.entry("template")) {
            (Some((_, content)), None) => {
                shape
                    .slots
                    .extend(Slot::read(content, Want::Scalar, Place::FileContent, d));
            }
            (None, Some((_, template))) => {
                shape.slots.extend(Slot::read(
                    template,
                    Want::TemplateFile(Place::FileContent),
                    Place::Readable("a file path"),
                    d,
                ));
            }
            (Some(_), Some((key, _))) => d.error(
                key.loc,
                "a `file` unit has exactly one of `content` and `template`; this one has both",
            ),
            (None, None) => d.error(
                settings.loc(),
                "a `file` unit needs exactly one of `content` and `template`",
            ),
        }
        if let Some(mode) = settings.get("mode") {
            shape.slots.extend(Slot::read(
                mode,
                Want::FileMode,
                Place::Readable("a file's mode"),
                d,
            ));
        }
        if let Some(overwrite) = settings.get("overwrite") {
            shape.slots.extend(Slot::read(
                overwrite,
                Want::Bool,
                Place::Readable("a file's settings"),
                d,
            ));
        }
        let name = settings
            .get("name")
            .and_then(|node| Slot::read(node, Want::FileName, Place::Readable("a file name"), d));
        let name_pieces = match &name {
            Some(slot) => match &slot.expr.kind {
                ExprKind::Text(pieces) => pieces.clone(),
                ExprKind::Ref(reference) => vec![Piece::Ref(reference.clone())],
                ExprKind::Number(n) => vec![Piece::Lit(n.to_string())],
                ExprKind::Bool(b) => vec![Piece::Lit(b.to_string())],
            },
            None => vec![Piece::Lit(cx.unit.to_string())],
        };
        shape.slots.extend(name);
        let folder = match settings.get("folder") {
            Some(node) => fields::name(node, "`folder`", d).map(|folder| folder.name),
            None => Some(cx.unit.to_string()),
        };
        if cx.d.len() != errors_before {
            return None;
        }
        let folder = folder?;
        // Files in one folder are compared by their names as written.
        let written_name: String = name_pieces
            .iter()
            .map(|piece| match piece {
                Piece::Lit(text) => text.clone(),
                Piece::Ref(reference) => reference.written(),
            })
            .collect();
        shape.claims.push(Claim {
            what: format!("the file `{FILES}/{folder}/{written_name}` in the state folder"),
            loc: settings.get("name").map_or(cx.uses, |node| node.loc),
        });
        let dir = vec![
            state_folder(cx.uses),
            Piece::Lit(format!("/{FILES}/{folder}")),
        ];
        let mut path = dir.clone();
        path.push(Piece::Lit("/".to_string()));
        path.extend(name_pieces);
        let given = |name: &str, pieces: Vec<Piece>| Given {
            name: NameAt {
                name: name.to_string(),
                loc: cx.uses,
            },
            value: GivenValue::Expr(Expr {
                loc: cx.uses,
                kind: ExprKind::Text(pieces),
            }),
        };
        shape.gives = vec![given("path", path), given("dir", dir)];
        Some(shape)
    }
}

/// `${environment.state}`, standing at the unit's `uses`, so that the path a
/// file unit gives is resolved like any value written in a definition.
fn state_folder(loc: Loc) -> Piece {
    Piece::Ref(Reference {
        loc,
        inner: "environment.state".to_string(),
        target: Target::Environment(EnvField::State),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::check;

    #[test]
    fn exactly_one_of_content_and_template_and_mode_is_text() {
        let (_, errors) = check("file", "name: a.conf\n");
        assert_eq!(
            errors,
            ["1:1: a `file` unit needs exactly one of `content` and `template`"]
        );
        let (_, errors) = check("file", "content: x\nmode: 0600\n");
        assert_eq!(errors, ["2:7: this must be text; this is a whole number"]);
        let (_, errors) = check("file", "content: x\nname: conf/a\n");
        assert_eq!(
            errors,
            ["2:7: this must be a file name with no folder in it; this is `conf/a`"]
        );
        let (_, errors) = check("file", "content: x\nfolder: Conf\noverwrite: no\n");
        assert_eq!(
            errors,
            [
                "2:9: `folder`: `Conf` must start with a lower-case letter",
                "3:12: this must be true or false; this is text",
            ]
        );
    }

    #[test]
    fn a_secret_may_go_in_the_content_whatever_mode_is_written() {
        for with in ["content: x\n", "content: x\nmode: \"0644\"\n"] {
            let shape = check("file", with).0.unwrap();
            let content = shape.slots.iter().find(|s| s.want == Want::Scalar).unwrap();
            assert_eq!(content.place, Place::FileContent, "{with}");
        }
    }

    #[test]
    fn it_gives_its_path_and_folder_under_files_in_the_state_folder() {
        let written = |with: &str| {
            let shape = check("file", with).0.unwrap();
            let names: Vec<_> = shape.gives.iter().map(|g| g.name.name.as_str()).collect();
            assert_eq!(names, ["path", "dir"]);
            let GivenValue::Expr(path) = &shape.gives[0].value else {
                panic!("the path is built from values");
            };
            let ExprKind::Text(pieces) = &path.kind else {
                panic!("the path is text");
            };
            (
                crate::syntax::expr::write_pieces(pieces).unwrap(),
                shape.claims[0].what.clone(),
            )
        };
        assert_eq!(
            written("content: x\nname: app.conf\n"),
            (
                "${environment.state}/files/the-unit/app.conf".to_string(),
                "the file `files/the-unit/app.conf` in the state folder".to_string()
            )
        );
        assert_eq!(
            written("content: x\nfolder: shared\n").0,
            "${environment.state}/files/shared/the-unit"
        );
    }
}

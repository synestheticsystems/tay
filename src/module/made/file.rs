//! `file`: a file tay writes from values, in the environment's state folder.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::{EnvField, Expr, ExprKind, Piece, Reference, Target};
use crate::syntax::fields::Fields;
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;

use crate::module::shape::{Given, GivenValue, Place, RestartDefault, Slot, UnitKind, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct File;

const FIELDS: &[&str] = &["content", "template", "name", "mode"];

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
        let mut slots = Vec::new();
        let mode = settings
            .get("mode")
            .and_then(|node| Slot::read(node, Want::FileMode, Place::Readable("a file's mode"), d));
        // A secret may go only into a file its owner alone can read. With no
        // mode written, the file is made that way when it holds a secret.
        let content_place = match &mode {
            None => Place::PrivateFile,
            Some(mode) if mode.expr.literal_text().as_deref() == Some("0600") => Place::PrivateFile,
            Some(_) => Place::Readable("a file whose mode is not \"0600\""),
        };
        slots.extend(mode);
        match (settings.entry("content"), settings.entry("template")) {
            (Some((_, content)), None) => {
                slots.extend(Slot::read(content, Want::Scalar, content_place, d));
            }
            (None, Some((_, template))) => {
                slots.extend(Slot::read(
                    template,
                    Want::TemplateFile(content_place),
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
        slots.extend(name);
        if cx.d.len() != errors_before {
            return None;
        }
        let dir = vec![state_folder(cx.uses), Piece::Lit(format!("/{}", cx.unit))];
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
        Some(UnitShape {
            kind: UnitKind::Made,
            slots,
            gives: vec![given("path", path), given("dir", dir)],
            outputs: Vec::new(),
            production: Vec::new(),
            ready_without_check: "when it has been written",
            restart_by_default: RestartDefault::Never,
        })
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
    }

    #[test]
    fn a_secret_may_go_in_only_when_the_mode_keeps_the_file_private() {
        let content = |with: &str| {
            let shape = check("file", with).0.unwrap();
            shape
                .slots
                .iter()
                .find(|s| s.want == Want::Scalar)
                .unwrap()
                .place
        };
        assert_eq!(content("content: x\n"), Place::PrivateFile);
        assert_eq!(content("content: x\nmode: \"0600\"\n"), Place::PrivateFile);
        assert!(matches!(
            content("content: x\nmode: \"0644\"\n"),
            Place::Readable(_)
        ));
    }

    #[test]
    fn it_gives_its_path_and_folder_in_the_state_folder() {
        let (shape, _) = check("file", "content: x\nname: app.conf\n");
        let shape = shape.unwrap();
        let names: Vec<_> = shape.gives.iter().map(|g| g.name.name.as_str()).collect();
        assert_eq!(names, ["path", "dir"]);
        let GivenValue::Expr(path) = &shape.gives[0].value else {
            panic!("the path is built from values");
        };
        assert_eq!(
            crate::syntax::expr::write_pieces(match &path.kind {
                ExprKind::Text(pieces) => pieces,
                _ => panic!("the path is text"),
            })
            .as_deref(),
            Some("${environment.state}/the-unit/app.conf")
        );
    }
}

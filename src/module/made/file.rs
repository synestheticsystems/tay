//! `file`: a file tay writes from values, in the environment's state folder.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::{EnvField, Expr, ExprKind, Piece, Reference, Target};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::Value;

use crate::module::description::{
    Claim, Description, ModuleGive, ModuleValue, ReadyWithoutCheck, RestartOn, UnitKind,
};
use crate::module::settings::{Destination, MapSlots, Settings, Slot, maybe};
use crate::module::want::{PathUse, Want};
use crate::module::{Cx, Module};

pub struct File;

const FIELDS: &[&str] = &["content", "template", "name", "folder", "mode", "overwrite"];

/// The folder under the state folder that file units write in.
const FILES: &str = "files";

/// A file unit's settings, each value a [`Slot`] as written or a [`Value`]
/// once resolved.
#[derive(Debug)]
struct FileSettings<V> {
    /// The text written: `content`, or the file `template` names, whose
    /// text is read before start and is the value once resolved.
    content: V,
    /// `None`: the unit's name.
    name: Option<V>,
    /// The folder under `files` in the state folder that it is written in.
    folder: String,
    /// `None`: `"0644"`, or `"0600"` when the content holds a secret.
    mode: Option<V>,
    /// `None`: written again whenever its content would differ.
    overwrite: Option<V>,
}

impl MapSlots for FileSettings<Slot> {
    type With<W> = FileSettings<W>;

    fn map_slots<'a, W>(
        &'a self,
        f: &mut dyn FnMut(&'a Slot) -> Option<W>,
    ) -> Option<FileSettings<W>> {
        let content = f(&self.content);
        let name = maybe(&self.name, f);
        let mode = maybe(&self.mode, f);
        let overwrite = maybe(&self.overwrite, f);
        Some(FileSettings {
            content: content?,
            name: name?,
            folder: self.folder.clone(),
            mode: mode?,
            overwrite: overwrite?,
        })
    }
}

impl Settings for FileSettings<Value> {}

impl Module for File {
    fn name(&self) -> &'static str {
        "file"
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        let name_pieces = match &settings.name {
            Some(slot) => match &slot.expr.kind {
                ExprKind::Text(pieces) => pieces.clone(),
                ExprKind::Ref(reference) => vec![Piece::Ref(reference.clone())],
                ExprKind::Number(n) => vec![Piece::Lit(n.to_string())],
                ExprKind::Bool(b) => vec![Piece::Lit(b.to_string())],
            },
            None => vec![Piece::Lit(cx.unit.to_string())],
        };
        // Files in one folder are compared by their names as written.
        let written_name: String = name_pieces
            .iter()
            .map(|piece| match piece {
                Piece::Lit(text) => text.clone(),
                Piece::Ref(reference) => reference.written(),
            })
            .collect();
        let claim = Claim {
            what: format!(
                "the file `{FILES}/{}/{written_name}` in the state folder",
                settings.folder
            ),
            loc: settings.name.as_ref().map_or(cx.uses, |slot| slot.expr.loc),
        };
        let dir = vec![
            state_folder(cx.uses),
            Piece::Lit(format!("/{FILES}/{}", settings.folder)),
        ];
        let mut path = dir.clone();
        path.push(Piece::Lit("/".to_string()));
        path.extend(name_pieces);
        let given = |name: &str, pieces: Vec<Piece>| ModuleGive {
            name: NameAt {
                name: name.to_string(),
                loc: cx.uses,
            },
            value: ModuleValue::Expr(Expr {
                loc: cx.uses,
                kind: ExprKind::Text(pieces),
            }),
        };
        Some(Description {
            gives: vec![given("path", path), given("dir", dir)],
            claims: vec![claim],
            ..Description::new(
                UnitKind::Made,
                Box::new(settings),
                ReadyWithoutCheck::Written,
                RestartOn::Never,
            )
        })
    }
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<FileSettings<Slot>> {
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
    // A secret may go in the content: the file is then written for its
    // owner alone, unless a `mode` written here says otherwise.
    let content = match (settings.entry("content"), settings.entry("template")) {
        (Some((_, content)), None) => {
            Slot::read(content, Want::Scalar, Destination::FileContent, d)
        }
        (None, Some((_, template))) => Slot::read(
            template,
            Want::Path(PathUse::Read(Destination::FileContent)),
            Destination::Readable("a file path"),
            d,
        ),
        (Some(_), Some((key, _))) => {
            d.error(
                key.loc,
                "a `file` unit has exactly one of `content` and `template`; this one has both",
            );
            None
        }
        (None, None) => {
            d.error(
                settings.loc(),
                "a `file` unit needs exactly one of `content` and `template`",
            );
            None
        }
    };
    let mode = settings.get("mode").and_then(|mode| {
        Slot::read(
            mode,
            Want::FileMode,
            Destination::Readable("a file's mode"),
            d,
        )
    });
    let overwrite = settings.get("overwrite").and_then(|overwrite| {
        Slot::read(
            overwrite,
            Want::Bool,
            Destination::Readable("a file's settings"),
            d,
        )
    });
    let name = settings.get("name").and_then(|node| {
        Slot::read(
            node,
            Want::FileName,
            Destination::Readable("a file name"),
            d,
        )
    });
    let folder = match settings.get("folder") {
        Some(node) => fields::name(node, "`folder`", d).map(|folder| folder.name),
        None => Some(cx.unit.to_string()),
    };
    if cx.d.len() != errors_before {
        return None;
    }
    Some(FileSettings {
        content: content?,
        name,
        folder: folder?,
        mode,
        overwrite,
    })
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
    use crate::module::testing::{self, check};

    fn settings(with: &str) -> FileSettings<Slot> {
        let (read, errors) = testing::read(with, |node, cx| read(Some(node), cx));
        assert!(errors.is_empty(), "{errors:?}");
        read.unwrap()
    }

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
            assert_eq!(
                settings(with).content.destination,
                Destination::FileContent,
                "{with}"
            );
        }
    }

    #[test]
    fn the_content_is_written_out_or_read_from_a_template_before_start() {
        let read = settings("template: ./site.conf.tmpl\nmode: \"0600\"\noverwrite: false\n");
        assert_eq!(
            read.content.want,
            Want::Path(PathUse::Read(Destination::FileContent))
        );
        assert!(read.mode.is_some() && read.overwrite.is_some() && read.name.is_none());
        assert_eq!(read.folder, "the-unit");
    }

    #[test]
    fn it_gives_its_path_and_folder_under_files_in_the_state_folder() {
        let written = |with: &str| {
            let description = check("file", with).0.unwrap();
            let names: Vec<_> = description
                .gives
                .iter()
                .map(|g| g.name.name.as_str())
                .collect();
            assert_eq!(names, ["path", "dir"]);
            let ModuleValue::Expr(path) = &description.gives[0].value else {
                panic!("the path is built from values");
            };
            let ExprKind::Text(pieces) = &path.kind else {
                panic!("the path is text");
            };
            (
                crate::syntax::expr::write_pieces(pieces).unwrap(),
                description.claims[0].what.clone(),
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

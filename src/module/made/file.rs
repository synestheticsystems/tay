//! `file`: a file tay writes from values, in the environment's state folder.

mod write;

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::expr::{EnvField, Expr, ExprKind, Piece, Reference, Target};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::Filled;

use crate::module::description::OpenedPort;
use crate::module::description::{
    Claim, Description, ModuleGive, ModuleValue, ReadyWithoutCheck, RestartOn, UnitKind,
};
use crate::module::operations::Operations;
use crate::module::settings::{Destination, MapValues, Operate, Slot, maybe};
use crate::module::want::{PathUse, Want};
use crate::module::{Cx, Module};

pub struct File;

const FIELDS: &[&str] = &["content", "template", "name", "folder", "mode", "overwrite"];

/// The folder under the state folder that file units write in.
const FILES: &str = "files";

/// The names of the values a file unit gives: its file's path, and the
/// folder it is in.
const PATH: &str = "path";
const DIR: &str = "dir";

/// A file unit's settings, each value a [`Slot`] as written, a [`Value`](crate::value::Value)
/// once resolved, and [`Filled`] once filled.
#[derive(Debug)]
struct FileSettings<V> {
    /// The text written: `content`, or the file `template` names, whose
    /// text is read before start and is the value once resolved.
    content: V,
    /// The file's name, which is the unit's unless `name` gives another,
    /// and where it is written: at `uses` when it is the unit's.
    name: NameAt,
    /// The folder under `files` in the state folder that it is written in.
    folder: String,
    /// `None`: `"0644"`, or `"0600"` when the content holds a secret.
    mode: Option<V>,
    /// `None`: written again whenever its content would differ.
    overwrite: Option<V>,
}

impl<V> MapValues for FileSettings<V> {
    type Value = V;
    type With<W> = FileSettings<W>;

    fn map_values<'a, W>(
        &'a self,
        f: &mut dyn FnMut(&'a V) -> Option<W>,
    ) -> Option<FileSettings<W>> {
        let content = f(&self.content);
        let mode = maybe(&self.mode, f);
        let overwrite = maybe(&self.overwrite, f);
        Some(FileSettings {
            content: content?,
            name: self.name.clone(),
            folder: self.folder.clone(),
            mode: mode?,
            overwrite: overwrite?,
        })
    }
}

/// A unit that is made publishes nothing, so no port is opened for it.
impl Operate for FileSettings<Filled> {
    fn operate(self, _: Vec<OpenedPort<Filled>>) -> Operations {
        Operations::Made(Box::new(self))
    }
}

impl Module for File {
    fn name(&self) -> &'static str {
        "file"
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        let claim = Claim {
            what: format!(
                "the file `{FILES}/{}/{}` in the state folder",
                settings.folder, settings.name.name
            ),
            loc: settings.name.loc,
        };
        let dir = vec![
            state_folder(cx.uses),
            Piece::Lit(format!("/{FILES}/{}", settings.folder)),
        ];
        let mut path = dir.clone();
        path.push(Piece::Lit(format!("/{}", settings.name.name)));
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
            gives: vec![given(PATH, path), given(DIR, dir)],
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
    let name = match settings.get("name") {
        Some(node) => file_name(node, d),
        None => Some(NameAt {
            name: cx.unit.to_string(),
            loc: cx.uses,
        }),
    };
    let folder = match settings.get("folder") {
        Some(node) => fields::name(node, "`folder`", d).map(|folder| folder.name),
        None => Some(cx.unit.to_string()),
    };
    if cx.d.len() != errors_before {
        return None;
    }
    Some(FileSettings {
        content: content?,
        name: name?,
        folder: folder?,
        mode,
        overwrite,
    })
}

/// `name`: a file's name, with no folder in it. As every name, it is
/// written out, so that two units that would write one file are found
/// before anything is written.
fn file_name(node: &Node, d: &mut Diagnostics) -> Option<NameAt> {
    let expr = Want::Text.read(node, d)?;
    let Some(name) = expr.literal_text() else {
        let written = node.as_str().map_or("", |text| text.text.as_str());
        d.error(
            node.loc,
            format!("`name`: `{written}` holds a reference, and a name must be written out"),
        );
        return None;
    };
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        d.error(
            node.loc,
            format!("this must be a file name with no folder in it; this is `{name}`"),
        );
        return None;
    }
    Some(NameAt {
        name,
        loc: node.loc,
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
    fn a_name_or_folder_with_a_reference_in_it_is_an_error() {
        let (_, errors) = check("file", "content: x\nname: \"${settings.name}.conf\"\n");
        assert_eq!(
            errors,
            [
                "2:7: `name`: `${settings.name}.conf` holds a reference, and a name must be written out"
            ]
        );
        let (_, errors) = check("file", "content: x\nfolder: \"${settings.folder}\"\n");
        assert_eq!(
            errors,
            ["2:9: `folder`: `${settings.folder}` is a reference, and a name must be written out"]
        );
        // `$${` writes a literal `${`, which a file's name may hold.
        assert_eq!(
            settings("content: x\nname: \"a$${b}\"\n").name.name,
            "a${b}"
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
        assert!(read.mode.is_some() && read.overwrite.is_some());
        assert_eq!(
            (read.name.name.as_str(), read.folder.as_str()),
            ("the-unit", "the-unit")
        );
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

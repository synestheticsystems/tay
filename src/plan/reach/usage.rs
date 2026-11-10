//! A unit using a value: the value resolved for where the unit runs, and
//! checked against what is wanted there and whether a secret may go there.

use std::fs;
use std::path::Path;

use crate::definition::read::cross_references::references as cross_references;
use crate::diagnostic::source::Loc;
use crate::diagnostic::{Diagnostic, quoted_list};
use crate::module::{Destination, Form, PathUse, Slot, Want};
use crate::syntax::expr::{self, Expr, ExprKind};

use super::visit::NeedEdge;
use crate::plan::Planner;
use crate::plan::model::{Because, OpenedBy, Opener};
use crate::plan::resolve::resolved::Resolved;
use crate::plan::resolve::{Asker, Fail, Scope, ViewProblem};

/// Who uses a value: a unit, from where it runs, or from the host for a
/// readiness check tay makes itself.
#[derive(Clone, Copy)]
pub struct User<'a> {
    pub unit: usize,
    pub name: &'a str,
    pub asker: Asker,
    pub readiness: bool,
}

impl User<'_> {
    /// The user, as an error about a view it cannot have begins.
    fn who(&self) -> String {
        let name = self.name;
        match (self.readiness, self.asker) {
            (true, Asker::Host) => {
                format!("the readiness check of unit `{name}` is made from the host")
            }
            (_, Asker::Host) => format!("unit `{name}` runs on the host"),
            (_, Asker::Container) => format!("unit `{name}` runs in a container"),
            (_, Asker::Nowhere) => format!("unit `{name}` is made, and runs nowhere"),
        }
    }
}

/// What using values adds to the plan: the needs it makes, and the
/// published ports it opens.
#[derive(Default)]
pub struct Uses {
    pub needs: Vec<NeedEdge>,
    pub opens: Vec<Opening>,
}

/// A published port that a use from the host opens.
pub struct Opening {
    pub container: String,
    pub port: i64,
    pub opener: Opener,
}

impl Planner<'_> {
    /// Resolves `expr` for `user` and checks it against what is wanted and
    /// where it ends up. Each unit that makes or produces a value it is built
    /// from is a need, and each published port it reaches from the host is
    /// opened; both go into `uses`. For a file read before start the value
    /// used is the file's text, resolved. `None` when it cannot be used; the
    /// reason is reported.
    pub fn use_value(&mut self, user: User<'_>, slot: &Slot, uses: &mut Uses) -> Option<Resolved> {
        let Slot {
            expr,
            want,
            destination,
        } = slot;
        let scope = Scope {
            unit: Some(user.unit),
            asker: user.asker,
        };
        let value = match self.resolver.resolve(expr, scope) {
            Ok(value) if matches!(want, Want::Path(_)) => self.resolver.host_path(value),
            Ok(value) => value,
            Err(Fail::Reported) => return None,
            Err(Fail::View(problems)) => {
                for problem in problems {
                    // A value that is one reference is used where it is.
                    let problem = match &expr.kind {
                        ExprKind::Ref(_) => problem.used_at(expr.loc),
                        _ => problem,
                    };
                    self.resolver.d.push(view_error(&user.who(), problem));
                }
                return None;
            }
        };
        let built_from = value
            .producers
            .iter()
            .map(|(unit, used)| (unit, Because::UsesProduced(used.clone())))
            .chain(
                value
                    .made
                    .iter()
                    .map(|(unit, used)| (unit, Because::UsesMade(used.clone()))),
            );
        for (unit, because) in built_from {
            if let Some(unit) = self.resolver.unit_index(unit) {
                uses.needs.push(NeedEdge {
                    unit,
                    loc: expr.loc,
                    because,
                });
            }
        }
        // Only a view can make a unit that does not run on the host reach a
        // published port from the host.
        let how = match (user.readiness, user.asker) {
            (true, _) => OpenedBy::ReadinessCheck,
            (false, Asker::Host) => OpenedBy::UnitOnHost,
            (false, _) => OpenedBy::View,
        };
        for used in &value.published_uses {
            uses.opens.push(Opening {
                container: used.container.clone(),
                port: used.port,
                opener: Opener {
                    unit: user.name.to_string(),
                    how,
                },
            });
        }
        if let Some(problem) = want.problem(&Form::of(&value.value)) {
            self.resolver
                .d
                .error(expr.loc, format!("{} {problem}", expr.subject()));
            return None;
        }
        self.check_destination(&value, *destination, expr.loc);
        match want {
            Want::Path(PathUse::Read(content)) => {
                if !value.producers.is_empty() {
                    self.resolver.d.error(
                        expr.loc,
                        "this must be known before start, because tay reads the file then; this is built from a value produced by running",
                    );
                    return None;
                }
                let path = value.value.exact_text()?;
                return self.use_template_file(user, &path, expr.loc, *content, uses);
            }
            Want::Path(PathUse::Mounted) => self.check_mount_source(&value, expr.loc),
            _ => {}
        }
        Some(value)
    }

    fn check_destination(&mut self, value: &Resolved, destination: Destination, loc: Loc) {
        let secrets = || quoted_list(value.value.secrets());
        let secret = value.value.is_secret();
        match destination {
            Destination::Readable(what) if secret => self.resolver.d.error(
                loc,
                format!(
                    "a secret ({}) cannot go in {what}: other programs can read it",
                    secrets()
                ),
            ),
            Destination::SecretEnv if !secret => self.resolver.d.error(
                loc,
                "a value in `secret_env` must have a secret in it, and this one has none",
            ),
            _ => {}
        }
    }

    /// A mount's source must exist when tay plans, unless tay makes it (in
    /// the state, log or shared folder) or running makes it (a produced
    /// value).
    /// A path not known before start cannot be looked at.
    fn check_mount_source(&mut self, value: &Resolved, at: Loc) {
        if !value.producers.is_empty() {
            return;
        }
        let Some(path) = value.value.exact_text() else {
            return;
        };
        if self.resolver.in_own_folders(Path::new(&path)) || Path::new(&path).exists() {
            return;
        }
        self.resolver.d.error(
            at,
            format!(
                "the mount's source `{path}` does not exist; outside the state, log and shared folders it must exist when tay plans"
            ),
        );
    }

    /// Reads a file whose text has references in it, before start, and
    /// resolves those references for `user`, whose setting at `at` names
    /// it: the text is the value used. Its references are checked as those
    /// written in a definition are. Each error in the file says which unit
    /// it was found for, so that a file several units read is reported once.
    fn use_template_file(
        &mut self,
        user: User<'_>,
        path: &str,
        at: Loc,
        content: Destination,
        uses: &mut Uses,
    ) -> Option<Resolved> {
        let text = self.template_text(path, at)?;
        let source = text.loc.source;
        let content = Slot {
            expr: text,
            want: Want::Scalar,
            destination: content,
        };
        let outer = std::mem::take(&mut self.resolver.d);
        let stack = self.resolver.stack;
        let unit = &stack.units[user.unit];
        cross_references(stack, &content.expr, Some(unit), &mut self.resolver.d);
        let value = self.use_value(user, &content, uses);
        let found = std::mem::replace(&mut self.resolver.d, outer);
        for error in found.into_sorted() {
            self.resolver.d.push(match error.loc.source == source {
                true => error.with(at, format!("read for unit `{}` here", user.name)),
                false => error,
            });
        }
        value
    }

    /// The text of the template file at `path`, read once however many units
    /// use it, as a value whose references each stand where they are
    /// written in it.
    /// `None` when it cannot be read or a reference in it cannot be parsed,
    /// which is reported.
    fn template_text(&mut self, path: &str, at: Loc) -> Option<Expr> {
        if !self.template_files.contains_key(path) {
            let file = Path::new(path);
            let text = match fs::read_to_string(file) {
                Ok(text) => text,
                Err(error) => {
                    self.resolver.d.error(
                        at,
                        format!("the template file `{path}` cannot be read: {error}"),
                    );
                    return None;
                }
            };
            let source = self.resolver.sources.add_file(file, &self.root);
            let read = expr::parse_document(&text, source).map(|pieces| Expr {
                loc: Loc::new(source, 1, 1),
                kind: ExprKind::Text(pieces),
            });
            self.template_files.insert(path.to_string(), read);
        }
        match &self.template_files[path] {
            Ok(text) => Some(text.clone()),
            Err(error) => {
                self.resolver.d.push(error.clone());
                None
            }
        }
    }
}

/// An address `who` uses where the problem says, and cannot have.
pub fn view_error(who: &str, problem: ViewProblem) -> Diagnostic {
    let at = problem.at();
    match problem {
        ViewProblem::NotPublished {
            owner,
            port,
            endpoint,
            ..
        } => Diagnostic::new(
            at,
            format!(
                "{who}, and uses here an address of unit `{owner}` as the host sees it, but `{owner}` publishes no host port for its port {port}"
            ),
        )
        .with(endpoint, format!("the endpoint of `{owner}` is defined here")),
        ViewProblem::NeedsView {
            reference,
            endpoint,
            ..
        } => Diagnostic::new(
            at,
            format!(
                "{who}, so `{reference}` must say which view of the address it wants: add `@host`, `@net`, or `@` and the name of the unit that uses it"
            ),
        )
        .with(endpoint, "the endpoint is defined here"),
    }
}

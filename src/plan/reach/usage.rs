//! A unit using a value: the value resolved for where the unit runs, and
//! checked against what the place wants and whether a secret may go there.

use std::fs;
use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::module::{Form, PathUse, Place, Want};
use crate::syntax::expr::{self, Expr};

use super::visit::NeedEdge;
use crate::plan::Planner;
use crate::plan::model::{Because, OpenedBy, Opener};
use crate::plan::resolve::value::Resolved;
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
    /// Resolves `expr` for `user` and checks it at its place. Each unit that
    /// makes or produces a value it is built from is a need, and each
    /// published port it reaches from the host is opened; both go into
    /// `uses`. `None` when it cannot be used; the reason is reported.
    pub fn use_value(
        &mut self,
        user: User<'_>,
        expr: &Expr,
        want: Want,
        place: Place,
        uses: &mut Uses,
    ) -> Option<Resolved> {
        let scope = Scope {
            unit: Some(user.unit),
            asker: user.asker,
        };
        let value = match self.resolver.resolve(expr, scope) {
            Ok(value) if matches!(want, Want::Path(_)) => self.resolver.host_path(value, expr.loc),
            Ok(value) => value,
            Err(Fail::Reported) => return None,
            Err(Fail::View(problems)) => {
                for problem in problems {
                    self.resolver
                        .d
                        .push(view_error(&user.who(), expr.loc, problem));
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
        for (container, port) in &value.published_uses {
            uses.opens.push(Opening {
                container: container.clone(),
                port: *port,
                opener: Opener {
                    unit: user.name.to_string(),
                    how,
                },
            });
        }
        if let Some(problem) = want.problem(&Form::of(&value.value)) {
            let what = match &expr.kind {
                expr::ExprKind::Ref(reference) => format!("`{}`", reference.written()),
                _ => "this".to_string(),
            };
            self.resolver.d.error(expr.loc, format!("{what} {problem}"));
            return None;
        }
        self.check_place(&value, place, expr.loc);
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
                self.use_template_file(user, &path, expr.loc, content, uses)?;
            }
            Want::Path(PathUse::Mounted) => self.check_mount_source(&value, expr.loc),
            _ => {}
        }
        Some(value)
    }

    fn check_place(&mut self, value: &Resolved, place: Place, loc: Loc) {
        let secrets = || {
            value
                .value
                .secrets()
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let secret = value.value.is_secret();
        match place {
            Place::Readable(what) if secret => self.resolver.d.error(
                loc,
                format!(
                    "a secret ({}) cannot go in {what}: other programs can read it",
                    secrets()
                ),
            ),
            Place::SecretEnv if !secret => self.resolver.d.error(
                loc,
                "a value in `secret_env` must have a secret in it, and this one has none",
            ),
            _ => {}
        }
    }

    /// A mount's source must exist when tay plans, unless tay makes it (in
    /// the state or shared folder) or running makes it (a produced value).
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
                "the mount's source `{path}` does not exist; outside the state and shared folders it must exist when tay plans"
            ),
        );
    }

    /// Reads a file whose text has references in it, before start, and
    /// resolves those references for `user`.
    fn use_template_file(
        &mut self,
        user: User<'_>,
        path: &str,
        at: Loc,
        content: Place,
        uses: &mut Uses,
    ) -> Option<()> {
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
        let source = self.sources.add_file(file, &self.root);
        let pieces = match expr::parse_document(&text, source) {
            Ok(pieces) => pieces,
            Err(error) => {
                self.resolver.d.push(error);
                return None;
            }
        };
        let content_expr = Expr {
            loc: Loc::new(source, 1, 1),
            kind: expr::ExprKind::Text(pieces),
        };
        self.use_value(user, &content_expr, Want::Scalar, content, uses)
            .map(|_| ())
    }
}

/// An address `who` uses at `at` and cannot have.
pub fn view_error(who: &str, at: Loc, problem: ViewProblem) -> Diagnostic {
    match problem {
        ViewProblem::NotPublished {
            owner,
            port,
            endpoint,
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
        ViewProblem::NotOpened { owner, port } => Diagnostic::new(
            at,
            format!(
                "{who}, and uses here an address of unit `{owner}` as the host sees it, but nothing in the plan opens its port {port} on the host"
            ),
        ),
    }
}

//! A unit using a value: the value resolved for where the unit runs, and
//! checked against what the place wants and whether a secret may go there.

use std::fs;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::Loc;
use crate::module::{Place, Want};
use crate::syntax::expr::{self, Expr};

use super::visit::NeedEdge;
use crate::plan::Planner;
use crate::plan::model::Because;
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

impl Planner<'_> {
    /// Resolves `expr` for `user` and checks it at its place. Each unit that
    /// makes or produces a value it is built from is a need, added to
    /// `needs`. `None` when it cannot be used; the reason is reported.
    pub fn use_value(
        &mut self,
        user: User<'_>,
        expr: &Expr,
        want: Want,
        place: Place,
        needs: &mut Vec<NeedEdge>,
    ) -> Option<Resolved> {
        let scope = Scope {
            unit: Some(user.unit),
            asker: user.asker,
        };
        let value = match self.resolver.resolve(expr, scope) {
            Ok(value) => value,
            Err(Fail::Reported) => return None,
            Err(Fail::View(problem)) => {
                self.resolver.d.push(view_error(user, expr.loc, problem));
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
                needs.push(NeedEdge {
                    unit,
                    loc: expr.loc,
                    because,
                });
            }
        }
        if let Some(problem) = want.problem(&value.form()) {
            let what = match &expr.kind {
                expr::ExprKind::Ref(reference) => format!("`{}`", reference.written()),
                _ => "this".to_string(),
            };
            self.resolver.d.error(expr.loc, format!("{what} {problem}"));
            return None;
        }
        self.check_place(&value, place, expr.loc);
        if let Want::TemplateFile(content) = want {
            let path = value.exact_text()?;
            self.use_template_file(user, &path, expr.loc, content, needs)?;
        }
        Some(value)
    }

    fn check_place(&mut self, value: &Resolved, place: Place, loc: Loc) {
        let secrets = || {
            value
                .secrets
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match place {
            Place::Readable(what) if value.is_secret() => self.resolver.d.error(
                loc,
                format!(
                    "a secret ({}) cannot go in {what}: other programs can read it",
                    secrets()
                ),
            ),
            Place::SecretEnv if !value.is_secret() => self.resolver.d.error(
                loc,
                "`secret_env` takes only references to secret values, and this value is not secret",
            ),
            _ => {}
        }
    }

    /// Reads a file whose text has references in it, before start, and
    /// resolves those references for `user`.
    fn use_template_file(
        &mut self,
        user: User<'_>,
        path: &str,
        at: Loc,
        content: Place,
        needs: &mut Vec<NeedEdge>,
    ) -> Option<()> {
        let file = self.root.join(path);
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                self.resolver.d.error(
                    at,
                    format!("the template file `{path}` cannot be read: {error}"),
                );
                return None;
            }
        };
        let source = self.sources.add_file(&file, &self.root);
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
        self.use_value(user, &content_expr, Want::Scalar, content, needs)
            .map(|_| ())
    }
}

fn view_error(user: User<'_>, at: Loc, problem: ViewProblem) -> Diagnostic {
    let who = user.who();
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
        } => Diagnostic::new(
            at,
            format!(
                "{who}, so `{reference}` must say which view of the address it wants: add `@host`, `@net`, or `@` and the name of the unit that uses it"
            ),
        )
        .with(endpoint, "the endpoint is defined here"),
    }
}

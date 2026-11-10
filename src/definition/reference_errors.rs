//! The errors a reference can have, each written once. The reader finds
//! them across every way a unit has, whether or not it comes up; the planner
//! finds them under the way chosen, and in a template file's text, which is
//! read only when its unit comes up.

use crate::diagnostic::{Diagnostic, quoted_list};
use crate::syntax::expr::Reference;

fn error(reference: &Reference, message: String) -> Diagnostic {
    Diagnostic::new(reference.loc, format!("`{}`{message}", reference.written()))
}

pub fn no_unit(reference: &Reference, name: &str) -> Diagnostic {
    error(reference, format!(": there is no unit `{name}`"))
}

pub fn self_outside_a_unit(reference: &Reference) -> Diagnostic {
    error(reference, ": `self` is used only inside a unit".to_string())
}

/// `given` is every value the unit gives, in order.
pub fn gives_no(reference: &Reference, owner: &str, name: &str, given: &[&str]) -> Diagnostic {
    let given = match given {
        [] => "nothing".to_string(),
        names => quoted_list(names),
    };
    error(
        reference,
        format!(": unit `{owner}` gives no `{name}`; it gives {given}"),
    )
}

pub fn not_an_endpoint(reference: &Reference, name: &str) -> Diagnostic {
    error(
        reference,
        format!(": `{name}` is not an endpoint, so it has no `.url`, `.host` or `.port`"),
    )
}

pub fn no_scheme(reference: &Reference) -> Diagnostic {
    error(
        reference,
        ": the endpoint has no `scheme`, so it has no URL".to_string(),
    )
}

pub fn builds_no_output(reference: &Reference, owner: &str, name: &str) -> Diagnostic {
    error(
        reference,
        format!(": unit `{owner}` builds no output `{name}`"),
    )
}

pub fn no_setting(reference: &Reference, name: &str) -> Diagnostic {
    error(
        reference,
        format!(": no setting `{name}` is declared in tay.yaml"),
    )
}

pub fn param_outside_a_template(reference: &Reference) -> Diagnostic {
    error(
        reference,
        " is a template's parameter, and this is not in a template".to_string(),
    )
}

pub fn view_of_a_unit_that_runs_nowhere(reference: &Reference, unit: &str) -> Diagnostic {
    error(
        reference,
        format!(
            " asks for the view of unit `{unit}`, which runs nowhere; the unit named after `@` must be one that runs"
        ),
    )
}

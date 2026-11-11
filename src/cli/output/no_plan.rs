//! Why a daemon has no plan, as a command says it and ends on it: the
//! definitions' errors, each as `tay plan` prints it, closed by what having
//! no plan means; or what else kept it from planning.

use serde_json::{Value, json};

use super::text;
use crate::cli::Exit;
use crate::control::protocol::NoPlan;

/// What having no plan means, as a closing line says it.
const MEANS: &str = "the daemon has no plan, so it looks after nothing until they plan";

/// Why there is no plan, as `command` prints it on standard error.
pub fn text(command: &str, no_plan: &NoPlan) -> String {
    match no_plan {
        // A tay too old for the definitions says only why, as `tay plan`.
        NoPlan::Errors(errors) if errors.too_old => errors
            .lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect(),
        NoPlan::Errors(errors) => text::errors(command, &errors.lines, MEANS),
        NoPlan::Problem { problem } => format!(
            "{command}: the daemon has no plan, so it looks after nothing until it can plan: {problem}\n"
        ),
    }
}

/// Why there is no plan, in a command's JSON.
pub fn json(no_plan: &NoPlan) -> Value {
    match no_plan {
        NoPlan::Errors(errors) => json!({
            "too_old": errors.too_old,
            "errors": errors.lines,
        }),
        NoPlan::Problem { problem } => json!({ "problem": problem }),
    }
}

/// How a command that met no plan ends: as one that plans and meets the
/// same errors, or as one that cannot use what tay keeps or does.
pub fn exit(no_plan: &NoPlan) -> Exit {
    match no_plan {
        NoPlan::Errors(errors) if errors.too_old => Exit::TooOld,
        NoPlan::Errors(_) => Exit::Errors,
        NoPlan::Problem { .. } => Exit::Unreachable,
    }
}

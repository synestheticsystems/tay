//! A change's events as `tay up` and `tay down` print them while they
//! happen: each change of state on a line of its own, and at the end what
//! the change came to, and of one that brings units up, how long it took.
//! `tay run` prints what the action writes, and the values it produced, on
//! standard output, and its own lines on standard error. With `--json`,
//! each is a JSON object on a line, in a versioned schema.

mod took;

use serde_json::{Value, json};

use super::no_plan;
use crate::cli::Exit;
use crate::control::protocol::Event;
use crate::definition::read::THIS_TAY;
use crate::diagnostic::quoted_list;
use crate::engine::{Decision, State};
use crate::print::{err, out};

/// The schema of the JSON lines. A change to any key or its meaning is a
/// new number.
pub const SCHEMA: u32 = 1;

/// What a value declared secret is shown as: never its value.
const SECRET: &str = "<secret>";

/// Prints the events of the change `command` asked for.
pub struct Printer {
    command: &'static str,
    json: bool,
    /// Where tay's own lines go, as text: standard output, or standard
    /// error for a command whose standard output is an action's.
    own: fn(&str),
    /// How wide unit names are printed, so that states line up.
    width: usize,
    /// How the command ends, once the change has said.
    exit: Option<Exit>,
}

impl Printer {
    pub fn new(command: &'static str, json: bool) -> Printer {
        Printer {
            command,
            json,
            own: out,
            width: 0,
            exit: None,
        }
    }

    /// The printer of a command whose standard output is what an action
    /// writes, and the values it produced: tay's own lines go to standard
    /// error.
    pub fn beside_output(command: &'static str, json: bool) -> Printer {
        Printer {
            own: err,
            ..Printer::new(command, json)
        }
    }

    /// How the command ends, from what the change said; `None` when it said
    /// nothing of its end.
    pub fn exit(&self) -> Option<Exit> {
        self.exit
    }

    pub fn event(&mut self, event: Event) {
        self.exit = match &event {
            Event::Units { .. }
            | Event::State { .. }
            | Event::Notice { .. }
            | Event::Output { .. }
            | Event::Removed { .. } => self.exit,
            Event::Wiped => Some(Exit::Done),
            Event::Ran {
                decision: Decision::Ready,
                ..
            } => Some(Exit::Done),
            // An action that failed exits as its program did, unless how
            // is not known, or says nothing of failing.
            Event::Ran { exit_status, .. } => Some(
                exit_status
                    .and_then(|status| u8::try_from(status).ok())
                    .filter(|code| *code != 0)
                    .map_or(Exit::Failed, Exit::Action),
            ),
            Event::Errors(errors) if errors.too_old => Some(Exit::TooOld),
            Event::Errors(_) => Some(Exit::Errors),
            Event::NoPlan(no_plan) => Some(no_plan::exit(no_plan)),
            Event::Decided {
                decision: Decision::Ready,
                ..
            } => Some(Exit::Done),
            Event::Decided { .. } | Event::Cancelled { .. } | Event::Problem { .. } => {
                Some(Exit::Failed)
            }
            Event::Down { left } if left.is_empty() => Some(Exit::Done),
            Event::Down { .. } => Some(Exit::Failed),
            Event::Refused(_) => Some(Exit::Refused),
        };
        if self.json {
            self.json_line(&event);
        } else {
            self.text(event);
        }
    }

    fn text(&mut self, event: Event) {
        let command = self.command;
        let own = self.own;
        match event {
            Event::Units { units } => {
                self.width = units.iter().map(String::len).max().unwrap_or(0);
            }
            Event::State {
                unit,
                state,
                detail,
                ..
            } => own(&format!(
                "  {unit:<width$}  {:<8}  {detail}\n",
                state.word(),
                width = self.width
            )),
            Event::Notice { notice } => own(&format!("{command}: {notice}.\n")),
            Event::Errors(errors) => {
                for line in &errors.lines {
                    err(&format!("{line}\n"));
                }
                if !errors.too_old {
                    err(&format!(
                        "{command}: the definitions have errors now; nothing more was started.\n"
                    ));
                }
            }
            Event::NoPlan(no_plan) => err(&no_plan::text(command, &no_plan)),
            Event::Decided {
                decision,
                seconds,
                took,
            } => {
                self.decision(decision);
                own(&took::line(command, seconds, &took));
            }
            Event::Cancelled { by } => own(&format!("{command}: cancelled by {by}.\n")),
            Event::Down { left } if left.is_empty() => {
                own(&format!("{command}: nothing is left running.\n"));
            }
            Event::Down { left } => {
                for left in left {
                    own(&format!(
                        "{command}: `{}` is left: {}\n",
                        left.unit, left.reason
                    ));
                }
            }
            Event::Problem { problem } => err(&format!("{command}: {problem}\n")),
            Event::Refused(refused) => err(&format!("{command}: {refused}\n")),
            Event::Output { line } => {
                out(&format!("{line}\n"));
            }
            Event::Removed { unit, removed } => {
                let removed = removed.join("; ");
                match unit {
                    Some(unit) => own(&format!(
                        "{command}: removed what `{unit}` kept: {removed}\n"
                    )),
                    None => own(&format!("{command}: removed {removed}\n")),
                }
            }
            Event::Wiped => own(&format!(
                "{command}: done; the environment is down, so nothing was brought up again.\n"
            )),
            Event::Ran {
                decision, values, ..
            } => {
                match decision {
                    Decision::Ready => own(&format!("{command}: the action is done.\n")),
                    failed => self.decision(failed),
                }
                if !values.is_empty() {
                    own(&format!("{command}: the action produced:\n"));
                }
                let width = values.iter().map(|value| value.name.len()).max();
                for produced in values {
                    let value = produced.value.as_deref().unwrap_or(SECRET);
                    out(&format!(
                        "{:<width$}  {value}\n",
                        produced.name,
                        width = width.unwrap_or(0)
                    ));
                }
            }
        }
    }

    /// Says what a change that brings units up came to.
    fn decision(&self, decision: Decision) {
        let command = self.command;
        let own = self.own;
        match decision {
            Decision::Ready => {
                own(&format!("{command}: everything asked for is ready.\n"));
            }
            Decision::Failed { failed } => {
                for failed in failed {
                    let came = match failed.state {
                        State::Unready => "is unready",
                        _ => "failed",
                    };
                    own(&format!(
                        "{command}: `{}` {came}: {}\n",
                        failed.unit, failed.reason
                    ));
                    if !failed.lines.is_empty() {
                        own("  the last lines it wrote:\n");
                        for line in &failed.lines {
                            own(&format!("    {line}\n"));
                        }
                    }
                    if !failed.waiting.is_empty() {
                        own(&format!(
                            "  waiting on it: {}\n",
                            quoted_list(&failed.waiting)
                        ));
                    }
                }
            }
        }
    }

    fn json_line(&self, event: &Event) {
        let mut line = match event {
            Event::Units { units } => json!({ "event": "units", "units": units }),
            Event::State {
                unit,
                state,
                detail,
                at,
            } => json!({
                "event": "state",
                "unit": unit,
                "state": state.word(),
                "detail": detail,
                "at": at,
            }),
            Event::Notice { notice } => json!({ "event": "notice", "notice": notice }),
            Event::Errors(errors) => json!({
                "event": "errors",
                "too_old": errors.too_old,
                "errors": errors.lines,
            }),
            Event::NoPlan(why) => {
                let mut line = no_plan::json(why);
                line["event"] = json!("no_plan");
                line
            }
            Event::Decided {
                decision,
                seconds,
                took,
            } => {
                let mut decided = self.decision_json(decision);
                took::add_to(&mut decided, *seconds, took);
                decided
            }
            Event::Cancelled { by } => json!({ "event": "cancelled", "by": by }),
            Event::Down { left } => json!({
                "event": "down",
                "left": left.iter().map(|left| json!({ "unit": left.unit, "reason": left.reason })).collect::<Vec<_>>(),
            }),
            Event::Problem { problem } => json!({ "event": "problem", "problem": problem }),
            Event::Refused(refused) => {
                json!({ "event": "refused", "problem": refused.to_string() })
            }
            Event::Output { line } => json!({ "event": "output", "line": line }),
            Event::Removed { unit, removed } => {
                json!({ "event": "removed", "unit": unit, "removed": removed })
            }
            Event::Wiped => json!({ "event": "wiped" }),
            Event::Ran {
                decision,
                exit_status,
                values,
            } => {
                let mut ran = self.decision_json(decision);
                ran["exit_status"] = json!(exit_status);
                ran["values"] = values
                    .iter()
                    .map(|produced| json!({ "name": produced.name, "value": produced.value }))
                    .collect();
                ran
            }
        };
        if let Value::Object(fields) = &mut line {
            fields.insert("schema".to_string(), json!(SCHEMA));
            fields.insert("tay".to_string(), json!(THIS_TAY));
        }
        out(&format!("{line}\n"));
    }
}

impl Printer {
    /// What a change came to, as an event named after its command: `up`
    /// for `tay up`.
    fn decision_json(&self, decision: &Decision) -> Value {
        let event = self.command.strip_prefix("tay ").unwrap_or(self.command);
        match decision {
            Decision::Ready => json!({ "event": event, "decision": "ready", "failed": [] }),
            Decision::Failed { failed } => json!({
                "event": event,
                "decision": "failed",
                "failed": failed.iter().map(|failed| json!({
                    "unit": failed.unit,
                    "state": failed.state.word(),
                    "reason": failed.reason,
                    "lines": failed.lines,
                    "waiting": failed.waiting,
                })).collect::<Vec<_>>(),
            }),
        }
    }
}

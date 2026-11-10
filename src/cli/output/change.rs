//! A change's events as `tay up` and `tay down` print them while they
//! happen: each change of state on a line of its own, and at the end what
//! the change came to. With `--json`, each is a JSON object on a line, in
//! a versioned schema.

use serde_json::{Value, json};

use crate::cli::Exit;
use crate::control::protocol::Event;
use crate::definition::read::THIS_TAY;
use crate::diagnostic::quoted_list;
use crate::engine::Decision;
use crate::print::{err, out};

/// The schema of the JSON lines. A change to any key or its meaning is a
/// new number.
pub const SCHEMA: u32 = 1;

/// Prints the events of the change `command` asked for.
pub struct Printer {
    command: &'static str,
    json: bool,
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
            width: 0,
            exit: None,
        }
    }

    /// How the command ends, from what the change said; `None` when it said
    /// nothing of its end.
    pub fn exit(&self) -> Option<Exit> {
        self.exit
    }

    pub fn event(&mut self, event: Event) {
        self.exit = match &event {
            Event::Units { .. } | Event::State { .. } | Event::Notice { .. } => self.exit,
            Event::Errors { too_old: true, .. } => Some(Exit::TooOld),
            Event::Errors { .. } => Some(Exit::Errors),
            Event::Decided(Decision::Ready) => Some(Exit::Done),
            Event::Decided(_) | Event::Cancelled { .. } | Event::Problem { .. } => {
                Some(Exit::Failed)
            }
            Event::Down { left } if left.is_empty() => Some(Exit::Done),
            Event::Down { .. } => Some(Exit::Failed),
        };
        if self.json {
            self.json_line(&event);
        } else {
            self.text(event);
        }
    }

    fn text(&mut self, event: Event) {
        let command = self.command;
        match event {
            Event::Units { units } => {
                self.width = units.iter().map(String::len).max().unwrap_or(0);
            }
            Event::State {
                unit,
                state,
                detail,
                ..
            } => out(&format!(
                "  {unit:<width$}  {:<8}  {detail}\n",
                state.word(),
                width = self.width
            )),
            Event::Notice { notice } => out(&format!("{command}: {notice}.\n")),
            Event::Errors { lines, too_old } => {
                for line in &lines {
                    err(&format!("{line}\n"));
                }
                if !too_old {
                    err(&format!(
                        "{command}: the definitions have errors now; nothing more was started.\n"
                    ));
                }
            }
            Event::Decided(Decision::Ready) => {
                out(&format!("{command}: everything asked for is ready.\n"));
            }
            Event::Decided(Decision::Failed { failed }) => {
                for failed in failed {
                    out(&format!(
                        "{command}: `{}` failed: {}\n",
                        failed.unit, failed.reason
                    ));
                    if !failed.lines.is_empty() {
                        out("  the last lines it wrote:\n");
                        for line in &failed.lines {
                            out(&format!("    {line}\n"));
                        }
                    }
                    if !failed.waiting.is_empty() {
                        out(&format!(
                            "  waiting on it: {}\n",
                            quoted_list(&failed.waiting)
                        ));
                    }
                }
            }
            Event::Cancelled { by } => out(&format!("{command}: cancelled by {by}.\n")),
            Event::Down { left } if left.is_empty() => {
                out(&format!("{command}: nothing is left running.\n"));
            }
            Event::Down { left } => {
                for left in left {
                    out(&format!(
                        "{command}: `{}` is left: {}\n",
                        left.unit, left.reason
                    ));
                }
            }
            Event::Problem { problem } => err(&format!("{command}: {problem}\n")),
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
            Event::Errors { lines, too_old } => json!({
                "event": "errors",
                "too_old": too_old,
                "errors": lines,
            }),
            Event::Decided(decision) => self.decision_json(decision),
            Event::Cancelled { by } => json!({ "event": "cancelled", "by": by }),
            Event::Down { left } => json!({
                "event": "down",
                "left": left.iter().map(|left| json!({ "unit": left.unit, "reason": left.reason })).collect::<Vec<_>>(),
            }),
            Event::Problem { problem } => json!({ "event": "problem", "problem": problem }),
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
                    "reason": failed.reason,
                    "lines": failed.lines,
                    "waiting": failed.waiting,
                })).collect::<Vec<_>>(),
            }),
        }
    }
}

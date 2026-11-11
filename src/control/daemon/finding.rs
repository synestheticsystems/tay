//! Finding the units again against the plan `tay plan` would make with
//! what the environment remembers being asked for: as the daemon starts,
//! and, while it has no plan, each time a command asks it anything, so
//! that what it says is true of the definitions as they are then. Without a
//! plan, what runs is found all the same and looked after by nothing; once
//! the definitions plan, the daemon goes on as one started at that moment
//! would.

use std::path::PathBuf;
use std::sync::Arc;

use super::{Daemon, log, options, planned};
use crate::control::protocol::NoPlan;
use crate::disk::{Asked, Intent};
use crate::engine::{self, Supervisor};
use crate::module::Occasion;
use crate::plan::{Plan, PlannedUnit};

/// Finds the units again as the daemon starts; why it has no plan, when it
/// has none.
pub(super) async fn found_again(supervisor: &Arc<Supervisor>) -> Option<NoPlan> {
    match plan_now(supervisor).await {
        Ok(plan) => {
            found_against(supervisor, plan).await;
            None
        }
        Err(no_plan) => {
            let why = match &no_plan {
                NoPlan::Errors(errors) => errors.lines.join("\n"),
                NoPlan::Problem { problem } => problem.clone(),
            };
            log(&format!(
                "there is no plan, so what runs is found and looked after by nothing:\n{why}"
            ));
            if let Err(why) = engine::find_without_plan(supervisor).await {
                log(&format!("what is left of the units cannot be told: {why}"));
            }
            Some(no_plan)
        }
    }
}

impl Daemon {
    /// Plans again, for a daemon that has no plan, as a command asks it
    /// something, and once the definitions plan, finds the units again
    /// against that plan. Not while a change is under way: one that brings
    /// units up plans for itself, and a down stops everything.
    pub(super) async fn plan_again_if_none(&self) {
        let _planning = self.planning.lock().await;
        let none = (self.no_plan.lock())
            .expect("why there is no plan is not poisoned")
            .is_some();
        let busy = (self.change.lock())
            .expect("the change under way is not poisoned")
            .is_some();
        if !none || busy {
            return;
        }
        let planned = plan_now(&self.supervisor).await;
        if planned.is_ok() {
            log("the definitions plan, so the units are found again against the plan");
        }
        let no_plan = match planned {
            Ok(plan) => {
                found_against(&self.supervisor, plan).await;
                None
            }
            Err(no_plan) => Some(no_plan),
        };
        *self
            .no_plan
            .lock()
            .expect("why there is no plan is not poisoned") = no_plan;
    }
}

/// The plan the daemon finds its units against, made now, on the blocking
/// pool; why there is none, when there is none.
async fn plan_now(supervisor: &Supervisor) -> Result<Plan, NoPlan> {
    let environment = Arc::clone(&supervisor.environment);
    let planning = move || {
        let layout = &environment.layout;
        let asked = Asked::read(layout).map_err(|error| NoPlan::Problem {
            problem: error.to_string(),
        })?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let options = options(asked.asking(Asked::default()), home);
        planned(layout.root(), &options).map_err(NoPlan::Errors)
    };
    tokio::task::spawn_blocking(planning)
        .await
        .unwrap_or_else(|error| {
            Err(NoPlan::Problem {
                problem: format!("tay failed while planning the stack: {error}"),
            })
        })
}

/// Finds the units again against `plan`: those of the plan, each as it is
/// found, and each that runs that the plan does not have. What runs is
/// looked after again whether or not the environment is meant to be up,
/// and what is made is checked when it is.
async fn found_against(supervisor: &Arc<Supervisor>, plan: Plan) {
    if plan.uses_host_listen {
        supervisor.learn_host_listen(Occasion::FindingAgain).await;
    }
    let production = plan.production;
    let plan: Vec<Arc<PlannedUnit>> = plan.units.into_iter().map(Arc::new).collect();
    supervisor.take_in(&plan);
    let up = match Intent::read(&supervisor.environment.layout) {
        Ok(intent) => intent == Intent::Up,
        Err(error) => {
            log(&format!("{error}; the environment is taken to be down"));
            false
        }
    };
    engine::find_again(supervisor, &plan, up).await;
    // Nothing on a server is watched.
    if up && !production {
        for problem in supervisor.watch(&plan).await {
            log(&problem);
        }
    }
    if let Err(why) = engine::find_unplanned(supervisor).await {
        log(&format!(
            "what is left of units the plan does not have cannot be told: {why}"
        ));
    }
}

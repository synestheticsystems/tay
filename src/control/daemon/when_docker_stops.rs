//! When Docker itself stops, as it does when it is upgraded or restarted:
//! every container ends with it, and none of that is a unit's doing. The
//! daemon follows Docker for as long as a unit of its plan runs in a
//! container. The moment Docker stops answering, the daemon lets go of
//! every unit in a container, which waits for Docker from then on rather
//! than have what ends counted against it, and asks Docker again until it
//! answers. Then it brings the environment up as `tay up` would, if it is
//! meant to be up, which starts each of them afresh. Its log, `tay serve`'s
//! output and `tay status` say when Docker stopped answering and when the
//! environment was brought up again. Host processes are not touched.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::change::{Change, Kind};
use super::{Daemon, log};
use crate::clock;
use crate::control::protocol::{Event, Outage, UpRequest};
use crate::disk::{Asked, Intent};
use crate::module;

/// How often Docker is asked whether it answers again.
const ASKED_AGAIN: Duration = Duration::from_secs(1);

impl Daemon {
    /// Follows Docker for as long as the daemon runs, as this module says.
    pub(super) async fn follow_docker(self: Arc<Self>) {
        let mut changes = self.supervisor.changes();
        loop {
            // Nothing is asked of Docker while no unit of the plan runs in
            // a container: a stack of host processes alone needs none.
            if !self.supervisor.runs_in_a_container() {
                if changes.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let why = module::until_containers_stop_answering().await;
            self.docker_stopped(&why.to_string());
            while module::containers_answer().await.is_err() {
                tokio::time::sleep(ASKED_AGAIN).await;
            }
            // Docker is followed again at once, while the environment is
            // brought up again, which may wait for a change under way.
            tokio::spawn(Arc::clone(&self).bring_up_again());
        }
    }

    /// Docker has stopped answering, for the reason `why` gives: every
    /// unit in a container is let go, and waits for Docker.
    fn docker_stopped(&self, why: &str) {
        let at = clock::now();
        let by = module::CONTAINERS_RUN_BY;
        let notice = format!("{by} stopped answering at {}: {why}", clock::utc(at));
        log(&notice);
        self.supervisor.containers_stopped_answering(at, why);
        *self.outage.lock().expect("the outage is not poisoned") = Some(Outage {
            of: by.to_string(),
            stopped_answering: at,
            brought_up_again: None,
        });
        if let Some(journal) = &self.journal {
            journal(Event::Notice { notice });
        }
    }

    /// Brings the environment up again, once Docker answers again, as a
    /// plain `tay up` from the daemon's home would, once no other change is
    /// under way, if it is meant to be up then.
    async fn bring_up_again(self: Arc<Self>) {
        let by = module::CONTAINERS_RUN_BY;
        let request = UpRequest {
            adds: Asked::default(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        };
        let change = loop {
            let under_way = {
                let mut under_way = self
                    .change
                    .lock()
                    .expect("the change under way is not poisoned");
                match under_way.as_ref() {
                    Some(change) => Arc::clone(change),
                    None => {
                        if self.exit.borrow().is_some() {
                            return;
                        }
                        if !matches!(Intent::read(&self.environment.layout), Ok(Intent::Up)) {
                            log(&format!(
                                "{by} answers again; the environment is not meant to be up, so it is not brought up"
                            ));
                            return;
                        }
                        let kind = Kind::Up(request.clone());
                        let change = match &self.journal {
                            Some(journal) => Change::journalled(kind, Arc::clone(journal)),
                            None => Change::new(kind),
                        };
                        *under_way = Some(Arc::clone(&change));
                        break change;
                    }
                }
            };
            under_way.ended().await;
        };
        let notice = format!("{by} answers again, so the environment is brought up again");
        log(&notice);
        if let Some(outage) = self
            .outage
            .lock()
            .expect("the outage is not poisoned")
            .as_mut()
        {
            outage.brought_up_again = Some(change.since);
        }
        if let Some(journal) = &self.journal {
            journal(Event::Notice { notice });
        }
        self.bring_up(change, request).await;
    }
}

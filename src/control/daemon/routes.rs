//! What the daemon answers under `/v1/`: who it is, each unit's state, and
//! the changes, one at a time. A second `tay up` that asks for what the one
//! under way asks for joins it; `tay down` is always accepted and cancels
//! what is under way; any other change is refused while one runs. A change
//! to what the environment remembers is made at once, while it holds the
//! place of the change under way, so that none begins meanwhile.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use http_body_util::channel::Channel;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::change::{Change, Kind, NotBegun};
use super::down::Down;
use super::{Daemon, log};
use crate::clock;
use crate::control::protocol::{
    self, Problem, Refused, Remember, Restart, RunRequest, Status, UnitStatus, UpRequest,
    WipeRequest,
};
use crate::engine;

type Body = BoxBody<Bytes, Infallible>;

/// A change being made, of whichever kind.
type Changing = std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;

/// How many events may wait to be sent to a follower that reads slowly;
/// past that, sending to it waits, and the change goes on without it.
const BEHIND: usize = 64;

pub async fn route(
    daemon: Arc<Daemon>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_string();
    // A daemon that is exiting answers nothing more: a command takes it to
    // be gone, waits for its lock, and its successor answers.
    if daemon.exit.borrow().is_some() {
        return Ok(json(
            StatusCode::SERVICE_UNAVAILABLE,
            &Problem {
                problem: "the daemon is exiting".to_string(),
            },
        ));
    }
    Ok(match (request.method().clone(), path.as_str()) {
        (Method::GET, protocol::DAEMON) => json(StatusCode::OK, &daemon.about),
        (Method::GET, protocol::STATUS) => status(&daemon).await,
        (Method::POST, protocol::UP) => match asked::<UpRequest>(request, "tay up").await {
            Ok(asked) => up(&daemon, asked),
            Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
        },
        (Method::POST, protocol::RESTART) => match asked::<Restart>(request, "tay restart").await {
            Ok(restart) => begin(&daemon, Kind::Restart(restart.clone()), |change| {
                let restarting = daemon.restarting(&restart)?;
                Ok(Arc::clone(&daemon).restart(change, restarting))
            }),
            Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
        },
        (Method::POST, protocol::RUN) => match asked::<RunRequest>(request, "tay run").await {
            Ok(run) => begin(&daemon, Kind::Run(run.clone()), |change| {
                Ok(Arc::clone(&daemon).run_action(change, run))
            }),
            Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
        },
        (Method::POST, protocol::WIPE) => match asked::<WipeRequest>(request, "tay wipe").await {
            Ok(wipe) => begin(&daemon, Kind::Wipe(wipe.clone()), |change| match wipe {
                WipeRequest::Units { units, home } => {
                    let wiping = daemon.wiping(&units, home)?;
                    Ok(Box::pin(Arc::clone(&daemon).wipe(change, wiping)) as Changing)
                }
                WipeRequest::All => Ok(Box::pin(Arc::clone(&daemon).wipe_all(change))),
            }),
            Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
        },
        (Method::POST, protocol::REMEMBER) => {
            match asked::<Remember>(request, "tay set or tay unset").await {
                Ok(remember) => remembered(&daemon, &remember),
                Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
            }
        }
        (Method::POST, protocol::DOWN) => down(&daemon),
        (Method::POST, protocol::EXIT) => {
            daemon.exit("asked to by a tay of another version");
            json(StatusCode::OK, &serde_json::json!({}))
        }
        (
            _,
            protocol::DAEMON
            | protocol::STATUS
            | protocol::UP
            | protocol::DOWN
            | protocol::EXIT
            | protocol::REMEMBER
            | protocol::RESTART
            | protocol::RUN
            | protocol::WIPE,
        ) => json(
            StatusCode::METHOD_NOT_ALLOWED,
            &Problem {
                problem: format!("`{path}` is not asked for that way"),
            },
        ),
        _ => json(
            StatusCode::NOT_FOUND,
            &Problem {
                problem: format!("there is nothing at `{path}`"),
            },
        ),
    })
}

/// What `command` asks for, as the body of its request says.
async fn asked<T: DeserializeOwned>(
    request: Request<Incoming>,
    command: &str,
) -> Result<T, Problem> {
    let body = request
        .into_body()
        .collect()
        .await
        .map_err(|error| Problem {
            problem: format!("the request could not be read: {error}"),
        })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|error| Problem {
        problem: format!("the request is not what `{command}` asks: {error}"),
    })
}

fn up(daemon: &Arc<Daemon>, asked: UpRequest) -> Response<Body> {
    let kind = Kind::Up(asked.clone());
    let mut under_way = daemon
        .change
        .lock()
        .expect("the change under way is not poisoned");
    let change = match under_way.as_ref() {
        None => {
            let change = Change::new(kind);
            *under_way = Some(Arc::clone(&change));
            let bringing = Arc::clone(daemon).bring_up(Arc::clone(&change), asked);
            tokio::spawn(then_leave_if_unneeded(Arc::clone(daemon), bringing));
            change
        }
        Some(change) if change.kind == kind => Arc::clone(change),
        Some(change) => return refused(change),
    };
    drop(under_way);
    follow(change)
}

/// Makes the change `changing`, and then leaves the daemon if it is no
/// longer needed.
async fn then_leave_if_unneeded(daemon: Arc<Daemon>, changing: impl Future<Output = ()>) {
    changing.await;
    daemon.leave_if_unneeded().await;
}

/// Begins the change `kind`, which `make` makes, unless another change is
/// under way; `make` says why it is not begun, when it is not.
fn begin<F>(
    daemon: &Arc<Daemon>,
    kind: Kind,
    make: impl FnOnce(Arc<Change>) -> Result<F, NotBegun>,
) -> Response<Body>
where
    F: Future<Output = ()> + Send + 'static,
{
    let mut under_way = daemon
        .change
        .lock()
        .expect("the change under way is not poisoned");
    if let Some(change) = under_way.as_ref() {
        return refused(change);
    }
    let change = Change::new(kind);
    let made = match make(Arc::clone(&change)) {
        Ok(made) => made,
        Err(not_begun) => {
            // A change refused is as done as one made.
            tokio::spawn(Arc::clone(daemon).leave_if_unneeded());
            return match not_begun {
                NotBegun::DoesNotFit(problem) => {
                    json(StatusCode::UNPROCESSABLE_ENTITY, &Problem { problem })
                }
                NotBegun::Refused(refused) => json(StatusCode::CONFLICT, &refused),
            };
        }
    };
    *under_way = Some(Arc::clone(&change));
    drop(under_way);
    tokio::spawn(then_leave_if_unneeded(Arc::clone(daemon), made));
    follow(change)
}

/// Changes what the environment remembers being asked for, unless another
/// change is under way.
fn remembered(daemon: &Arc<Daemon>, remember: &Remember) -> Response<Body> {
    let under_way = daemon
        .change
        .lock()
        .expect("the change under way is not poisoned");
    if let Some(change) = under_way.as_ref() {
        return refused(change);
    }
    log(&format!("change: {}", remember.describe()));
    match remember.apply(&daemon.environment.layout) {
        Ok(()) => json(StatusCode::OK, &serde_json::json!({})),
        Err(error) => json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &Problem {
                problem: error.to_string(),
            },
        ),
    }
}

fn down(daemon: &Arc<Daemon>) -> Response<Body> {
    let mut under_way = daemon
        .change
        .lock()
        .expect("the change under way is not poisoned");
    let change = match under_way.take() {
        Some(change) if change.kind == Kind::Down => change,
        before => {
            let change = Change::new(Kind::Down);
            tokio::spawn(Arc::clone(daemon).take_down(Arc::clone(&change), before, Down::Asked));
            change
        }
    };
    *under_way = Some(Arc::clone(&change));
    drop(under_way);
    follow(change)
}

/// Each unit the daemon knows, from a check made now, and after them each
/// the plan does not have of which something runs. A unit the check finds
/// in the state recorded keeps the time it took it.
async fn status(daemon: &Arc<Daemon>) -> Response<Body> {
    let units = daemon.supervisor.snapshot();
    let environment = Arc::clone(&daemon.environment);
    let host_listen = daemon.supervisor.host_listen();
    let checked = tokio::task::spawn_blocking(move || {
        let found = engine::check_now(&units, &environment, &host_listen);
        (units, found)
    })
    .await;
    let (units, mut found) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            return json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &Problem {
                    problem: format!("tay failed while checking the units: {error}"),
                },
            );
        }
    };
    found.extend(engine::check_unplanned(&daemon.supervisor).await);
    let units = found
        .into_iter()
        .map(|found| {
            let since = units
                .state(&found.unit)
                .filter(|state| state.state == found.state.state)
                .and_then(|_| units.since(&found.unit))
                .map_or_else(clock::now, clock::seconds);
            UnitStatus {
                unit: found.unit,
                state: found.state.state,
                detail: found.state.detail,
                since,
            }
        })
        .collect();
    json(StatusCode::OK, &Status { units })
}

/// The answer to a change asked for while `change` is under way.
fn refused(change: &Change) -> Response<Body> {
    json(
        StatusCode::CONFLICT,
        &Refused::AnotherChange {
            change: change.describe(),
            since: change.since,
        },
    )
}

/// An answer that is what `change` says, as it says it.
fn follow(change: Arc<Change>) -> Response<Body> {
    let (to, body) = Channel::new(BEHIND);
    tokio::spawn(async move { change.follow(to).await });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/x-ndjson")
        .body(body.boxed())
        .expect("a response of a status and a type is sound")
}

fn json(status: StatusCode, body: &impl Serialize) -> Response<Body> {
    let body = serde_json::to_vec(body).expect("an answer serializes");
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)).boxed())
        .expect("a response of a status and a type is sound")
}

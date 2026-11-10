//! What the daemon answers under `/v1/`: who it is, each unit's state, and
//! the changes, one at a time. A second `tay up` that asks for what the one
//! under way asks for joins it; `tay down` is always accepted and cancels
//! what is under way; any other change is refused while one runs.

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::channel::Channel;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;

use super::Daemon;
use super::change::{Change, Kind};
use super::down::Down;
use crate::clock;
use crate::control::protocol::{self, Problem, Refused, Status, UnitStatus, UpRequest};
use crate::engine;

type Body = BoxBody<Bytes, Infallible>;

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
        (Method::POST, protocol::UP) => match up_request(request).await {
            Ok(asked) => up(&daemon, asked),
            Err(problem) => json(StatusCode::BAD_REQUEST, &problem),
        },
        (Method::POST, protocol::DOWN) => down(&daemon),
        (Method::POST, protocol::EXIT) => {
            daemon.exit("asked to by a tay of another version");
            json(StatusCode::OK, &serde_json::json!({}))
        }
        (
            _,
            protocol::DAEMON | protocol::STATUS | protocol::UP | protocol::DOWN | protocol::EXIT,
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

async fn up_request(request: Request<Incoming>) -> Result<UpRequest, Problem> {
    let body = request
        .into_body()
        .collect()
        .await
        .map_err(|error| Problem {
            problem: format!("the request could not be read: {error}"),
        })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|error| Problem {
        problem: format!("the request is not what `tay up` asks: {error}"),
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
            tokio::spawn(Arc::clone(daemon).bring_up(Arc::clone(&change), asked));
            change
        }
        Some(change) if change.kind == kind => Arc::clone(change),
        Some(change) => {
            return json(
                StatusCode::CONFLICT,
                &Refused {
                    change: change.describe(),
                    since: change.since,
                },
            );
        }
    };
    drop(under_way);
    follow(change)
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

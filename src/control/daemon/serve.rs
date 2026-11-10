//! Serving the daemon's socket until the daemon is to exit, and watching
//! that its folder and its socket are still there.

use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::UnixListener;
use tokio::task::JoinSet;

use super::{Daemon, Socket, log, routes};
use crate::disk::Lock;

/// How often the daemon looks for its folder and its socket.
const WATCH: Duration = Duration::from_millis(500);

/// How long answers under way have to finish once the daemon is to exit:
/// the last of `tay down` among them.
const FINISH: Duration = Duration::from_secs(5);

pub async fn serve(daemon: &Arc<Daemon>, listener: UnixListener, lock: &Lock, socket: &Socket) {
    let mut exit = daemon.exit.subscribe();
    let mut connections = JoinSet::new();
    let mut watch = tokio::time::interval(WATCH);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let daemon = Arc::clone(daemon);
                    connections.spawn(async move {
                        let service = service_fn(move |request| {
                            routes::route(Arc::clone(&daemon), request)
                        });
                        if let Err(error) = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await
                        {
                            log(&format!("a connection ended badly: {error}"));
                        }
                    });
                }
                Err(error) => {
                    // Such as too many files open: trying at once would
                    // fail again at once.
                    log(&format!("a connection could not be taken: {error}"));
                    tokio::time::sleep(WATCH).await;
                }
            },
            Some(_) = connections.join_next() => {}
            _ = exit.wait_for(Option::is_some) => break,
            _ = watch.tick() => {
                if !lock.is_at(&daemon.environment.layout.daemon_lock()) {
                    daemon.environment_gone();
                } else if !socket.is_ours() {
                    daemon.exit("its socket was removed or replaced, so no command can reach it");
                }
            }
        }
    }
    if let Some(why) = exit.borrow().as_ref() {
        log(&format!("exiting: {why}"));
    }
    // A command that connects from now on finds nothing listening, and so
    // waits for the lock rather than for an answer that would not come.
    drop(listener);
    let finished = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(FINISH, finished).await.is_err() {
        log("answers still under way are cut off");
    }
}

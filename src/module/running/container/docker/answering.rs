//! Whether Docker itself answers. When Docker stops, as it does when it is
//! upgraded or restarted, every container ends with it. Docker's own stream
//! of events ends the moment it stops, also when it is back before anyone
//! could have asked it, so following that stream tells that it stopped,
//! and asking it whether it answers tells that it is back.

use std::collections::HashMap;
use std::time::Duration;

use bollard::query_parameters::EventsOptions;
use futures_util::StreamExt;

use super::{Docker, DockerError};

/// How long Docker has to say that it answers. One whose socket a service
/// manager holds while it starts takes connections and answers none until
/// it has started.
const ASKED: Duration = Duration::from_secs(5);

impl Docker {
    /// Whether Docker answers now.
    pub async fn answers(&self) -> Result<(), DockerError> {
        match tokio::time::timeout(ASKED, self.api.ping()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(self.failed("say that it answers")(error)),
            Err(_) => Err(DockerError::Unreachable {
                socket: self.socket.clone(),
                why: format!("it did not answer within {} seconds", ASKED.as_secs()),
            }),
        }
    }

    /// Follows Docker's stream of its own events until it ends, as it does
    /// the moment Docker stops; why it ended. Docker that cannot be reached
    /// has stopped already.
    pub async fn until_it_stops(&self) -> DockerError {
        // Events of Docker itself alone, which are few: the stream is
        // followed for its end.
        let filters = HashMap::from([("type".to_string(), vec!["daemon".to_string()])]);
        let options = EventsOptions {
            filters: Some(filters),
            ..EventsOptions::default()
        };
        let mut events = self.api.events(Some(options));
        loop {
            match events.next().await {
                // Docker reloading its configuration goes on answering.
                Some(Ok(_)) => {}
                Some(Err(error)) => return self.failed("go on telling its events")(error),
                None => {
                    return DockerError::Other(
                        "Docker ended the stream of its events, as it does when it stops"
                            .to_string(),
                    );
                }
            }
        }
    }
}

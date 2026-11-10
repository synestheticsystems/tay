//! A container's readiness, when its unit writes no `ready`: its image's
//! own health check, or having started when the image has none, judged for
//! as long as it runs; and a `command` check, run inside it.

use std::convert::Infallible;
use std::time::Duration;

use bollard::models::{ContainerInspectResponse, HealthStatusEnum};

use crate::module::operations::{Step, Steps, UnitPlace};
use crate::module::running::command_line::shown;
use crate::module::running::container::container_name;
use crate::module::running::container::docker::{Docker, DockerError, Exec, Exited, OurContainer};

/// How often a container's own health is looked at until it passes, and
/// after.
const JUDGE_STARTING: Duration = Duration::from_millis(500);
const JUDGE_READY: Duration = Duration::from_secs(5);

/// The longest a health check's last words are said.
const SAID: usize = 120;

/// Judges the container's readiness by its image's own health check, for
/// as long as it runs, telling `steps` each change; or once, by its having
/// started, when its image has none.
pub(super) async fn judge(docker: &Docker, container: &OurContainer, steps: &Steps) -> Infallible {
    let mut said: Option<Result<String, String>> = None;
    loop {
        let judged = match docker.inspect(container).await {
            Ok(Some(inspected)) => health(&inspected),
            // The output ends with the container, and is waited for.
            Ok(None) => std::future::pending().await,
            Err(error) => Some(Err(format!("its health cannot be told: {error}"))),
        };
        let Some(judged) = judged else {
            let _ = steps.send(Step::Passes(
                "running: its image has no health check".to_string(),
            ));
            return std::future::pending().await;
        };
        let passes = judged.is_ok();
        if said.as_ref() != Some(&judged) {
            let _ = steps.send(match &judged {
                Ok(words) => Step::Passes(words.clone()),
                Err(why) => Step::Fails(why.clone()),
            });
            said = Some(judged);
        }
        tokio::time::sleep(if passes { JUDGE_READY } else { JUDGE_STARTING }).await;
    }
}

/// What the image's own health check says of the container now; `None`
/// when the image has none.
fn health(inspected: &ContainerInspectResponse) -> Option<Result<String, String>> {
    let health = inspected.state.as_ref()?.health.as_ref()?;
    Some(match health.status {
        Some(HealthStatusEnum::HEALTHY) => Ok("its image's health check passes".to_string()),
        Some(HealthStatusEnum::UNHEALTHY) => {
            let last = health
                .log
                .as_ref()
                .and_then(|log| log.last())
                .and_then(|result| result.output.as_deref())
                .map(|output| {
                    output
                        .trim()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(SAID)
                        .collect::<String>()
                })
                .filter(|output| !output.is_empty());
            Err(match last {
                Some(last) => format!("its image's health check fails: {last}"),
                None => "its image's health check fails".to_string(),
            })
        }
        Some(HealthStatusEnum::NONE) | None => return None,
        Some(HealthStatusEnum::STARTING | HealthStatusEnum::EMPTY) => {
            Err("its image's health check has not passed yet".to_string())
        }
    })
}

/// Runs the readiness command `command` inside the running container of
/// `place`'s unit, for as long as `limit`; why it did not pass. A check
/// that does not end in time goes on in the container: Docker can stop it
/// only by stopping the container.
pub async fn check_inside(
    place: &UnitPlace,
    command: &[String],
    limit: Duration,
) -> Result<(), String> {
    let shown = shown(command);
    let checked = async {
        let docker = Docker::connect()?;
        let found = docker
            .container(place.environment.id.as_str(), &container_name(place))
            .await?;
        let Some((container, _)) = found else {
            return Err(DockerError::Other(format!(
                "the container of `{}` is not there",
                place.unit
            )));
        };
        let exec = Exec {
            argv: command,
            env: &[],
            secrets: &[],
            user: None,
            workdir: None,
            stdin: None,
        };
        docker.exec(&container, &exec, &mut |_| {}).await
    };
    match tokio::time::timeout(limit, checked).await {
        Err(_) => Err(format!(
            "{shown} did not end within {} seconds",
            limit.as_secs()
        )),
        Ok(Ok(Exited { code: 0 })) => Ok(()),
        Ok(Ok(Exited { code })) => Err(format!("{shown} exited with {code}")),
        Ok(Err(error)) => Err(error.to_string()),
    }
}

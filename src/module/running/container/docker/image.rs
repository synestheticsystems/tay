//! A container's image: pulled if it is not here, or built. Either says how
//! far it has come while it runs. A pull is sent with the sign-in the
//! account tay runs as has given Docker for the image's registry, when there
//! is one ([`sign_in`]). An image tay builds carries the labels of the
//! environment and the unit it is built for, as everything tay creates
//! does, so that it is found by them, and removed with what the
//! environment keeps.
//!
//! An image is built by Docker's own command line, `docker build`, pointed
//! at the same socket, because Docker's API builds as the command line does
//! only with a session of its own that the command line holds open: without
//! it, the API's builder reads no `.dockerignore` and knows none of the
//! features of today's builder, so a Dockerfile that builds by hand would not
//! build under tay. What a build is given holds no secret: the planner
//! refuses one in a build's settings.

mod sign_in;

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};

use bollard::query_parameters::{
    CreateImageOptionsBuilder, ListImagesOptionsBuilder, RemoveImageOptions,
};

use super::ours::{ENVIRONMENT, Owner, is_ours};
use super::{Docker, DockerError, missing};
use sign_in::SignIn;

/// How a pull or a build is told to say how far it has come, in words a
/// person reads after "pulling" or "building".
pub type Progress<'a> = &'a mut (dyn FnMut(String) + Send);

/// The longest step of a build said in the unit's detail.
const STEP: usize = 72;

/// What `docker build` is given.
pub struct Build<'a> {
    /// Whom the image is built for, whose labels it carries.
    pub owner: Owner<'a>,
    /// The image's name and tag.
    pub tag: &'a str,
    pub context: &'a Path,
    pub dockerfile: Option<&'a str>,
    pub args: &'a [(String, String)],
    pub platform: Option<&'a str>,
}

impl Docker {
    /// The id of the image `name` here; `None` when it is not here.
    pub async fn image_id(&self, name: &str) -> Result<Option<String>, DockerError> {
        match self.api.inspect_image(name).await {
            Ok(image) => Ok(image.id),
            Err(error) if missing(&error) => Ok(None),
            Err(error) => Err(self.failed(format!("look at the image `{name}`"))(error)),
        }
    }

    /// Pulls `name`, for `platform` when one is named, with the account's
    /// sign-in to its registry when there is one, telling `progress` each
    /// time another of its layers is here.
    pub async fn pull(
        &self,
        name: &str,
        platform: Option<&str>,
        progress: Progress<'_>,
    ) -> Result<(), DockerError> {
        let mut options = CreateImageOptionsBuilder::default().from_image(name);
        if let Some(platform) = platform {
            options = options.platform(platform);
        }
        progress(format!("pulling `{name}`"));
        let sign_in = SignIn::for_image(name).await;
        let mut pulling = self
            .api
            .create_image(Some(options.build()), None, sign_in.credentials());
        let mut layers = Layers::default();
        while let Some(said) = pulling.next().await {
            let said = said
                .map_err(|error| sign_in.refused(self.failed(format!("pull `{name}`"))(error)))?;
            if let (Some(layer), Some(status)) = (said.id, said.status)
                && layers.saw(layer, &status)
            {
                progress(format!(
                    "pulling `{name}`: {} of {} layers",
                    layers.here,
                    layers.seen.len()
                ));
            }
        }
        Ok(())
    }

    /// Builds `build` with `docker build`, telling `progress` each step it
    /// comes to and `line` each line it prints. Why it failed, with the
    /// last thing it said, when it did.
    pub async fn build(
        &self,
        build: &Build<'_>,
        progress: Progress<'_>,
        line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(), DockerError> {
        let failed = |why: String| DockerError::Refused {
            doing: format!("build `{}`", build.tag),
            why,
        };
        let mut command = tokio::process::Command::new("docker");
        // Today's builder is told to say each step on a line of its own
        // through its environment, which the builder before it ignores.
        command
            .args(["build", "--tag", build.tag])
            .env("BUILDKIT_PROGRESS", "plain")
            .env("DOCKER_HOST", format!("unix://{}", self.socket.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (label, value) in build.owner.labels() {
            command.arg("--label").arg(format!("{label}={value}"));
        }
        if let Some(dockerfile) = build.dockerfile {
            command.arg("--file").arg(build.context.join(dockerfile));
        }
        for (name, value) in build.args {
            command.arg("--build-arg").arg(format!("{name}={value}"));
        }
        if let Some(platform) = build.platform {
            command.args(["--platform", platform]);
        }
        command.arg(build.context);
        let mut child = command
            .spawn()
            .map_err(|error| failed(format!("`docker` cannot be started: {error}")))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(failed(
                "what `docker build` says cannot be read".to_string(),
            ));
        };
        // Today's builder says how it goes on its standard error, the one
        // before it on its standard output.
        let mut out = BufReader::new(stdout).lines();
        let mut err = BufReader::new(stderr).lines();
        let (mut out_open, mut err_open) = (true, true);
        let mut last = String::new();
        let unread = |error: std::io::Error| {
            failed(format!("what `docker build` says cannot be read: {error}"))
        };
        progress(format!("building `{}`", build.tag));
        while out_open || err_open {
            let said = tokio::select! {
                said = out.next_line(), if out_open => match said.map_err(unread)? {
                    Some(said) => said,
                    None => {
                        out_open = false;
                        continue;
                    }
                },
                said = err.next_line(), if err_open => match said.map_err(unread)? {
                    Some(said) => said,
                    None => {
                        err_open = false;
                        continue;
                    }
                },
            };
            line(&said);
            if let Some(step) = step(&said) {
                progress(format!("building `{}`: {step}", build.tag));
            }
            if !said.trim().is_empty() {
                last = said;
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|error| failed(format!("`docker build` cannot be waited for: {error}")))?;
        match status.success() {
            true => Ok(()),
            false => Err(failed(format!("`docker build` {status}: {}", last.trim()))),
        }
    }
}

/// An image built for an environment: one Docker says carries its label.
pub struct OurImage {
    id: String,
    /// What a person knows it by: its tag, or its id once a later build
    /// has taken the tag.
    pub name: String,
}

impl Docker {
    /// The images built for `environment`.
    pub async fn images(&self, environment: &str) -> Result<Vec<OurImage>, DockerError> {
        let filters = HashMap::from([("label", vec![format!("{ENVIRONMENT}={environment}")])]);
        let options = ListImagesOptionsBuilder::default()
            .filters(&filters)
            .build();
        let listed = self
            .api
            .list_images(Some(options))
            .await
            .map_err(self.failed("list the environment's images"))?;
        Ok(listed
            .into_iter()
            .filter(|image| is_ours(Some(&image.labels), environment))
            .map(|image| OurImage {
                name: (image.repo_tags.iter())
                    .find(|tag| *tag != "<none>:<none>")
                    .unwrap_or(&image.id)
                    .clone(),
                id: image.id,
            })
            .collect())
    }

    /// Removes `image`; one that is gone already is. Docker refuses one
    /// that someone has given another tag as well, which tay leaves to
    /// them.
    pub async fn remove_image(&self, image: &OurImage) -> Result<(), DockerError> {
        match self
            .api
            .remove_image(&image.id, None::<RemoveImageOptions>, None)
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if missing(&error) => Ok(()),
            Err(error) => {
                let doing = format!("remove the image `{}`", image.name);
                Err(self.failed(doing)(error))
            }
        }
    }
}

/// The layers of a pull, as Docker says how each stands.
#[derive(Default)]
struct Layers {
    seen: Vec<String>,
    here: usize,
    done: Vec<String>,
}

impl Layers {
    /// Takes in that `layer` is at `status`; whether another layer is here
    /// now.
    fn saw(&mut self, layer: String, status: &str) -> bool {
        // What Docker says of the image as a whole, rather than of a layer,
        // carries the image's name or tag as its id.
        let of_a_layer = matches!(
            status,
            "Pulling fs layer"
                | "Waiting"
                | "Downloading"
                | "Verifying Checksum"
                | "Download complete"
                | "Extracting"
                | "Pull complete"
                | "Already exists"
        );
        if !of_a_layer {
            return false;
        }
        if !self.seen.contains(&layer) {
            self.seen.push(layer.clone());
        }
        let here = matches!(status, "Pull complete" | "Already exists");
        if !here || self.done.contains(&layer) {
            return false;
        }
        self.done.push(layer);
        self.here += 1;
        true
    }
}

/// The step a line of `docker build` begins, as it names it: today's
/// builder's `#5 [2/3] RUN make`, or the builder before it's
/// `Step 2/3 : RUN make`, gives `[2/3] RUN make`.
fn step(line: &str) -> Option<String> {
    let rest = match line.strip_prefix("Step ") {
        Some(legacy) => {
            let (count, step) = legacy.split_once(" : ")?;
            format!("[{count}] {step}")
        }
        None => {
            let (number, rest) = line.strip_prefix('#')?.split_once(' ')?;
            if !number.chars().all(|c| c.is_ascii_digit()) || !rest.starts_with('[') {
                return None;
            }
            rest.to_string()
        }
    };
    let mut step: String = rest.chars().take(STEP).collect();
    if rest.chars().count() > STEP {
        step.push('…');
    }
    Some(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_says_each_layer_that_is_here_once() {
        let mut layers = Layers::default();
        let said = [
            ("1.36", "Pulling from library/busybox"),
            ("a", "Pulling fs layer"),
            ("b", "Pulling fs layer"),
            ("a", "Downloading"),
            ("a", "Pull complete"),
            ("a", "Pull complete"),
            ("b", "Already exists"),
        ];
        let mut told = Vec::new();
        for (layer, status) in said {
            if layers.saw(layer.to_string(), status) {
                told.push((layers.here, layers.seen.len()));
            }
        }
        assert_eq!(told, [(1, 2), (2, 2)]);
    }

    #[test]
    fn a_builds_step_is_the_line_that_begins_it() {
        assert_eq!(step("#5 [2/3] RUN make").as_deref(), Some("[2/3] RUN make"));
        assert_eq!(
            step("Step 2/3 : RUN make").as_deref(),
            Some("[2/3] RUN make")
        );
        assert_eq!(step("#5 DONE 0.1s"), None);
        assert_eq!(step("#5 0.311 compiling"), None);
        let long = format!("#7 [1/1] RUN {}", "x".repeat(100));
        assert_eq!(step(&long).unwrap().chars().count(), STEP + 1);
    }
}

//! A program run inside a container that runs: a readiness check, or a
//! command run `in` a unit's container. Its secrets reach it through its
//! environment, never its arguments. And a container's own program fed on
//! its standard input, as a command's container of its own is.

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use std::pin::Pin;

use tokio::io::AsyncWrite;

use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::query_parameters::AttachContainerOptionsBuilder;

use crate::module::running::container::docker::ours::OurContainer;
use crate::module::running::container::docker::{Docker, DockerError, without_secrets};

/// What is run inside a container.
pub struct Exec<'a> {
    pub argv: &'a [String],
    /// `NAME=value`, as Docker takes them.
    pub env: &'a [String],
    /// The values in `env` that are secret, which nothing Docker says back
    /// is said with.
    pub secrets: &'a [String],
    pub user: Option<&'a str>,
    pub workdir: Option<&'a str>,
    /// What it is fed on its standard input, which is then closed.
    pub stdin: Option<&'a [u8]>,
}

/// How a program run inside a container ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exited {
    pub code: i64,
}

impl Docker {
    /// Runs `exec` inside `container` and waits for it to end, giving each
    /// line it prints to `line` as it prints it.
    pub async fn exec(
        &self,
        container: &OurContainer,
        exec: &Exec<'_>,
        line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Exited, DockerError> {
        let shown = exec.argv.join(" ");
        let failed = |error| {
            let failed = self.failed(format!(
                "run `{shown}` in the container of `{}`",
                container.unit()
            ))(error);
            without_secrets(failed, exec.secrets)
        };
        let options = CreateExecOptions {
            attach_stdin: Some(exec.stdin.is_some()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            env: Some(exec.env.to_vec()),
            cmd: Some(exec.argv.to_vec()),
            user: exec.user.map(str::to_string),
            working_dir: exec.workdir.map(str::to_string),
            ..CreateExecOptions::default()
        };
        let created = self
            .api
            .create_exec(&container.id, options)
            .await
            .map_err(failed)?;
        let started = self
            .api
            .start_exec(&created.id, None)
            .await
            .map_err(failed)?;
        let StartExecResults::Attached {
            mut output,
            mut input,
        } = started
        else {
            return Err(DockerError::Other(format!(
                "Docker ran `{shown}` without letting tay read what it prints"
            )));
        };
        if let Some(stdin) = exec.stdin {
            // A program that ends before it reads all it is fed closes its
            // end, which is how it ends, not a failure of the feeding.
            let _ = input.write_all(stdin).await;
            let _ = input.shutdown().await;
        }
        drop(input);
        let mut lines = Lines::default();
        while let Some(said) = output.next().await {
            match said.map_err(failed)? {
                LogOutput::StdOut { message }
                | LogOutput::StdErr { message }
                | LogOutput::Console { message } => lines.add(&message, line),
                LogOutput::StdIn { .. } => {}
            }
        }
        lines.end(line);
        let inspected = self
            .api
            .inspect_exec(&created.id)
            .await
            .map_err(self.failed(format!("learn how `{shown}` ended")))?;
        match inspected.exit_code {
            Some(code) => Ok(Exited { code }),
            None => Err(DockerError::Other(format!(
                "Docker did not say how `{shown}` ended"
            ))),
        }
    }
}

/// A container's standard input, held from before it starts, to be fed
/// once and closed.
pub struct Feeding {
    input: Pin<Box<dyn AsyncWrite + Send>>,
}

impl Feeding {
    /// Feeds `bytes`, then closes the input. A program that ends before it
    /// reads all it is fed closes its end, which is how it ends, not a
    /// failure of the feeding.
    pub async fn feed(mut self, bytes: &[u8]) {
        let _ = self.input.write_all(bytes).await;
        let _ = self.input.shutdown().await;
    }
}

impl Docker {
    /// The standard input of `container`, which was created to be fed, held
    /// open until it is fed.
    pub async fn feeding(&self, container: &OurContainer) -> Result<Feeding, DockerError> {
        let options = AttachContainerOptionsBuilder::default()
            .stdin(true)
            .stream(true)
            .build();
        let attached = self
            .api
            .attach_container(&container.id, Some(options))
            .await
            .map_err(self.failed(format!("feed the container of `{}`", container.unit())))?;
        Ok(Feeding {
            input: attached.input,
        })
    }
}

/// What a program prints, cut into lines as it comes.
#[derive(Default)]
struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    fn add(&mut self, bytes: &[u8], line: &mut (dyn FnMut(&str) + Send)) {
        self.partial.extend_from_slice(bytes);
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let text: Vec<u8> = self.partial.drain(..=end).collect();
            line(String::from_utf8_lossy(&text[..end]).trim_end_matches('\r'));
        }
    }

    /// What is left once the program has ended: a last line with no end.
    fn end(self, line: &mut (dyn FnMut(&str) + Send)) {
        if !self.partial.is_empty() {
            line(&String::from_utf8_lossy(&self.partial));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_printed_is_cut_into_lines_whatever_pieces_it_comes_in() {
        let mut lines = Lines::default();
        let mut seen = Vec::new();
        for piece in ["one\ntw", "o\r\n", "", "three"] {
            lines.add(piece.as_bytes(), &mut |line| seen.push(line.to_string()));
        }
        lines.end(&mut |line| seen.push(line.to_string()));
        assert_eq!(seen, ["one", "two", "three"]);
    }
}

//! A file fetched from where a command ran in a container: the one its
//! `TAY_VALUES` names. Docker gives a container's file as a tar archive,
//! which holds it as the one file in it, after any entry that only
//! describes the next.

use futures_util::StreamExt;

use bollard::query_parameters::DownloadFromContainerOptionsBuilder;

use crate::module::running::container::docker::ours::OurContainer;
use crate::module::running::container::docker::{Docker, DockerError, missing};

/// The size of a tar archive's blocks, in which its headers and each
/// file's text are laid out.
const BLOCK: usize = 512;

/// The longest file of values tay reads.
const LONGEST: usize = 1 << 20;

impl Docker {
    /// The text of the file at `path` in `container`, running or ended;
    /// `None` when there is none.
    pub async fn read_file(
        &self,
        container: &OurContainer,
        path: &str,
    ) -> Result<Option<Vec<u8>>, DockerError> {
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        let mut archive = Vec::new();
        let mut downloading = self
            .api
            .download_from_container(&container.id, Some(options));
        while let Some(bytes) = downloading.next().await {
            match bytes {
                Ok(bytes) => archive.extend_from_slice(&bytes),
                Err(error) if missing(&error) => return Ok(None),
                Err(error) => {
                    return Err(self.failed(format!(
                        "fetch `{path}` from the container of `{}`",
                        container.unit()
                    ))(error));
                }
            }
            if archive.len() > LONGEST + 4 * BLOCK {
                return Err(DockerError::Other(format!(
                    "`{path}` in the container of `{}` is longer than the {LONGEST} bytes tay reads of it",
                    container.unit()
                )));
            }
        }
        first_file(&archive).map(Some).ok_or_else(|| {
            DockerError::Other(format!("what Docker gave of `{path}` is not a file"))
        })
    }
}

/// The text of the first regular file in a tar archive.
fn first_file(archive: &[u8]) -> Option<Vec<u8>> {
    let mut at = 0;
    while at + BLOCK <= archive.len() {
        let header = &archive[at..at + BLOCK];
        if header.iter().all(|byte| *byte == 0) {
            return None;
        }
        let size = size(&header[124..136])?;
        let start = at + BLOCK;
        let end = start.checked_add(size)?;
        match header[156] {
            b'0' | 0 => return archive.get(start..end).map(<[u8]>::to_vec),
            // An entry that describes the next, or anything that is not a
            // file, is passed over.
            _ => at = start + size.div_ceil(BLOCK) * BLOCK,
        }
    }
    None
}

/// A size in a tar header: octal digits ended by a space or nothing.
fn size(field: &[u8]) -> Option<usize> {
    let digits: Vec<u8> = field
        .iter()
        .copied()
        .skip_while(|byte| *byte == b' ')
        .take_while(|byte| (b'0'..=b'7').contains(byte))
        .collect();
    usize::from_str_radix(std::str::from_utf8(&digits).ok()?, 8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry of a tar archive: a header of `kind` and `text`, padded.
    fn entry(name: &str, kind: u8, text: &[u8]) -> Vec<u8> {
        let mut header = vec![0u8; BLOCK];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{:011o}\0", text.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = kind;
        let mut entry = header;
        entry.extend_from_slice(text);
        entry.resize(entry.len().div_ceil(BLOCK) * BLOCK, 0);
        entry
    }

    #[test]
    fn the_first_file_is_read_past_an_entry_that_describes_it() {
        let mut archive = entry("PaxHeaders/values.json", b'x', b"30 mtime=1794310984.859\n");
        archive.extend(entry("values.json", b'0', br#"{"token":"abc"}"#));
        archive.extend(vec![0u8; 2 * BLOCK]);
        assert_eq!(first_file(&archive).unwrap(), br#"{"token":"abc"}"#);
        assert_eq!(first_file(&entry("folder/", b'5', b"")), None);
        assert_eq!(first_file(b"short"), None);
    }
}

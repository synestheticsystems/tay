//! Writing to standard output and standard error. A reader that closes its
//! end, as `head` does once it has read enough, ends the printing quietly:
//! nothing more is written to it, and the command carries on.

use std::io::{self, Write};

/// Writes `text` to standard output.
pub fn out(text: &str) {
    if let Err(error) = write_all(&mut io::stdout().lock(), text)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        err(&format!("tay: the output could not be written: {error}\n"));
    }
}

/// Writes `text` to standard error. When it cannot be written there is
/// nowhere left to say so.
pub fn err(text: &str) {
    let _unwritable = write_all(&mut io::stderr().lock(), text);
}

fn write_all(stream: &mut impl Write, text: &str) -> io::Result<()> {
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

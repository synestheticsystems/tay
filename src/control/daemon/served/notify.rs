//! Telling systemd that `tay serve` answers, so that `systemctl start`
//! returns only then, and a `tay up` run right after it joins the bring-up
//! rather than start a daemon of its own. For a unit of `Type=notify`,
//! systemd names a datagram socket in `NOTIFY_SOCKET`, and takes `READY=1`
//! sent to it as the service being ready. With no `NOTIFY_SOCKET`, no
//! service manager waits for the notice, and none is sent.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

/// What systemd takes as the service being ready.
const READY: &[u8] = b"READY=1";

/// Tells the service manager that waits for it, if one does, that the
/// daemon answers.
pub fn ready() -> io::Result<()> {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET").filter(|socket| !socket.is_empty()) else {
        return Ok(());
    };
    let datagram = UnixDatagram::unbound()?;
    match socket.as_bytes().strip_prefix(b"@") {
        Some(name) => send_to_abstract(&datagram, name),
        None => datagram.send_to(READY, Path::new(&socket)).map(drop),
    }
}

/// Sends the notice to the socket of the abstract name `name`, which
/// systemd may give in place of a path on Linux.
#[cfg(target_os = "linux")]
fn send_to_abstract(datagram: &UnixDatagram, name: &[u8]) -> io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
    datagram.send_to_addr(READY, &address).map(drop)
}

/// Only Linux has sockets of abstract names.
#[cfg(not(target_os = "linux"))]
fn send_to_abstract(_: &UnixDatagram, _: &[u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "`NOTIFY_SOCKET` names a socket by an abstract name, which only Linux has",
    ))
}

//! Whether a port number is free on this machine: no program listens on it,
//! on any address. A socket that holds the number and listens on nothing,
//! as the side of a connection that a program closed first leaves for a
//! while in TIME_WAIT, holds nothing: a program that listens on the number
//! next is let in, as a unit started again on its own number is.
//!
//! First every address of a family is bound at once without
//! `SO_REUSEADDR`, which the system refuses while any socket has the number
//! on any address of it: when nothing has it, it is free. When something
//! does, the system is asked whether that is a program listening. Linux
//! says so in its tables of sockets. macOS gives its table only as its
//! kernel's own structures, so there each address the number could be
//! listened on is bound with `SO_REUSEADDR`, which macOS lets through
//! beside a closed connection and refuses beside a program that listens on
//! that very address: the address that holds every other, and each address
//! of the machine. macOS also refuses it, for a few milliseconds, beside a
//! socket that has only just been closed, one of an earlier asking among
//! them, so a refusal counts only once it has outlasted that. Linux, which
//! refuses that bind beside a closed connection whose program did not set
//! the option, could not be asked so.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

use rustix::io::Errno;
use rustix::net::{AddressFamily, SocketType, bind, socket, sockopt};

/// Whether no program on this machine listens on `number`.
pub fn is_free(number: u16) -> io::Result<bool> {
    Ok(nothing_has(number)? || !listened_on(number)?)
}

/// Whether no socket on this machine has `number`. Each test binds every
/// address of a family at once without `SO_REUSEADDR`, which the system
/// refuses while any socket holds the number on any address of it: on
/// macOS a bind with that option would be let through beside a program
/// listening on one address only.
fn nothing_has(number: u16) -> io::Result<bool> {
    let ipv4 = socket(AddressFamily::INET, SocketType::STREAM, None)?;
    match bind(&ipv4, &SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, number)) {
        Ok(()) => {}
        Err(Errno::ADDRINUSE) => return Ok(false),
        Err(errno) => return Err(errno.into()),
    }
    let ipv6 = match socket(AddressFamily::INET6, SocketType::STREAM, None) {
        Ok(ipv6) => ipv6,
        // A machine with no IPv6 has nothing listening on it.
        Err(Errno::AFNOSUPPORT) => return Ok(true),
        Err(errno) => return Err(errno.into()),
    };
    sockopt::set_ipv6_v6only(&ipv6, true)?;
    match bind(
        &ipv6,
        &SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, number, 0, 0),
    ) {
        Ok(()) | Err(Errno::ADDRNOTAVAIL) => Ok(true),
        Err(Errno::ADDRINUSE) => Ok(false),
        Err(errno) => Err(errno.into()),
    }
}

/// Whether a program listens on `number`, as the system's tables of TCP
/// sockets say: a line whose local address ends in the number, in
/// hexadecimal, and whose state is `0A`, listening.
#[cfg(not(target_os = "macos"))]
fn listened_on(number: u16) -> io::Result<bool> {
    let listens = |line: &str| {
        let mut fields = line.split_whitespace();
        let port = (fields.nth(1))
            .and_then(|local| local.rsplit_once(':'))
            .and_then(|(_, port)| u16::from_str_radix(port, 16).ok());
        port == Some(number) && fields.nth(1) == Some("0A")
    };
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let text = match std::fs::read_to_string(table) {
            Ok(text) => text,
            // A machine with no IPv6 has no table of it.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if text.lines().skip(1).any(listens) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// How often an address that refuses to be bound is asked again, and how
/// long apart, before the refusal is taken for a program listening. A
/// socket that was only just closed has been seen to refuse for up to six
/// milliseconds; a program that listens refuses every time.
#[cfg(target_os = "macos")]
const ASKED_AGAIN: u32 = 5;
#[cfg(target_os = "macos")]
const SETTLE: std::time::Duration = std::time::Duration::from_millis(10);

/// Whether a program listens on `number` at an address of this machine,
/// or at the address that holds every other: each is bound with
/// `SO_REUSEADDR`, which is refused there alone, and is refused still when
/// asked again. An address that cannot be bound at all, as one still being
/// set up, has nothing listening on it.
#[cfg(target_os = "macos")]
fn listened_on(number: u16) -> io::Result<bool> {
    use std::net::SocketAddr;

    let every = [
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, number)),
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, number, 0, 0)),
    ];
    for mut address in every.into_iter().chain(addresses()?) {
        address.set_port(number);
        let mut refused = 0;
        while refuses(&address)? {
            if refused == ASKED_AGAIN {
                return Ok(true);
            }
            refused += 1;
            std::thread::sleep(SETTLE);
        }
    }
    Ok(false)
}

/// Whether `address` refuses, as held, a socket bound to it with
/// `SO_REUSEADDR`.
#[cfg(target_os = "macos")]
fn refuses(address: &std::net::SocketAddr) -> io::Result<bool> {
    let family = match address {
        std::net::SocketAddr::V4(_) => AddressFamily::INET,
        std::net::SocketAddr::V6(_) => AddressFamily::INET6,
    };
    let probe = socket(family, SocketType::STREAM, None)?;
    sockopt::set_socket_reuseaddr(&probe, true)?;
    if address.is_ipv6() {
        sockopt::set_ipv6_v6only(&probe, true)?;
    }
    match bind(&probe, address) {
        Ok(()) | Err(Errno::ADDRNOTAVAIL) => Ok(false),
        Err(Errno::ADDRINUSE) => Ok(true),
        Err(errno) => Err(errno.into()),
    }
}

/// Each address of this machine's network interfaces, with no port.
#[cfg(target_os = "macos")]
fn addresses() -> io::Result<Vec<std::net::SocketAddr>> {
    use std::net::SocketAddr;

    let mut first: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` writes to `first` a list it allocates, which is
    // freed below and not read after.
    if unsafe { libc::getifaddrs(&mut first) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addresses = Vec::new();
    let mut next = first;
    while !next.is_null() {
        // SAFETY: each entry of the list is valid until the list is freed.
        let entry = unsafe { &*next };
        next = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: an entry's address is a socket address whose family says
        // which kind it is, and so how much of it there is to read.
        let address = unsafe {
            match i32::from((*entry.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let address = &*entry.ifa_addr.cast::<libc::sockaddr_in>();
                    let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
                    SocketAddr::V4(SocketAddrV4::new(ip, 0))
                }
                libc::AF_INET6 => {
                    let address = &*entry.ifa_addr.cast::<libc::sockaddr_in6>();
                    let ip = Ipv6Addr::from(address.sin6_addr.s6_addr);
                    SocketAddr::V6(SocketAddrV6::new(ip, 0, 0, address.sin6_scope_id))
                }
                _ => continue,
            }
        };
        addresses.push(address);
    }
    // SAFETY: the list was allocated by `getifaddrs`, and is not read after.
    unsafe { libc::freeifaddrs(first) };
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream, UdpSocket};

    #[test]
    fn a_number_a_program_listens_on_is_not_free_on_any_one_address() {
        // A program on the loopback address alone, or on every address.
        for address in ["127.0.0.1:0", "0.0.0.0:0", "[::1]:0"] {
            let Ok(listener) = TcpListener::bind(address) else {
                continue;
            };
            let number = listener.local_addr().unwrap().port();
            assert!(!is_free(number).unwrap(), "{address}");
            drop(listener);
        }
    }

    #[test]
    fn a_number_a_program_listens_on_at_another_address_of_this_machine_is_not_free() {
        // The address this machine would reach the world from, which needs
        // no packet sent to find; a machine with no route has none.
        let outward = UdpSocket::bind("0.0.0.0:0").unwrap();
        if outward.connect("192.0.2.1:9").is_err() {
            return;
        }
        let address = outward.local_addr().unwrap().ip();
        let listener = TcpListener::bind((address, 0)).unwrap();
        let number = listener.local_addr().unwrap().port();
        assert!(!is_free(number).unwrap(), "{address}");
    }

    #[test]
    fn a_number_whose_program_closed_its_connection_and_stopped_listening_is_free() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let number = listener.local_addr().unwrap().port();
        let mut client = TcpStream::connect(("127.0.0.1", number)).unwrap();
        let (served, _) = listener.accept().unwrap();
        // The program's side closes first, as that of a program that is
        // stopped does, and is left holding the number for a while.
        drop(served);
        drop(listener);
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).unwrap();
        drop(client);
        assert!(!nothing_has(number).unwrap());
        // However often it is asked: each asking closes sockets of its own
        // on the number, which the next must not take for a program.
        for asked in 0..50 {
            assert!(is_free(number).unwrap(), "asked {asked} times before");
        }
    }
}

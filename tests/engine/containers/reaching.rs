//! The host and its containers reaching each other, each by the address
//! the plan gave it: a container reaches a host process by the one name the
//! host has in containers, where the process listens on `${host.listen}`;
//! a host process reaches a container through the port the plan opens.

use std::time::Duration;

use crate::harness::{Machine, eventually, log};

#[test]
fn a_container_reaches_a_host_process_and_a_host_process_reaches_a_container() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    // The address a container is given for the host's page is the host's
    // one name in containers.
    let plan = machine
        .at(&root, "plan", &["--set", "up=fetcher,fetching", "--json"])
        .json();
    let page = crate::support::given(crate::support::unit(&plan["plan"], "host-page"), "page");
    let seen = page["from_containers"].to_string();
    assert!(seen.contains("http://host.docker.internal:"), "{seen}");
    let up = machine.at(&root, "up", &["fetcher", "fetching"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        eventually(Duration::from_secs(20), || log(&root, "fetcher")
            .contains("| <p>hello from the host</p>")),
        "{}",
        log(&root, "fetcher")
    );
    assert!(
        eventually(Duration::from_secs(20), || log(&root, "fetching")
            .contains("| hello from the box")),
        "{}",
        log(&root, "fetching")
    );
}

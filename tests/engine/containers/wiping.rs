//! Wiping containers: a container's volumes are removed, what mounts them
//! stopped first, and it is brought up again from nothing; `--all` removes
//! every volume of the environment; and nothing that does not carry the
//! environment's label is removed, whatever its name, the wipe stopping
//! there and saying what it did not remove.

use std::time::Duration;

use super::labelled;
use crate::docker::{docker, inspect};
use crate::harness::{Machine, eventually, log};

/// A volume this test made without any environment's label, removed when
/// it is dropped, also when the test fails.
struct Foreign(String);

impl Foreign {
    fn make(name: &str) -> Foreign {
        docker(&["volume", "create", name]);
        assert!(inspect(name).is_some(), "the volume `{name}` was made");
        Foreign(name.to_string())
    }
}

impl Drop for Foreign {
    fn drop(&mut self) {
        docker(&["volume", "rm", "-f", &self.0]);
    }
}

#[test]
fn wiping_a_container_removes_its_volumes_and_brings_it_up_from_nothing() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["keeper-data"]).code, 0);
    let starts = |count: &str| log(&root, "keeper-data").matches(count).count();
    assert!(eventually(Duration::from_secs(20), || starts("start 1") == 1));
    assert_eq!(machine.at(&root, "restart", &["keeper-data"]).code, 0);
    assert!(eventually(Duration::from_secs(20), || starts("start 2") == 1));
    let wipe = machine.at(&root, "wipe", &["keeper-data", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    let volume = format!("tay-{}-keeper-data.data", machine.id(&root));
    assert!(
        wipe.stdout.contains(&format!(
            "tay wipe: removed what `keeper-data` kept: its volume `data`, `{volume}`; its record\n"
        )),
        "{}",
        wipe.stdout
    );
    // Its volume made afresh, it counts from its first start again.
    assert!(eventually(Duration::from_secs(20), || starts("start 1") == 2));
    assert!(inspect(&volume).is_some());
}

#[test]
fn a_container_that_mounts_the_volumes_wiped_is_stopped_first_and_brought_up_again() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["keeper-data", "data-reader"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let reader = super::container(&machine, &root, "data-reader").unwrap();
    // The reader does not need the keeper, and holds its volume: Docker
    // removes no volume a container uses.
    let wipe = machine.at(&root, "wipe", &["keeper-data", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    assert_eq!(
        wipe.changes()[0],
        "data-reader stopped stopped by `tay wipe`"
    );
    assert!(
        wipe.stdout
            .ends_with("tay wipe: everything asked for is ready.\n"),
        "{}",
        wipe.stdout
    );
    let again = super::container(&machine, &root, "data-reader").unwrap();
    assert_ne!(again["Id"], reader["Id"]);
    assert_eq!(again["State"]["Running"], true);
}

#[test]
fn nothing_without_the_environments_label_is_removed_whatever_its_name() {
    let machine = Machine::with_docker();
    let one = machine.stack_as("containers", "boxes-one");
    let two = machine.stack_as("containers", "boxes-two");
    for root in [&one, &two] {
        let up = machine.at(root, "up", &["keeper-data"]);
        assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    }
    // A volume named as a unit of `one` names its own, which no tay made:
    // a volume another program made is as foreign as another
    // environment's.
    let foreign = Foreign::make(&format!("tay-{}-owned.data", machine.id(&one)));
    // Brought up, it is refused the volume.
    let up = machine.at(&one, "up", &["keeper-data", "owned"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    // The wipe stops there, and says what it did not remove.
    let wipe = machine.at(&one, "wipe", &["owned", "keeper-data", "--yes"]);
    assert_eq!(wipe.code, 1, "{}{}", wipe.stdout, wipe.stderr);
    assert!(
        wipe.stderr
            .starts_with("tay wipe: what `owned` keeps could not be removed: "),
        "{}",
        wipe.stderr
    );
    assert!(
        wipe.stderr.ends_with(&format!(
            "; not removed: its volume `data`, `{}`; its record, nor what `keeper-data` keeps; what was stopped for the wipe stays stopped until `tay up`\n",
            foreign.0
        )),
        "{}",
        wipe.stderr
    );
    assert!(inspect(&foreign.0).is_some());
    let own = format!("tay-{}-keeper-data.data", machine.id(&one));
    assert!(inspect(&own).is_some());
    let all = machine.at(&one, "wipe", &["--all", "--yes"]);
    assert_eq!(all.code, 0, "{}{}", all.stdout, all.stderr);
    // Its own volume is gone; the one no tay made, and every one of the
    // other environment, are left.
    assert!(inspect(&own).is_none());
    assert!(inspect(&foreign.0).is_some());
    let [containers, volumes, _] = labelled(&machine, &two);
    assert_eq!(
        volumes,
        [format!("tay-{}-keeper-data.data", machine.id(&two))]
    );
    assert_eq!(containers.len(), 1, "{containers:?}");
    let up = machine.at(&two, "status", &["keeper-data"]);
    assert!(up.stdout.contains("keeper-data  ready"), "{}", up.stdout);
}

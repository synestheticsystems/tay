//! Files written in the state folder, and volumes one unit owns and another
//! mounts.

use crate::support::{given, plan, plan_json, unit};

#[test]
fn two_files_with_the_same_folder_and_name_are_an_error_and_a_mount_names_an_owned_volume() {
    let run = plan("claims-and-volumes", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:10:46: unit `second-config` makes the file `files/proxy/routes.yaml` in the state folder, as unit `first-config` does",
            "  tay.yaml:7:46: `first-config` makes it here",
            "tay.yaml:23:21: unit `database` owns no volume `logs`; its volumes are `data`",
            "tay.yaml:24:21: there is no unit `nobody`",
        ]
    );
}

#[test]
fn a_file_is_written_under_files_in_its_folder_in_the_state_folder() {
    let plan = plan_json("secrets-allowed", &[]);
    let root = crate::support::fixture("secrets-allowed")
        .canonicalize()
        .unwrap();
    let credentials = unit(&plan, "credentials");
    assert_eq!(
        given(credentials, "path")["from_host"]["value"],
        format!(
            "{}/.tay/state/files/credentials/credentials",
            root.display()
        )
    );
    assert_eq!(
        given(credentials, "dir")["from_host"]["value"],
        format!("{}/.tay/state/files/credentials", root.display())
    );
}

#[test]
fn a_template_file_is_read_once_and_its_references_checked_as_a_definitions_are() {
    // Two units read `a.tmpl`: an error in it is reported once, naming each
    // unit it was found for, in the words the same reference written in a
    // definition would get.
    let run = plan("template-files", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "a.tmpl:1:6: `${p.nmber}`: unit `p` gives no `nmber`; it gives `number`",
            "  tay.yaml:6:40: read for unit `one` here",
            "  tay.yaml:7:40: read for unit `two` here",
            "a.tmpl:2:6: `${args.user}` is an action's argument, and unit `one` is not an action",
            "  tay.yaml:6:40: read for unit `one` here",
            "a.tmpl:2:6: `${args.user}` is an action's argument, and unit `two` is not an action",
            "  tay.yaml:7:40: read for unit `two` here",
        ]
    );
}

#[test]
fn an_address_a_template_file_cannot_have_is_reported_at_its_reference_in_the_file() {
    let run = plan("template-file-addresses", &[]);
    assert_eq!(
        run.errors(),
        [
            "site.tmpl:3:5: unit `conf` is made, and runs nowhere, and uses here an address of unit `api` as the host sees it, but `api` publishes no host port for its port 8080",
            "  tay.yaml:8:20: the endpoint of `api` is defined here",
            "  tay.yaml:9:41: read for unit `conf` here",
            "site.tmpl:4:6: unit `conf` is made, and runs nowhere, and uses here an address of unit `api` as the host sees it, but `api` publishes no host port for its port 8080",
            "  tay.yaml:8:20: the endpoint of `api` is defined here",
            "  tay.yaml:9:41: read for unit `conf` here",
        ]
    );
}

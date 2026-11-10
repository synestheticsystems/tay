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

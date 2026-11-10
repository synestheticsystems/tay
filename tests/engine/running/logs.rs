//! `tay logs UNIT [--since 10m] [--follow]`: every line the unit wrote,
//! each with its time, and each attempt marked.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::Duration;

use crate::harness::Machine;

#[test]
fn logs_gives_every_line_with_its_time_and_marks_each_attempt() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let ticks = || crate::harness::log(&root, "web").matches("| tick").count();
    assert!(crate::harness::eventually(
        Duration::from_secs(20),
        || ticks() >= 3
    ));
    let logs = machine.at(&root, "logs", &["web"]);
    assert_eq!(logs.code, 0, "{}", logs.stderr);
    let lines: Vec<&str> = logs.stdout.lines().collect();
    assert!(
        lines[0].ends_with(&format!(
            " # attempt 1, command: starting `{}/services/ticker.sh`",
            root.display()
        )),
        "{}",
        lines[0]
    );
    assert!(
        lines[1..].iter().all(|line| line.contains(" | tick ")),
        "{}",
        logs.stdout
    );
    for line in &lines {
        let (time, _) = line.split_once(' ').unwrap();
        assert_eq!(time.len(), 24, "{line}");
        assert!(time.ends_with('Z') && time.as_bytes()[10] == b'T', "{line}");
    }
    // `--since` keeps only what was written lately: not the note of the
    // attempt's start, written more than a second before.
    std::thread::sleep(Duration::from_millis(1500));
    let all = machine.at(&root, "logs", &["web"]).stdout.lines().count();
    let lately = machine.at(&root, "logs", &["web", "--since", "1s"]);
    assert!(lately.stdout.lines().count() < all, "{}", lately.stdout);
    assert!(!lately.stdout.contains("attempt 1"), "{}", lately.stdout);
}

#[test]
fn logs_follows_what_the_unit_writes_until_its_reader_stops() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let mut command = machine.command(
        &root,
        &[
            "logs",
            "web",
            "--follow",
            "--since",
            "1s",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    command.stdout(Stdio::piped());
    let mut follow = command.spawn().unwrap();
    let mut ticks = BufReader::new(follow.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap)
        .filter(|line| line.contains(" | tick "));
    let first = ticks.next().unwrap();
    let number = |line: &str| -> u64 { line.rsplit(' ').next().unwrap().parse().unwrap() };
    // Lines written after it began are given as they are written.
    let later = ticks.by_ref().take(20).last().unwrap();
    assert!(
        number(&later) >= number(&first) + 20,
        "{first} then {later}"
    );
    drop(ticks);
    // With no reader, it ends at the next line it would print.
    let ended = crate::harness::eventually(Duration::from_secs(10), || {
        matches!(follow.try_wait(), Ok(Some(_)))
    });
    assert!(ended, "`tay logs --follow` ended once nothing read it");
}

#[test]
fn logs_of_a_unit_that_has_written_nothing_says_so() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let logs = machine.at(&root, "logs", &["web"]);
    assert_eq!(logs.code, 2);
    assert_eq!(
        logs.stderr,
        format!(
            "tay logs: `web` has written nothing in this environment: there is no log at `{}`\n",
            root.join(".tay/logs/web").display()
        )
    );
    let bad = machine.at(&root, "logs", &["web", "--since", "soon"]);
    assert_eq!(bad.code, 2);
    assert!(
        bad.stderr
            .contains("`--since` is a whole number followed by s, m or h"),
        "{}",
        bad.stderr
    );
}

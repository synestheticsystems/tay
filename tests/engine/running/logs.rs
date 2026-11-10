//! `tay logs [UNIT...] [--since 10m] [--follow]`: every line the units
//! wrote, each with its time, and each attempt marked; with several units,
//! or none, the lines of all in the order of their times, each naming its
//! unit.

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

/// A line `tay logs` gives among several units' as its time, its unit and
/// the rest.
fn named(line: &str) -> (&str, &str, &str) {
    let (time, rest) = line.split_once(' ').unwrap();
    let rest = rest.trim_start();
    let (unit, rest) = rest.split_once(' ').unwrap();
    (time, unit, rest.trim_start())
}

#[test]
fn logs_of_several_units_or_none_give_their_lines_in_the_order_of_their_times_each_naming_its_unit()
{
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    assert!(crate::harness::eventually(Duration::from_secs(20), || {
        crate::harness::log(&root, "web").matches("| tick").count() >= 3
    }));
    for args in [&[][..], &["api", "web"][..]] {
        let logs = machine.at(&root, "logs", args);
        assert_eq!(logs.code, 0, "{}", logs.stderr);
        let lines: Vec<(&str, &str, &str)> = logs.stdout.lines().map(named).collect();
        assert!(
            lines.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "{}",
            logs.stdout
        );
        // Every line of each unit is there, in the order it wrote them; the
        // daemon's own log is no unit's.
        for unit in ["api", "web"] {
            let own = machine.at(&root, "logs", &[unit]).stdout;
            let given: Vec<String> = lines
                .iter()
                .filter(|(_, named, _)| *named == unit)
                .map(|(time, _, rest)| format!("{time} {rest}"))
                .collect();
            assert!(!given.is_empty(), "{unit}: {}", logs.stdout);
            // What it writes meanwhile follows.
            let own: Vec<&str> = own.lines().take(given.len()).collect();
            assert_eq!(own, given, "{unit}");
        }
        assert!(
            lines
                .iter()
                .all(|(_, unit, _)| ["api", "web"].contains(unit)),
            "{}",
            logs.stdout
        );
        // The web starts once the API listens, so the API's line comes
        // first.
        let listening = lines
            .iter()
            .position(|(_, unit, rest)| *unit == "api" && *rest == "| listening")
            .unwrap();
        let ticked = lines
            .iter()
            .position(|(_, unit, rest)| *unit == "web" && rest.starts_with("| tick"))
            .unwrap();
        assert!(listening < ticked, "{}", logs.stdout);
    }
}

#[test]
fn logs_of_every_unit_followed_takes_in_a_unit_that_begins_to_write() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let mut command = machine.command(
        &root,
        &[
            "logs",
            "--follow",
            "--since",
            "1s",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    command.stdout(Stdio::piped());
    let mut follow = command.spawn().unwrap();
    let mut lines = BufReader::new(follow.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap);
    assert!(lines.by_ref().any(|line| named(&line).1 == "web"));
    let up = machine.at(&root, "up", &["reporter"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    // The web ticks five times a second, so this waits about a minute at
    // most.
    let reported = lines.by_ref().take(300).any(|line| {
        let (_, unit, rest) = named(&line);
        unit == "reporter" && rest == "| variable MODE=fast"
    });
    assert!(reported, "a line of `reporter` was given once it wrote one");
    drop(lines);
    let ended = crate::harness::eventually(Duration::from_secs(10), || {
        matches!(follow.try_wait(), Ok(Some(_)))
    });
    assert!(ended, "`tay logs --follow` ended once nothing read it");
}

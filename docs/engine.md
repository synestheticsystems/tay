# The engine

The engine brings up what `tay plan` lists, keeps watching it, and says what is true about it. This document is the contract for that: what the commands promise, what a unit's state means, what is kept on disk, and how the pieces find each other again after any of them dies. The definitions themselves are in `definition-format.md`.

It is written ahead of the code in places. Each section says at its top whether it is built.

Four rules run through all of it.

- **True.** A state is reported from a check made now, or from a record written by whoever saw the event, never from memory alone. `tay up` says success only when every unit asked for is ready.
- **Nothing foreign is touched.** tay never signals a process it did not start, never removes a container, volume or network it did not create, and never takes a port from whoever holds it.
- **Blind.** The engine drives every unit through the same few operations. It never asks which module a unit uses.
- **It outlives itself.** The stack keeps running when the daemon dies or is replaced. The next daemon finds every unit again, and learns how each one ended if it ended in between.

## Commands

| Command | What it does | Built |
|---|---|---|
| `tay plan` | Reads and checks the definitions and prints the plan. Starts nothing | yes |
| `tay up [UNIT...]` | Brings the units up, in order, and returns when all are ready or one has failed | no |
| `tay down` | Stops everything the environment runs, checks that nothing is left, and says so | no |
| `tay status [UNIT]` | Each unit's state, from a check | no |
| `tay logs UNIT [--since 10m] [--follow]` | What the unit wrote, with the time of each line | no |
| `tay ls` | The environments on this machine | no |

Every command takes `--root FOLDER`, and `--json` where it prints something a program would read. No command ever reads from the terminal. Without a terminal there is no colour, and nothing follows for ever unless asked to.

### `tay up`

1. Reads and plans exactly as `tay plan` does. Errors in the definitions stop it here, with nothing started.
2. Starts this environment's daemon if none is running, and hands it the plan.
3. Prints each change of state as it happens, one line per change.
4. Returns when every unit asked for is `ready`, or when one of them can no longer become ready.

With no unit named it brings up what the choices list under `up`. With units named it brings up those and what they need, and leaves everything else as it is.

When a unit fails, `tay up` names it, gives the reason and the last lines it wrote, and names every unit that was waiting on it. A unit that failed an attempt and then became ready counts as ready, and the earlier attempts are shown.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Done: everything asked for is ready (`up`), or nothing is left running (`down`) |
| 1 | A unit failed, or something was left that could not be stopped |
| 2 | The definitions have errors |
| 3 | This tay is too old for the definitions |
| 4 | Refused: another change is running. The message says which, and since when |
| 5 | The daemon could not be started or reached |

## A unit's state

| State | Means |
|---|---|
| `waiting` | Something it needs is not ready yet. The detail names what, and if that has failed, says so at once: there is no timer on waiting |
| `starting` | An attempt is under way. The detail says which attempt and what it is doing: checking, preparing, running, waiting to be ready |
| `ready` | Its readiness check passes, or for a made unit, it exists |
| `unready` | It was ready and is still running, but its check no longer passes |
| `failed` | It cannot become ready without a change. The detail gives the reason, the exit code or signal, and how many attempts were made |
| `stopped` | It is not running because it was stopped or never asked for |

- The time a unit has to become ready is its own `within`, counted from the start of the attempt. No constant in tay overrides it.
- The daemon keeps checking after a unit is ready, so `unready` and a return to `ready` are both seen.
- **Restarts.** When a unit that runs exits, its `restart` decides. `attempts` counts attempts in a row that did not reach `ready`; reaching `ready` starts the count again. When they are used up the unit is `failed`.
- A unit whose need has failed stays `waiting` and is reported as blocked. It is never started against a failed need.

### Readiness checks, exactly

- `http`: a GET that gets any 2xx answer. `tcp`: a connection opens. Both are made from the host and tried again about twice a second until `within` passes.
- `command`: exits 0. For a `process` it runs on the host; for a `container`, inside it.
- `log`: a line the unit wrote contains the text.

## The daemon

*Not built.*

One daemon per environment. The first command that needs it starts it, detached from the terminal and from the shell that ran the command, so it survives both.

- **It owns the changes.** It starts units in order, watches them, applies restart policies, runs the checks and writes the records.
- **It listens on a Unix socket** and speaks HTTP with JSON bodies, under `/v1/`. It opens no TCP port. Who may control an environment is decided by the socket's file permissions alone. The socket is in a short folder of its own named by the environment's id, because a socket's path is limited to about a hundred bytes.
- **Its lifetime is a lock** on `.tay/daemon.lock`, held for as long as it runs and released by the system if it dies. A socket file whose lock is free is stale and is replaced.
- **One change at a time.** A second `up` while one runs joins it and gets the same result. `down` is always accepted and cancels what is running, so there is always a way to stop. Any other change is refused with exit code 4.
- **Versions.** A command and a daemon of different versions do not talk past each other: the command asks the older daemon to exit, which leaves the stack running, and starts its own.
- **`tay down`** stops every unit, in the reverse of the order they started in, then the daemon exits.
- **If the daemon dies,** nothing in the stack stops. The next command starts a new daemon, which adopts what is running (see "Finding units again").
- **If the environment's folder disappears** while the stack runs, the daemon stops everything it started and exits.

It keeps one fact about intent on disk: whether the environment is meant to be up. `tay up` sets it and `tay down` clears it.

## What is kept on disk

*Not built.*

```
.tay/
  choices.yaml            this environment's choices; people write it
  id                      the environment's id
  daemon.lock             held while the daemon runs
  state/
    intent                up or down
    ports.json            the port numbers this environment keeps
    secrets/              one file per secret, readable by this account only
    files/<folder>/<name> what `file` units write
    units/<unit>/         one folder per unit that runs: its lock, its record, the values it produced
  logs/
    <unit>/               that unit's log files
    tay/                  the daemon's own log
```

Per machine, outside any environment: a list of the environments tay knows, with the folder and the ports of each, in `~/.local/state/tay/`; and the folder shared by one person's environments of a stack, `~/.local/share/tay/<stack>/`.

### What identifies an environment

- **The id** is random, made the first time the environment is brought up, and kept in `.tay/id`. Sockets, the machine's list and Docker's labels use it. Moving the folder keeps it. A copied folder is noticed, because the machine's list has that id at another folder that still exists, and gets a new id.
- **The name** is the folder's name, for people. `tay ls` shows the folder too, so two environments with one name can be told apart.

An environment leaves the machine's list when a check finds nothing of it left: no process, no container, no volume, no network. A missing folder alone does not remove it.

## Units that are made

*Not built.*

**`port`.** A port unit promises a number that no other environment of tay on this machine will be given, tested as free when chosen, and remembered.

- The preferred number is tried once, the first time. After that the environment keeps what it got: kept beats preferred.
- Other numbers come from a range the system does not hand out by itself.
- Choosing goes through one lock and the machine's list, so two environments starting at once cannot get the same number.
- If a kept port turns out to be taken, by something that is not this environment, when the stack comes up: a `fixed` port fails its unit and names the holder as well as tay can; any other port is chosen again, and `tay up` says so.

**`secret`.** Generated once and kept in `state/secrets/`. Its value never appears in any output, in the stored plan, or in the argument list of anything tay starts.

**`file`.** Written whole and then moved into place, so no reader sees half a file. Written again only when its content would differ, and never when `overwrite` is `false` and it exists.

## Units that run

*Not built.*

### The keeper

Only a process's parent is told how it ended. If the daemon were the parent of what it starts, a daemon that died would lose both the exit code and the output of everything it had started. So each host process and each host command gets a keeper: tay's own binary run as `tay keep`, with no knowledge of the stack.

The order of events closes every gap:

1. The daemon writes "starting, attempt N" to the unit's record, takes a lock on the unit's lock file, and starts the keeper.
2. The keeper inherits the lock, writes its own record, and starts the unit's command as its child, in a process group of its own. It does not pass the lock on to the child.
3. The keeper writes every line the child prints to the unit's log, with the time.
4. When the child ends, the keeper writes the exit code or signal and the time to the record, and ends. The lock is released with it.

So the lock is held without a gap from before the keeper exists until it ends, and the system releases it if the keeper dies.

- **Stopping** a unit signals the child's whole group, waits, and then kills what is left, so processes the unit started in turn go with it.
- **Secrets** reach the child through its environment, which the keeper inherits from the daemon. They are never arguments.
- Two limits are known and accepted. A process that puts itself in a session of its own escapes the stop. And if a keeper is killed, a child that prints dies at its next line, while a silent one lives on; the next daemon sees a record with no lock and no ending, and stops the recorded group.

### Finding units again

A new daemon reads each unit's record and tries its lock.

| Lock | Record | The unit is |
|---|---|---|
| Held | Any | Running, under a live keeper. The daemon adopts it and goes on watching |
| Free | Has an ending | Ended while no daemon ran. Its exit code and time are known, and its restart policy applies |
| Free | No ending | Gone with no record. Reported as exactly that; anything left of its process group is stopped |

### `command` on the host

1. `check` runs first, if there is one. Exit 0: done, nothing more runs. Exit 1: not done. Any other: the unit fails, and `run` does not run.
2. `run` runs, with `stdin` if given.
3. The values it wrote to the file named by `TAY_VALUES` are kept; one declared secret goes with the secrets. Every file under `outputs` must exist afterwards.

A command killed part-way, or whose daemon was killed during it, is simply not done: the next `tay up` checks again and runs it again. A command with no `check` runs at every `tay up`.

### `process`

`prepare` commands run to completion, in order, before the first start. Then the command runs under its keeper, with only the environment the definition gives it and the names under `inherit`.

### Logs

Every line a unit writes while its keeper runs reaches `.tay/logs/<unit>/` with the time it was written, whether or not any command, or the daemon, is running. Each attempt is marked in the log. Files are rotated by size, and old ones are dropped, so a stack left running does not fill the disk.

## Containers

*Not built, and not yet written.* What is already settled is in `definition-format.md`: one network per environment, the labels `tay.environment` and `tay.unit` on everything tay creates, and published ports opened only when used from the host. Docker is a container's parent, keeps its exit code, and lists it by label, so a container needs no keeper.

## Not in this document yet

What restarts when a definition changes. `restart`, `wipe`, `set` and `run`. Running on a server under a service manager. These are written when they are built.

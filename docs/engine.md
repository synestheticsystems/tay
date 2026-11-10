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
| `tay up [UNIT...]` | Brings the units up, in order, and returns when all are ready or one has failed | yes |
| `tay down` | Stops everything the environment runs, checks that nothing is left, and says so | yes |
| `tay status [UNIT]` | Each unit's state, from a check | yes |
| `tay logs [UNIT...] [--since 10m] [--follow]` | What the units wrote, with the time of each line. With several units, or none, each line also names its unit | one unit |
| `tay ls` | The environments on this machine | yes |
| `tay restart UNIT...` | Stops the units and starts them again | no |
| `tay run ACTION [NAME=VALUE...]` | Runs an action, with its arguments | no |
| `tay set PLACE=VALUE...`, `tay unset PLACE...` | Changes what the environment remembers being asked for | no |
| `tay wipe UNIT... --yes`, `tay wipe --all --yes` | Removes what the units keep, or everything the environment keeps | no |

Every command takes `--root FOLDER`, and `--json` where it prints something a program would read. No command ever reads from the terminal. Without a terminal there is no colour, and nothing follows for ever unless asked to.

### `tay up`

1. Reads and plans exactly as `tay plan` does. Errors in the definitions stop it here, with nothing started.
2. Starts this environment's daemon if none is running, and hands it what was asked for. The daemon plans again from the same definitions, so no secret and no module's settings travel between the two.
3. Prints each change of state as it happens, one line per change.
4. Returns when every unit asked for is `ready`, or when one of them can no longer become ready. In the second case the daemon goes on bringing up whatever can still come up; `tay status` shows where that ends.

With no unit named it brings up what the choices list under `up`. With units named it brings up those and what they need, and leaves everything else as it is.

**What an environment was asked for stays asked.** The profile, each `--set` and the units named on `tay up` are remembered by the environment, so a later plain `tay up`, a `tay status`, a `tay plan` and a daemon started afresh all see the same stack. A new `--set` for the same place replaces the remembered one, and `--profile` replaces the remembered profile. `tay status` lists what is remembered.

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

- The time a unit has to become ready is its own `within`, counted from the moment its own program starts. Preparing, building and pulling come before that and are not counted. No constant in tay overrides it.
- The daemon keeps checking after a unit is ready, so `unready` and a return to `ready` are both seen.
- **Restarts.** When a unit that runs exits, its `restart` decides. `attempts` counts attempts in a row that did not reach `ready`; reaching `ready` starts the count again. When they are used up the unit is `failed`.
- A unit whose need has failed stays `waiting` and is reported as blocked. It is never started against a failed need.

### Readiness checks, exactly

- `http`: a GET that gets any 2xx answer. `tcp`: a connection opens. Both are made from the host and tried again about twice a second until `within` passes.
- `command`: exits 0. For a `process` it runs on the host; for a `container`, inside it.
- `log`: a line the unit wrote contains the text.

## The daemon

One daemon per environment. The first command that needs it starts it, detached from the terminal and from the shell that ran the command, so it survives both.

- **It owns the changes.** It starts units in order, watches them, applies restart policies, runs the checks and writes the records.
- **It listens on a Unix socket** and speaks HTTP with JSON bodies, under `/v1/`. It opens no TCP port. Who may control an environment is decided by the socket's file permissions alone. The socket is in a short folder of its own named by the environment's id, because a socket's path is limited to about a hundred bytes.
- **Its lifetime is a lock** on `.tay/daemon.lock`, held for as long as it runs and released by the system if it dies. A socket file whose lock is free is stale and is replaced.
- **One change at a time.** A second `up` that asks for exactly what the running one asks for joins it and gets the same result. `down` is always accepted and cancels what is running, so there is always a way to stop. Any other change, a different `up` included, is refused with exit code 4.
- **Versions.** A command and a daemon of different versions do not talk past each other: the command asks an older daemon to exit, which leaves the stack running, and starts its own. A command older than the daemon is refused with exit code 5 and changes nothing.
- **`tay down`** stops every unit, in the reverse of the order they started in, then the daemon exits. What made units left on disk stays: ports, secrets and files are an environment's own and are kept.
- **If the daemon dies,** nothing in the stack stops. The next command starts a new daemon, which adopts what is running (see "Finding units again").
- **If the environment's folder disappears** while the stack runs, the daemon stops everything it started and exits.

It keeps two facts about intent on disk: whether the environment is meant to be up, which `tay up` sets and `tay down` clears, and what it was last asked for. The second is readable by this account only, because a `--set` may carry a secret setting's value.

## What is kept on disk

```
.tay/
  choices.yaml            this environment's choices; people write it
  id                      the environment's id
  daemon.lock             held while the daemon runs
  state/
    intent                up or down
    asked                 the profile, the `--set`s and the units last asked for
    ports.json            the port numbers this environment keeps
    secrets/              one file per secret, readable by this account only
    files/<folder>/<name> what `file` units write
    units/<unit>/         one folder per unit that runs: its lock, its record, the values it produced
    attempts.json         what the daemon counts of each unit's attempts
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

**`port`.** A port unit promises a number that no other environment of tay on this machine will be given, tested as free when chosen, and remembered.

- The preferred number is tried once, the first time. After that the environment keeps what it got: kept beats preferred.
- Other numbers come from a range the system does not hand out by itself.
- Choosing goes through one lock and the machine's list, so two environments starting at once cannot get the same number.
- If a kept port turns out to be taken, by something that is not this environment, when the stack comes up: a `fixed` port fails its unit and names the holder as well as tay can; any other port is chosen again, and `tay up` says so.

**`secret`.** Generated once and kept in `state/secrets/`. Its value never appears in any output, in the stored plan, or in the argument list of anything tay starts.

**`file`.** Written whole and then moved into place, so no reader sees half a file. Written again only when its content would differ, and never when `overwrite` is `false` and it exists.

## Units that run

### The keeper

Only a process's parent is told how it ended. If the daemon were the parent of what it starts, a daemon that died would lose both the exit code and the output of everything it had started. So each host process and each host command gets a keeper: tay's own binary run as `tay keep`, with no knowledge of the stack.

The order of events closes every gap:

1. The daemon writes "starting, attempt N" to the unit's record, takes a lock on the unit's lock file, and starts the keeper.
2. The keeper inherits the lock, writes its own record, and starts the unit's command as its child, in a process group of its own. It does not pass the lock on to the child.
3. The keeper writes every line the child prints to the unit's log, with the time.
4. When the child ends, the keeper writes the exit code or signal and the time to the record, and ends. The lock is released with it.

So the lock is held without a gap from before the keeper exists until it ends, and the system releases it if the keeper dies.

- **Stopping** a unit signals the child's whole group, waits five seconds, and then kills what is left, so processes the unit started in turn go with it. When the child ends by itself, the keeper likewise stops what is left of its group, so that nothing a unit started is ever left running with no one who knows of it.
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

`prepare` commands run to completion, in order, before the first start of each bring-up, and not again before a restart. A `prepare` that fails fails the unit. Then the command runs under its keeper, with only the environment the definition gives it and the names under `inherit`, which are taken from the environment of the command that started the daemon.

A process that ends is not ready, whatever its exit code: with nothing left to restart it, it is `failed`, and the detail says how it ended.

### Logs

Every line a unit writes while its keeper runs reaches `.tay/logs/<unit>/` with the time it was written, whether or not any command, or the daemon, is running. Each attempt is marked in the log. Files are rotated by size, and old ones are dropped, so a stack left running does not fill the disk.

## Containers

Docker is a container's parent: it keeps the exit code, says whether the container was killed for memory, holds its output with the time of each line, and lists it by label. So a container needs no keeper, and everything below is asked of Docker, never remembered.

**What belongs to an environment.** One network, named `tay-<id>`, made when the first container needs it. Every container, volume and network tay creates carries the labels `tay.environment` (the id) and `tay.unit`. A container is named `tay-<id>-<unit>`, and on the network it answers to its unit's name and its `aliases`. Nothing without the environment's label is ever stopped, removed or changed.

**Bringing a container up.**

1. The image is pulled if it is not here, or built if the unit has `build`. Either shows in the unit's detail while it runs, and neither holds up a unit that does not need this one.
2. The unit's volumes are made if missing, and given to their `owner` when they are made.
3. The container is created from the unit's settings and started. A published port is given to Docker only if the plan opens it.
4. Readiness is checked: the image's own health check, or having started, when the unit writes no `ready`; a `command` check runs inside the container; `http` and `tcp` checks are made from the host through the opened port; a `log` check reads what the container wrote.

**A container whose settings have changed is replaced.** When the container that exists was created from other settings than the plan now gives (another image, environment, mount or published port), `tay up` removes it and creates it again. Its volumes stay.

**When a container ends,** tay reads from Docker how: the exit code, and whether it was killed for memory. The unit's `restart` decides what follows, as for a process, and the count of attempts is kept the same way.

**Finding containers again.** A new daemon asks Docker for the containers with the environment's label. One that runs is adopted. One that has ended is reported with how it ended, and its restart policy applies. A unit the plan has but Docker does not is simply not started yet.

**Logs.** The daemon copies each container's output into the unit's log, with Docker's time for each line, from where it last stopped. A daemon that was away loses nothing Docker still has. Before a container is removed, what remains of its output is copied.

**`tay down`** stops and removes the environment's containers and its network, and keeps its volumes: data is kept.

**A command in a container.** With `in`, the command runs inside the named unit's running container, as `user` if given. With `image`, it runs as a container of its own on the environment's network, with `run` in place of the image's entrypoint, and is removed when it ends. Either way its `check` runs the same way first, its secrets reach it through its environment, and the file named by `TAY_VALUES` is fetched from where the command ran.

**The host, seen from a container,** has one name on every system, so a definition never has to know which Docker it runs on. `${host.listen}` is the address a host process must listen on for containers to reach it and other machines not to.

**Units the plan no longer has.** A unit that is running and is not in the plan any more is stopped at the next `tay up`, which says so, and `tay status` shows it until then. This holds for processes as for containers.

**An environment leaves the machine's list** only when no process, container, volume or network of it is left.

## Changing an environment that is up

*Not built.*

Each of these is one change, taken by the daemon in turn like any other.

**`tay restart UNIT...`** stops each unit named and starts it again, with a new first attempt. Units that need it are left running: one that suffers shows it by its own check and follows its own `restart`. A command is run again from its `check`. A unit that is made has nothing to restart, and naming one is an error.

**`tay run ACTION [NAME=VALUE...]`** runs a unit that has `action: true`, with its arguments, after bringing up what it needs. It prints what the action prints and exits as the action did. An argument the action does not declare, or a missing one that has no default, is an error before anything runs. An action's values are given to whoever asked and are not kept.

**`tay set PLACE=VALUE...`** and **`tay unset PLACE...`** change what the environment remembers being asked for, in the places `--set` takes, and nothing else: no unit is touched until the next `tay up`. `tay up --set` is the same thing followed by `up`. `tay unset` forgets one place, so the profile's or the stack's value holds again. `tay set` with nothing after it lists what is remembered. A secret setting's value is never printed.

**`tay wipe UNIT... --yes`** removes what a unit keeps, so that it starts from nothing:

| The unit is | What is removed |
|---|---|
| A container | Its volumes |
| A command | What its `undo` undoes, and the values it produced. The units it needs must be up, or the wipe is refused and says which |
| A secret, a file, a port | The stored secret, the written file, the kept number |
| A process | Nothing beyond its record |

In order: every running unit that needs the unit is stopped, the unit is stopped, what it keeps is removed, and, if the environment is meant to be up, everything is brought up again as `tay up` would, so commands that needed it check again and run again where their work is gone.

**`tay wipe --all --yes`** takes the environment down and removes everything it keeps: every volume, and `.tay/` but for `choices.yaml`. No `undo` runs, since everything an undo would act on goes too. Without `--yes` either form changes nothing and says what it would remove.

## Not in this document yet

What restarts when a definition changes, for units other than containers. Watching files and running a unit again when they change. `when_removed: undo`. Running on a server under a service manager. These are written when they are built.

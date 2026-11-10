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
| `tay logs [UNIT...] [--since 10m] [--follow]` | What the units wrote, with the time of each line. With several units, or none, each line also names its unit | yes |
| `tay ls` | The environments on this machine | yes |
| `tay restart UNIT...` | Stops the units and starts them again | yes |
| `tay run ACTION [NAME=VALUE...]` | Runs an action, with its arguments | yes |
| `tay set PLACE=VALUE...`, `tay unset PLACE...` | Changes what the environment remembers being asked for | yes |
| `tay wipe UNIT... --yes`, `tay wipe --all --yes` | Removes what the units keep, or everything the environment keeps | yes |

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
| 2 | The definitions have errors, or what was asked does not fit this stack: a unit that does not exist, a restart of a unit that is made, an argument an action does not declare |
| 3 | This tay is too old for the definitions |
| 4 | Refused as things stand: another change is running (the message says which, and since when), or what was asked needs something that is not up, or could only be done by disturbing a unit that is up (the message says what) |
| 5 | The daemon could not be started or reached |

`tay run` is the one exception: when the action ran to an end, `tay run` exits with the action's own status, whatever number that is. When the action never ran, it exits with one of the codes above, and tay's message on standard error says which case it is.

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
    units/<unit>/         one folder per unit that runs: its lock, its record, the values it produced,
                          and a fingerprint of what it was started from
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

**`secret`.** Generated once and kept in `state/secrets/`. Its value never appears in any output, in the stored plan, or in the argument list of anything tay starts. Kept beats written, as for a port: if a secret's `length` or `encoding` is changed after it was generated, the stored value stays, because whatever already holds it (a database's own password, say) would not change with it, and `tay up` says that the two no longer match. `tay wipe` on the secret makes a new one. If its `kind` is changed, the stored value cannot give what the unit must give, so the unit fails and says to wipe it.

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

1. The image is pulled if it is not here, or built if the unit has `build`. Either shows in the unit's detail while it runs, and neither holds up a unit that does not need this one. An image tay builds carries the environment's label, like everything else tay creates, and `tay wipe --all` removes it.
2. The unit's volumes are made if missing, and given to their `owner` when they are made.
3. The container is created from the unit's settings and started. A published port is given to Docker only if the plan opens it.
4. Readiness is checked: the image's own health check, or having started, when the unit writes no `ready`; a `command` check runs inside the container; `http` and `tcp` checks are made from the host through the opened port; a `log` check reads what the container wrote.

**A container whose settings have changed is replaced.** When the container that exists was created from other settings than the plan now gives (another image, environment, mount or published port), `tay up` removes it and creates it again. Its volumes stay.

**When a container ends,** tay reads from Docker how: the exit code, and whether it was killed for memory, as far as Docker says: on Linux Docker sometimes fails to record a memory kill. The unit's `restart` decides what follows, as for a process, and the count of attempts is kept the same way.

**Finding containers again.** A new daemon asks Docker for the containers with the environment's label. One that runs is adopted. One that has ended is reported with how it ended, and its restart policy applies. A command that ran in a container leaves nothing in Docker once it has ended, so the daemon that saw it end keeps a record of how, and a new daemon finds the command from that record. Any other unit the plan has and Docker does not is simply not started yet.

**Logs.** The daemon copies each container's output into the unit's log, with Docker's time for each line, from where it last stopped. A daemon that was away loses nothing Docker still has. Before a container is removed, what remains of its output is copied.

**`tay down`** stops and removes the environment's containers and its network, and keeps its volumes: data is kept.

**A command in a container.** With `in`, the command runs inside the named unit's running container, as `user` if given. With `image`, it runs as a container of its own on the environment's network, with `run` in place of the image's entrypoint, and is removed when it ends. Either way its `check` runs the same way first, its secrets reach it through its environment, and the file named by `TAY_VALUES` is fetched from where the command ran.

**The host, seen from a container,** has one name on every system, so a definition never has to know which Docker it runs on. `${host.listen}` is the address a host process must listen on for containers to reach it and other machines not to.

**Units the plan no longer has.** A unit that is running and is not in the plan any more is stopped at the next `tay up`, which says so, and `tay status` shows it until then. This holds for processes as for containers.

**An environment leaves the machine's list** only when no process, container, volume or network of it is left.

## Changing an environment that is up

Each of these is one change, taken by the daemon in turn like any other.

**`tay restart UNIT...`** stops each unit named and starts it again, with a new first attempt, bringing up what the unit needs as `tay up UNIT` would. Units that need it are left running: one that suffers shows it by its own check and follows its own `restart`. A command is run again from its `check`. A unit that is made, or a group, has nothing to restart, and naming one is an error (exit code 2). On an environment that is down there is nothing to restart either (exit code 4).

**`tay run ACTION [NAME=VALUE...]`** runs a unit that has `action: true`, with its arguments, after bringing up what it needs. It works on an environment that is up, on one that is down and on one that was never brought up, and leaves the environment as it found it:

- What `tay run` had to bring up for the action is stopped again when the action ends.
- It never stops, restarts or replaces a unit that is up. If the action could only run by doing that, `tay run` is refused and says which unit (exit code 4).
- An argument the action does not declare, or a missing one that has no default, is an error before anything runs. An argument is read as its default's type, and as text when it has no default.
- The action's own lines go to standard output as they are written, and `tay run` exits as the action did. tay's own lines go to standard error.
- An action's values are given to whoever asked and are not kept: `tay run` prints each value the action produced, by name, after the action's output. A value declared secret prints as `<secret>`, never as itself, so an action that hands a person a credential writes it to a file of its own and gives the file's path.

**`tay set PLACE=VALUE...`** and **`tay unset PLACE...`** change what the environment remembers being asked for, in the places `--set` takes, and nothing else: no unit is touched until the next `tay up`. `tay up --set` is the same thing followed by `up`. `tay unset` forgets one place, so the profile's or the stack's value holds again; a place with nothing remembered is an error. `tay set` plans the stack with the new values before remembering them, and remembers nothing if it no longer plans. `tay set` with nothing after it lists what is remembered: the profile, the units and each place. A secret setting's value is never printed. Neither command starts a daemon: with none running, the change is made under the daemon's own lock.

**`tay wipe UNIT... --yes`** removes what a unit keeps, so that it starts from nothing:

| The unit is | What is removed |
|---|---|
| A container | Its volumes |
| A command | What its `undo` undoes, and the values it produced. The units it needs must be up, or the wipe is refused and says which (exit code 4) |
| A secret, a file, a port | The stored secret, the written file, the kept number |
| A process | Nothing beyond its record |

In order: every running unit that needs the unit or mounts one of its volumes, directly or through other units that do, is stopped; the unit is stopped; what it keeps is removed; and, if the environment is meant to be up, everything is brought up again as `tay up` would, so commands that needed it check again and run again where their work is gone.

- Only a unit in the environment's plan can be wiped. What a unit left out of `up` still keeps is removed by `tay wipe --all`.
- A wipe that would itself stop a unit a command's `undo` needs is refused like one whose need is not up.
- If a removal fails, the wipe stops there and says what was removed and what was not. What it stopped stays stopped until the next `tay up`.

**`tay wipe --all --yes`** takes the environment down and removes everything it keeps: every volume, and `.tay/` but for `choices.yaml`. No `undo` runs, since everything an undo would act on goes too. Without `--yes` either form changes nothing and says what it would remove.

**On an environment that is down,** `tay run`, `tay wipe` and anything else that needed a daemon leave none behind: once the change is done and nothing runs, the daemon removes the environment's network, as `tay down` does, and exits.

## When something changes

**What a unit was started from.** For each unit that is up, tay keeps a fingerprint of what it was started from: its filled settings, with each secret taken as a digest and never as its value; the content of every file it uses that tay knows of (a file a `file` unit wrote, a file another unit built and lists under `outputs`); and, for a container, the image it was created from, not the tag.

**At `tay up` on an environment that is up,** each unit is compared with its fingerprint.

- A unit whose settings, files or image differ from what it was started from is started again: a container is replaced, a process is restarted, a file is written again. A port keeps its number and a secret its value: kept beats written.
- A unit that is running with no fingerprint kept, as one started by an older tay is, is started again, and `tay up` says that what it was started from is not known.
- A unit that only receives a value from a unit that changed is started again if the value it receives changed, and left alone if it did not.
- A command is checked again, as at every `tay up`.
- Everything else is left running.

`tay up` says of each unit it starts again what changed, by the setting's or the file's name, and never shows a secret.

**Watching, in development.** A unit with `watch` is run again when a file under the paths it names changes: a command runs again, a process is restarted. Files that git ignores do not count, and a folder that git ignores is not watched at all, so a large build folder costs nothing. This is what rebuilds a service when its source is saved.

- Changes are gathered for a moment, so that saving several files runs the unit once.
- A change that arrives while the unit is running does not stop it. The unit runs once more when it ends, however many changes arrived meanwhile.
- While a command runs again, and if it fails, the units that use what it builds stay as they are, `ready` on the last good build. Its failure shows on the command itself, with the last lines it wrote, and the next change runs it again.
- When it succeeds, each unit that uses one of its `outputs` is restarted if that file's content differs from what the unit was started from, and left alone if it does not. So one build that changes one of three programs restarts one service.
- `tay status` shows a command that is running again as `starting`, with what changed.

Production profiles refuse a unit with `watch`, so none of this happens on a server.

**A command with `when_removed: undo`** keeps, once it has run, what its `undo` needs: the command, where it runs, and its environment, the secrets with the secrets. At a `tay up` whose plan no longer has the unit, tay runs that `undo` before it stops anything else the plan no longer has, and then forgets it. If the `undo` fails, `tay up` says so, keeps it, and tries again at the next `tay up`. `tay wipe --all` runs none of them.

## Not in this document yet

Running on a server under a service manager. Written when it is built.

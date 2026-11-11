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
| `tay serve` | Is the environment's daemon in the foreground, for a service manager to start, restart and stop | yes |

Every command takes `--root FOLDER`, and `--json` where it prints something a program would read. No command ever reads from the terminal. Without a terminal there is no colour, and nothing follows for ever unless asked to.

### `tay up`

1. Reads and plans exactly as `tay plan` does. Errors in the definitions stop it here, with nothing started.
2. Starts this environment's daemon if none is running, and hands it what was asked for. The daemon plans again from the same definitions, so no secret and no module's settings travel between the two.
3. Prints each change of state as it happens, one line per change.
4. Returns when every unit asked for is `ready`, or when one of them can no longer become ready. In the second case the daemon goes on bringing up whatever can still come up; `tay status` shows where that ends.
5. Ends by saying how long it took and which units took longest, so that a slow bring-up shows where its time went: each unit this `tay up` started that took a second or more, from the start of its first attempt until it was `ready` or had failed, the longest first, five at most. The time a unit spent waiting for what it needs is not its own and is not counted. With `--json` the closing line carries `seconds`, and under `took` every unit this `tay up` started with its seconds. The whole is counted from when the daemon takes the change until it is decided, so a `tay up` that joins another says the same. A unit still under way when `tay up` returns, as one can be when another has failed, is counted until then and named as still under way; under `took` each unit also carries the state it came to. A unit started again because what it was started from changed is counted from when that was found, stopping it included.

With no unit named it brings up what the environment is asked for: the units an earlier `tay up` named, or what the choices list under `up` when none did. With units named, those are what the environment is asked for from then on: it brings them up with what they need, and stops what was up and is no longer needed, as it stops a unit taken out of the plan. `tay restart <unit>` starts one unit again without changing what is asked for.

**What an environment was asked for stays asked.** The profile, each `--set` and the units named on `tay up` are remembered by the environment, so a later plain `tay up`, a `tay status`, a `tay plan` and a daemon started afresh all see the same stack. A new `--set` for the same place replaces the remembered one, and `--profile` replaces the remembered profile. `tay status` lists what is remembered, and names the profile in force with what chose it: the command line (now or remembered), the environment's file, or the stack's default. It names the profile a plan made now would use: after `tay set profile=<name>` it names the new one, which the units take at the next `tay up`. A remembered choice that no longer fits the stack, as when a change of branch takes away a setting or a unit it names, is reported as remembered, with the `tay unset` that forgets it.

When a unit fails, `tay up` names it, gives the reason and the last lines it wrote, and names every unit that was waiting on it. A unit that failed an attempt and then became ready counts as ready, and the earlier attempts are shown.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Done: everything asked for is ready (`up`), or nothing is left running (`down`) |
| 1 | A unit failed, or something was left that could not be stopped |
| 2 | The definitions have errors, or what was asked does not fit this stack: a unit that does not exist, a restart of a unit that is made, an argument an action does not declare |
| 3 | This tay is too old for the definitions |
| 4 | Refused as things stand: another change is running (the message says which, and since when), or what was asked needs something that is not up, or could only be done by disturbing a unit that is up (the message says what) |
| 5 | The daemon could not be started or reached; for `tay serve`, it could not be the daemon or could not go on |

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
- **It listens on a Unix socket** and speaks HTTP with JSON bodies, under `/v1/`. It opens no TCP port. Who may control an environment is decided by the socket's file permissions alone. The socket is in a short folder of its own named by the environment's id, because a socket's path is limited to about a hundred bytes. Where that folder is depends on the account's session, so the daemon writes the socket's path in `.tay/`, and a command finds the daemon by what is written there: a daemon started by a service manager and a person's own shell then agree on it whatever variables each was started with.
- **Its lifetime is a lock** on `.tay/daemon.lock`, held for as long as it runs and released by the system if it dies. A socket file whose lock is free is stale and is replaced.
- **One change at a time.** A second `up` that asks for exactly what the running one asks for joins it and gets the same result. `down` is always accepted and cancels what is running, so there is always a way to stop. Any other change, a different `up` included, is refused with exit code 4.
- **Versions.** A command and a daemon of different versions do not talk past each other: the command asks an older daemon to exit, which leaves the stack running, and starts its own. A command older than the daemon is refused with exit code 5 and changes nothing.
- **`tay down`** stops every unit, in the reverse of the order they started in, then the daemon exits. What made units left on disk stays: ports, secrets and files are an environment's own and are kept. If it cannot record that the environment is down, as on a full disk, it stops everything all the same, says that the next command will take the environment for up and start it again, and exits with code 1.
- **If the daemon dies,** nothing in the stack stops. The next command starts a new daemon, which adopts what is running (see "Finding units again").
- **If the environment's folder disappears** while the stack runs, the daemon stops everything it started and exits.

It keeps two facts about intent on disk: whether the environment is meant to be up, which `tay up` sets and `tay down` clears, and what it was last asked for. The second is readable by this account only, because a `--set` may carry a secret setting's value.

## What is kept on disk

```
.tay/
  choices.yaml            this environment's choices and its profile; people write it
  id                      the environment's id
  daemon.lock             held while the daemon runs
  daemon.socket           the path of the daemon's socket, written once it listens
  bootstrap.lock          held by the command that starts the daemon, and by `tay serve` as it takes its place
  state/
    intent                up or down
    asked                 the profile, the `--set`s and the units last asked for
    ports.json            the port numbers this environment keeps
    secrets/              a file for each secret unit's value, for the secrets a command produced, and for those of a kept undo; readable by this account only
    files/<folder>/<name> what `file` units write
    units/<unit>/         one folder per unit that runs: its lock, its record, the values it produced,
                          a fingerprint of what it was started from, the undo it keeps,
                          and why that undo last did not run
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
- A number is free when no program listens on it, on any address of the machine. A number an ordinary user may not take, as one below 1024 on Linux, is judged by the system's tables of what listens alone: tay never holds such a port itself, Docker does. A connection that a stopped program left closing holds nothing, so a unit started again finds its number free.
- A kept port is the environment's own for as long as a unit of the environment that uses it runs, whatever the change being made: `tay restart` of another unit, `tay run`, and the bring-up after a `tay wipe` never choose a new number for it.
- If a kept port turns out to be taken, by something that is not this environment, when the stack comes up: a `fixed` port fails its unit and names the holder as well as tay can; any other port is chosen again, and `tay up` says so.

**`secret`.** Generated once and kept in `state/secrets/`. Its value never appears in any output, in the stored plan, or in the argument list of anything tay starts. Kept beats written, as for a port: if a secret's `length` or `encoding` is changed after it was generated, the stored value stays, because whatever already holds it (a database's own password, say) would not change with it, and `tay up` says that the two no longer match. `tay wipe` on the secret makes a new one. If its `kind` is changed, the stored value cannot give what the unit must give, so the unit fails and says to wipe it.

**`file`.** Written whole and then moved into place, so no reader sees half a file. Written again only when its content would differ, and never when `overwrite` is `false` and it exists. Such a file belongs to the program that rewrites it from then on: tay does not read it again, it is ready when it exists, and in the fingerprint of a unit that uses it it counts as there or not there, never by its content. So a program that takes the file as its own, or changes it, fails nothing and restarts nothing.

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

A daemon started while the definitions have errors cannot plan. It still finds what runs of the environment, and looks after none of it until the definitions plan again: nothing that ends is started again, and the next `tay up` that plans takes everything up from there. Until then `tay status` shows the errors with each unit found running and exits with code 2, `tay restart` and `tay wipe` of a unit are refused with the same errors, and `tay down` stops everything it finds.

### `command` on the host

1. `check` runs first, if there is one. Exit 0: done, nothing more runs. Exit 1: not done. Any other: the unit fails, and `run` does not run.
2. `run` runs, with `stdin` if given.
3. The values it wrote to the file named by `TAY_VALUES` are kept; one declared secret goes with the secrets. Every file under `outputs` must exist afterwards.

A command killed part-way, or whose daemon was killed during it, is simply not done: the next `tay up` checks again and runs it again. A command with no `check` runs at every `tay up`.

### `process`

`prepare` commands run to completion, in order, before the unit is started by a `tay up` or a `tay restart`, because what it was started from changed, or because a file it watches was saved. They do not run again when its restart policy starts it after it ended. A `prepare` that fails fails the unit. Then the command runs under its keeper, with only the environment the definition gives it and the names under `inherit`, which are taken from the environment of the command that started the daemon.

A process that ends is not ready, whatever its exit code: with nothing left to restart it, it is `failed`, and the detail says how it ended.

### Logs

Every line a unit writes while its keeper runs reaches `.tay/logs/<unit>/` with the time it was written, whether or not any command, or the daemon, is running. Each attempt is marked in the log. Files are rotated by size, and old ones are dropped, so a stack left running does not fill the disk. A line that cannot be written, as when the disk is full, is lost, and the program goes on running.

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

**`tay down`** stops and removes the environment's containers and its network, and keeps its volumes: data is kept. A volume Docker made by itself for a path an image declares and the unit does not name belongs to that one container and is removed with it, whenever tay removes a container. Each attempt runs in a container of its own, so such a volume is empty again at every attempt, a restart after a crash included: a unit that must keep what its image writes there names the volume. Unnamed volumes that an earlier tay left behind carry no label, and this one does not find them.

**When Docker itself stops,** as it does when it is upgraded or restarted, every container ends with it, and none of that is a unit's doing. While Docker does not answer, nothing that ends or cannot start is counted against a unit; when it answers again, the daemon brings the environment up as `tay up` would, if it is meant to be up, and says in its log and in `tay status` when Docker stopped answering and when the environment was brought up again. Host processes are not touched. A `tay up` run while Docker does not answer cannot tell what is left in Docker, so it exits with code 1 and the units go on waiting. Two things this does not cover: a Docker set to keep containers running through its own restart has them replaced all the same, and a Docker that hangs without closing its connections is not noticed.

**A command in a container.** With `in`, the command runs inside the named unit's running container, as `user` if given. With `image`, it runs as a container of its own on the environment's network, with `run` in place of the image's entrypoint, and is removed when it ends. Either way its `check` runs the same way first, its secrets reach it through its environment, and the file named by `TAY_VALUES` is fetched from where the command ran.

**The host, seen from a container,** has one name on every system, so a definition never has to know which Docker it runs on. `${host.listen}` is the address a host process must listen on for containers to reach it and other machines not to.

**Units the plan no longer has.** A unit that is running and is not in the plan any more is stopped at the next `tay up`, which says so, and `tay status` shows it until then. This holds for processes as for containers. So `tay status` shows the units of the plan, each unit that still runs though the plan no longer has it, and each unit whose `undo` is still waiting (below); a port, a file, a secret or a group that the plan no longer has is not shown, since nothing of it runs.

**An environment leaves the machine's list** only when no process, container, volume or network of it is left.

## Changing an environment that is up

Each of these is one change, taken by the daemon in turn like any other.

**`tay restart UNIT...`** stops each unit named and starts it again, with a new first attempt, bringing up what the unit needs as `tay up UNIT` would. Units that need it are left running: one that suffers shows it by its own check and follows its own `restart`. A command is run again from its `check`. A unit that is made, or a group, has nothing to restart, and naming one is an error (exit code 2). On an environment that is down there is nothing to restart either (exit code 4). `tay restart`, and a `tay wipe` that brings the environment up again, end as `tay up` does, saying how long they took.

**`tay run ACTION [NAME=VALUE...]`** runs a unit that has `action: true`, with its arguments, after bringing up what it needs. It works on an environment that is up, on one that is down and on one that was never brought up, and leaves the environment as it found it:

- What `tay run` had to bring up for the action is stopped again when the action ends.
- It never stops, restarts or replaces a unit that is up. If the action could only run by doing that, `tay run` is refused and says which unit (exit code 4).
- An argument the action does not declare, or a missing one that has no default, is an error before anything runs. An argument is read as its default's type, and as text when it has no default.
- The action's own lines go to standard output as they are written, and `tay run` exits as the action did. tay's own lines go to standard error.
- An action's values are given to whoever asked and are not kept: `tay run` prints each value the action produced, by name, after the action's output. A value declared secret prints as `<secret>`, never as itself, so an action that hands a person a credential writes it to a file of its own and gives the file's path.

**`tay set PLACE=VALUE...`** and **`tay unset PLACE...`** change what the environment remembers being asked for, in the places `--set` takes, and nothing else: no unit is touched until the next `tay up`. `tay up --set` is the same thing followed by `up`. `tay unset` forgets one place, so the profile's or the stack's value holds again; a place with nothing remembered is an error. `tay set` plans the stack with the new values before remembering them, and remembers nothing if it no longer plans. `tay set` with nothing after it lists what is remembered: the profile, the units and each place. A secret setting's value is never printed. Neither command starts a daemon: with none running, the change is made under the daemon's own lock. `profile` is a place for both as well: `tay set profile=<name>` remembers a profile as `--profile` does, and `tay unset profile` forgets the remembered one, so that the environment's file or the stack's default chooses again. `units` is a place for `tay unset` alone: it forgets the units named on an earlier `tay up`, so that what the choices list under `up` comes up again.

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

**A command with `when_removed: undo`** keeps, once it has run, what its `undo` needs: the command, where it runs, its environment, the secrets with the secrets, and the names of the units it needed. At a `tay up` whose plan no longer has the unit, tay runs that `undo` once the units it needed that are still in the plan are ready, and before it stops anything else the plan no longer has, so that what it needed and is still up stays up for it; then it forgets the unit. A unit it needed that is neither up nor in the plan any more is not waited for, because nothing would bring it up: the `undo` runs without it, and tay says which units it ran without. This is what lets an app be taken off the list while the environment is down. If the `undo` fails, or a unit it needed that the plan still has has failed, `tay up` says so, exits with code 1, keeps the `undo` and tries again at the next `tay up`; `tay status` lists every unit whose `undo` is still waiting, with why it last did not run. Several undos run one at a time, the last kept first. `tay wipe --all` runs none of them. An action cannot have `when_removed: undo`: it is never part of what `tay up` brings up, so it never leaves.

## Running on a server under a service manager

A server has to bring its stack back after a reboot with no one logged in, restart the daemon if it dies, and stop the stack in order when the machine shuts down. Every service manager does those three things for a program that stays in the foreground, so tay has one command that is this environment's daemon in the foreground: `tay serve`.

**`tay serve`** becomes the environment's daemon without detaching, brings up what the environment remembers being asked for, and stays until it is told to stop.

- **Being started is being asked to be up.** `tay serve` brings the environment up whatever was last recorded of whether it is meant to be up. The service manager's own switch (enabled or not, started or not) is the record that counts on a server.
- **It prints each change of state**, for as long as it runs, one line per change as `tay up` does, so the service manager's log holds the stack's history. Units' own output stays in their logs.
- **Told to stop** (the signal a service manager sends, `SIGTERM`, or an interrupt from a terminal), it takes the environment down in order, as `tay down` does, and exits as `tay down` would: with 0, or with 1 if something was left that could not be stopped. A hang-up (`SIGHUP`) does not stop it.
- **`tay down`** by a person does the same through the socket: the stack stops, `tay serve` exits with 0, and a service manager told to restart only on failure leaves it stopped until it is started again. So down means down, on a server too.
- **If it dies**, nothing in the stack stops. The service manager starts it again, and it adopts what is running, as any new daemon does. It exits with a status other than 0 only when it could not be the daemon or could not go on.
- **If a daemon already runs** for the environment, started by a plain `tay up` in someone's session, `tay serve` asks it to exit, which leaves the stack running, and takes its place.
- **Every other command works as always**, through the socket: `tay status`, `tay up` (which joins and returns when the stack is ready, so an install script can wait on it), `tay restart`, `tay logs`. Who may use them is decided by the socket's permissions: the account `tay serve` runs as.
- **A newer tay.** A command newer than a daemon that is being served does not start a daemon of its own in the person's session. It asks the old one to exit, waits up to thirty seconds for the service manager to start the new one from the same path, and says so if that does not happen (exit code 5).
- **It tells the service manager when it answers.** `tay serve` reports itself ready to systemd once its socket answers, so that `systemctl start` returns only then, and a `tay up` run right after it joins the bring-up instead of starting a daemon of its own.

With systemd, the whole of it is:

```
[Unit]
Description=<stack> (tay)
Wants=docker.service
After=docker.service network-online.target

[Service]
Type=notify
User=<account in the docker group>
ExecStart=/usr/local/bin/tay serve --root /srv/<stack>
Restart=on-failure
TimeoutStopSec=5min
KillMode=mixed

[Install]
WantedBy=multi-user.target
```

`KillMode=mixed` sends the stop signal to `tay serve` alone, which stops the stack itself. A production profile runs nothing on the host, so nothing of the stack is in the service's own process group to be killed with it. The unit wants Docker and starts after it, and does not require it: a service that requires Docker is stopped for good whenever Docker restarts, and what `tay serve` does when Docker stops is said under "Containers".

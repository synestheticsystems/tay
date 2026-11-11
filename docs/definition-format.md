# The definition format, version 1

A stack's definitions say what the stack is made of and what each part needs. tay reads them, works out every value that can be known before anything starts, puts the units in order, and only then starts anything. This document is the contract for the first half of that: what a definition may contain and what `tay plan` does with it. Nothing described here starts a process or a container.

Two rules run through all of it.

- **Strict.** An unknown key, a wrong type, a duplicate name, or a reference to something that does not exist is an error with the file, line and column. So is a YAML tag, a second document in one file, and a file or folder under the units folder that defines nothing. A file or folder whose name begins with a dot is not read at all. Nothing written where a mapping may be empty is an empty one: a profile or a choices file that holds only comments, a setting declared with nothing after its name, `settings` with every line under it commented out. tay reports every error it can find in one run, and starts nothing if there is any.
- **Blind.** The planner knows units, their needs and their values. It never asks which module a unit uses. It hands each module that module's own settings to check, and asks every module the same two questions: where does this unit run, and is it something that runs or something that is made.

Words used here: a **stack** is everything one `tay.yaml` describes. An **environment** is one running copy of a stack. A **unit** is one thing tay brings up. A **module** is the mechanism a unit uses. A **way** is one of several mechanisms a unit may be run by.

## Files

| File | What it holds |
|---|---|
| `tay.yaml` | Marks the root of a stack. tay finds it by walking up from the current directory |
| `stack/units/<name>/unit.yaml` | One unit, named after its folder. Other files in the folder are that unit's own |
| `stack/units/<anything>.yaml` | Several units in one file: a mapping from unit name to unit |
| `stack/templates/<name>.yaml` | A unit shape the stack repeats |
| `stack/profiles/<name>.yaml` | A named set of choices: what comes up, which way units run, values of settings |
| `.tay/` | This environment's own choices and state. Never committed. `tay plan` reads it and never writes it |

A path that begins `./` is relative to the folder of the file it is written in. Any other relative path is relative to the stack's root. This holds wherever a setting is a path on the host: the `workdir` of a process or of a command run on the host, a mount's `from`, a `template`, a `stdin`, a build's `context`, an entry in `outputs` or `watch`. The value a unit gives for such a path is the whole path. Text that merely contains a path, such as an argument in a `command`, is left as written.

### `tay.yaml`

```yaml
format: 1                    # required; the version of this document
requires: ">=0.1.0"          # optional; the oldest tay that may read these definitions
stack: example               # required; the stack's name, formed as a unit's name is
default_profile: dev         # required if there is more than one profile
units_dir: stack/units       # optional; this is the default
templates_dir: stack/templates
profiles_dir: stack/profiles
settings:                    # optional; the settings this stack has (see Settings)
  log.web: { default: info }
up: [web]                    # optional; what comes up unless a later choice says otherwise
run: { default: source }     # optional; likewise for the way units run (see Choices)
units:                       # optional; units written here instead of in files
  web-port: { uses: port }
```

A small project may keep everything in `tay.yaml` and have no `stack/` folder and no profile. Units from `units:` and from files form one set.

A folder that `tay.yaml` names and that does not exist is an error. The default folders may be absent.

`requires` is `>=` and a version of three numbers. A tay that is older than `requires` stops and names the version it would need. A tay that does not know `format` stops and names the format. Neither guesses.

### Names

A unit, template, profile, way, volume or folder name is lower-case letters, digits and hyphens, starts with a letter, and is at most 40 characters. Unit names are unique across the whole stack, groups included; a duplicate is an error that names both places. The names `settings`, `environment`, `host`, `net`, `self`, `params` and `args` are not available to units, because references use them, and neither is `tay`, which names tay's own log.

The name of a value (in `gives`, `produces`, `outputs`, `params` or `args`) is lower-case letters, digits and underscores, and starts with a letter. `outputs` is not available as one.

## A unit

Every unit has the same fields, whatever it uses.

| Field | Meaning |
|---|---|
| `doc` | One line saying what the unit is for |
| `uses`, `with` | The one module, and that module's own settings |
| `run` | In place of `uses`: the ways this unit can run, by name, each with its own `uses` |
| `from`, `params` | In place of `uses`: a template of this stack, and the values to fill it with |
| `needs` | Units that must be ready before this one starts |
| `after` | Units to wait for, ready or failed, without depending on them |
| `ready` | How readiness is checked, and the time allowed |
| `restart` | What to do when it exits |
| `watch` | Files and folders whose change runs the unit again. Development only |
| `gives` | Named values other units may use |
| `action`, `args` | `true` for a unit that runs only when asked for by name, and the arguments it may be asked with |

`ready`, `restart` and `watch` belong to units that run. On a unit that is made (a `port`, a `file`, a `secret`) each is an error: such a unit is ready when it exists, and nothing would ever run the check.

A unit has exactly one of `uses`, `run` or `from`, or none of them. A unit with none is a **group**: it may have only `doc` and `needs`, and asking for it asks for everything it needs.

```yaml
# stack/units/web/unit.yaml
doc: The site's development server.
uses: process
with:
  command: ["node", "node_modules/vite/bin/vite.js", "--port", "${web-port.number}", "--strictPort"]
ready: { http: "${self.site.url}", within: 2m }
gives:
  site: { port: "${web-port.number}", scheme: http }
```

### More than one way to run

`run` lists a unit's ways. The rest of the stack refers to the unit, never to a way.

```yaml
needs: [database]
with:
  env: { DATABASE: "${database.db}" }
restart: { on: failure, attempts: 3 }
run:
  source:
    uses: process
    with: { command: ["${build.outputs.server}"] }
    ready: { http: "${self.api.url}/health", within: 2m }
    gives:
      api: { port: "${api-port.number}", scheme: http }
  image:
    uses: container
    with: { image: "example/server@${settings.images.server}" }
    ready: { command: ["server", "--health"], within: 3m }
    gives:
      api: { port: 3000, scheme: http }
```

- A way may have `uses`, `with`, `needs`, `after`, `ready`, `restart`, `watch` and `gives`.
- The unit's own `needs`, `after` and `gives` apply to every way and are joined with the way's.
- The unit's own `ready`, `restart` and `watch` are what a way has when it writes none of its own.
- The unit's own `with` is joined with the way's, setting by setting, the way's winning. A setting that maps names to values (`env`, `secret_env`, `labels`, `volumes`, `publish`) is joined entry by entry. Each way is then checked against its module, so a setting written for the whole unit must be one that every way's module has.
- After joining, every way of a unit must give the same value names, and the names a way lists under `produces` and `outputs` count among them. If one gives `api` and another does not, that is an error: a consumer must not break when the unit is switched.
- Which way is used is a choice of the environment (see Choices). A unit must have a usable way only if the plan reaches it: it comes up, something that comes up uses one of its values, or a reference asks for its view with `@`.

### Templates

A template is a unit shape with holes.

```yaml
# stack/templates/postgres-database.yaml
params:
  name: {}                      # no default: required
  owner: { default: app }
unit:
  uses: command
  with:
    in: postgres
    env: { DB: "${params.name}" }
    check: { shell: "out=$(psql -U postgres -tAc \"select 1 from pg_database where datname='$DB'\") || exit 2; test \"$out\" = 1" }
    run: ["createdb", "-U", "postgres", "-O", "${params.owner}", "${params.name}"]
    undo: ["dropdb", "-U", "postgres", "--force", "--if-exists", "${params.name}"]
```

```yaml
# stack/units/databases.yaml
app-db: { from: postgres-database, params: { name: app } }
```

- A template's `unit` may be anything a unit may be, ways included, except that it may not itself use `from`.
- A unit made `from` a template may have only `doc`, `from`, `params`, `needs` and `after`; its `needs` and `after` are joined with the template's.
- A parameter's value may be any YAML value and may contain references. They mean what they would mean written in the unit that is made. Parameters are put in place before anything else is read from the unit, so a mapping passed as a parameter joins as one written out would.
- A missing required parameter, or a parameter the template does not declare, is an error.
- A template's settings are checked through the units made from it, so a template nothing uses is checked only for its shape.

### Actions

An action is a unit that runs only when someone asks for it by name. It may declare arguments, as a template declares parameters, and uses them as `${args.name}`.

```yaml
# stack/units/mint-session/unit.yaml
doc: Print a signed-in session for one user.
action: true
args:
  user: {}                      # no default: required
  hours: { default: 8 }
uses: command
with:
  in: server
  run: ["server", "session", "mint", "--user", "${args.user}", "--hours", "${args.hours}"]
```

`args` is allowed only with `action: true`. An action is read and checked with everything else, whether or not anyone asks for it. A unit that comes up and needs an action no one asked for is an error.

## Modules

Six modules ship with tay. Each is a mechanism; none knows any product. A module's settings are checked strictly against the tables below.

Three of them **run** something: `process`, `container` and `command`. The other three **make** something that then simply exists: `port`, `file` and `secret`. The difference matters in one place, what using a unit's value does to the plan (see Values).

### `process`: a command run on the host

| Setting | Meaning |
|---|---|
| `command` | Required. A list: the program and its arguments. Never a string, and never run through a shell |
| `workdir` | Folder to run in. Default: the stack's root |
| `env` | Environment variables, a mapping from name to value |
| `secret_env` | The same, for values that are secret (see Secrets) |
| `inherit` | Names of variables to pass through from the environment tay was started in. Default: `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR`. Nothing else is inherited |
| `prepare` | A list of commands (each a list) run to completion, in order, before the first start |

### `container`: an image run through Docker

| Setting | Meaning |
|---|---|
| `image` | The image to run. Exactly one of `image` and `build` |
| `build` | `{ context, dockerfile, args }`: build the image from a folder |
| `command`, `entrypoint` | Lists, replacing the image's own |
| `env`, `secret_env` | As for `process` |
| `volumes` | Named volumes this unit owns: a mapping from a name to a path in the container, or to `{ to, owner }` where `owner` is the numeric user the volume is given to. tay gives it by running `chown` as root from the unit's own image, once, when the volume is made; an image with no `chown` cannot have an `owner`, and a `command` unit of the stack must give the volume instead |
| `mounts` | A list. Each is `{ from, to, readonly }`, a host file or folder made visible in the container, or `{ volume, to, readonly }`, a volume another unit owns, written `"unit/name"`. `readonly` defaults to `true` |
| `tmpfs` | A list of paths in the container that are kept in memory |
| `publish` | A mapping from a container port to a host port, or to the long form `{ port, address, always }`. See "When a published port is opened" |
| `aliases` | Further names for this container on the environment's network. Its unit name is always one |
| `labels` | A mapping. A label whose name begins `tay.` is tay's own and may not be written |
| `user`, `workdir`, `platform` | As Docker means them |
| `read_only`, `security_opt`, `pids_limit` | As Docker means them |

Every container joins the environment's one network. That network belongs to the environment, not to any unit, and has no definition; `${environment.network}` is its name. Every container carries the labels `tay.environment`, the environment's id, and `tay.unit`, the unit's name.

A mount's `from` under `.tay/` (the state and log folders) or in the shared folder need not exist when `tay plan` runs; if it is still missing when the unit starts, tay creates it as a folder. Anywhere else it must exist when `tay plan` runs, unless it is built from a value produced by running. Mounting another unit's volume orders nothing: if the units must start in an order, `needs` says so.

### `command`: something run once

| Setting | Meaning |
|---|---|
| `run` | Required. A list (program and arguments), or `{ shell: "..." }` when a shell is wanted |
| `check` | Same form. Exit 0: already done, do not run. Exit 1: not done, run. Any other exit: cannot tell, and the unit fails. With no `check`, the command runs every time the environment is brought up |
| `undo` | Same form. What wiping this unit runs |
| `when_removed` | `keep` (the default) or `undo`. With `undo`, tay runs `undo` when the unit leaves the plan of an environment that had run it, with what the unit last ran with and while the units it needed are still up. Requires `undo`, and is not allowed on an action |
| `in` | Run inside this container unit's container. At most one of `in` and `image`. It is a need for that unit, and writing the unit in `needs` as well is an error |
| `image` | Run as a one-off container from this image, on the environment's network. `run` is then the whole command, in place of the image's entrypoint. With it: `mounts` as for `container` |
| `user` | With `in` or `image`: the user to run as |
| `env`, `secret_env`, `workdir` | As for `process` |
| `stdin` | A file fed to `run` on standard input |
| `produces` | The values the command writes out when it runs (see Values): a list of names, or a mapping from a name to `{ secret: true }` where a value is a credential |
| `outputs` | A mapping from a name to a file the command builds, as a path on the host |

With neither `in` nor `image` the command runs on the host, with the same environment rules as `process`.

### `port`: a host port for this environment

| Setting | Meaning |
|---|---|
| `prefer` | A number to try first |
| `fixed` | A number that must be had. At most one of `prefer` and `fixed` |

Gives `number`.

### `file`: a file written from values

| Setting | Meaning |
|---|---|
| `content` | The text, with references. Exactly one of `content` and `template` |
| `template` | A file whose text is the content; references in it are filled in. `$${` writes a literal `${` |
| `name` | The file's name, written out: no reference. Default: the unit's name |
| `folder` | A named folder to write it in, written out: no reference. File units that name the same folder write side by side. Default: a folder named after the unit |
| `mode` | Permissions, as a string such as `"0600"`. Default `"0644"`, or `"0600"` if the content uses a secret. A `mode` that is written wins |
| `overwrite` | `false` for a file another program rewrites: tay writes it once and then leaves it alone, without reading it again. Default `true` |

The file is written inside the environment's state folder, at `files/<folder>/<name>`, never among the checkout's own files. It belongs to the account tay runs as. On Linux a container sees a mounted file's real owner and mode, so a file that holds a secret, readable by its owner alone, cannot be read by a container that runs as another user: give that container its secret through its environment (`secret_env`), or have a step of the stack give that user a copy of its own. Docker on a Mac hides file owners from containers, so only a Linux machine shows this. Gives `path` and `dir`, the folder. Two file units with the same `folder` and the same `name` are an error. Using `dir` brings in this unit's file only: a unit that wants the folder's other files needs their units.

### `secret`: a value generated once and kept

| Setting | Meaning |
|---|---|
| `kind` | `password`, `bytes` or `keypair` |
| `length` | For `password`: how many characters, default 32. For `bytes`: how many bytes, default 32. At most 65536 |
| `encoding` | For `bytes`: `hex` (the default), `base64` (padded) or `base64url` (not padded) |
| `algorithm` | For `keypair`, and required there: `ed25519` or `rsa-2048`. *`ed25519` is not built yet: a unit that names it is an error.* |

A `password` is letters and digits only, so it is safe inside a URL, a JSON string or a shell word. Gives `value` (for `password` and `bytes`), or `public` and `private` (for `keypair`, as PEM text). Everything a `secret` unit gives is secret, except a key pair's `public`.

## Values and references

A unit gives values; other units use them by reference. A reference is `${...}` inside a string. `$${` writes a literal `${`.

| Reference | Is |
|---|---|
| `${unit.name}` | A value the unit gives |
| `${unit.outputs.name}` | A file a `command` unit builds |
| `${self.name}` | A value this same unit gives |
| `${settings.a.b}` | A setting |
| `${environment.name}` and the rest | This environment (see below) |
| `${host.listen}` | The address a host process listens on so that this machine's containers can reach it and other machines cannot. It is `127.0.0.1` where Docker runs in a virtual machine of its own, as on a Mac, and the host's address on the environment's network where Docker runs on the host itself, as on Linux. When nothing in the plan runs in a container there is no container to reach the process, so it is `127.0.0.1` on every system, and a stack of host processes alone needs no Docker |
| `${params.name}` | A template's parameter. Templates only |
| `${args.name}` | An action's argument. Actions only |

- References are resolved on the parsed structure, never by pasting text into YAML. A value cannot change the shape of a definition.
- A string that is one reference and nothing else keeps the type of what it refers to, so `"${web-port.number}"` is a number where a number is wanted. Anything else becomes text.
- References are allowed in values, not in keys, and not in `uses`, `from`, `needs`, `after`, `action` or names.
- Nothing reads the environment variables tay was started with. A definition gets outside values through settings.
- There are no conditions and no operations on text. A value is used as it is.
- A `$` cannot be written directly before a reference: `$${` always writes a literal `${`.

### The environment

| Reference | Is |
|---|---|
| `${environment.name}` | The name of the folder the stack's root is |
| `${environment.id}` | A short id that no other environment on this machine has |
| `${environment.label}` | What tells this environment's hostnames from another's: the folder's name with the stack's name and a hyphen taken off the front, cut down to what a hostname allows. Empty when the folder is named after the stack. A choice may set it |
| `${environment.label_suffix}`, `${environment.label_prefix}` | `-label` and `label-`, or nothing when the label is empty |
| `${environment.root}`, `${environment.state}` | The stack's root folder, and this environment's state folder, which is `.tay/state` under the root |
| `${environment.logs}` | The folder tay writes logs in, one folder per unit. It is `.tay/logs` under the root |
| `${environment.shared}` | A folder shared by all of one person's environments of this stack, for what is costly to fetch and safe to share. It is `~/.local/share/tay/<stack>`, or under `XDG_DATA_HOME` where that is set |
| `${environment.network}` | The name of the environment's Docker network |
| `${environment.profile}` | The profile in use |

### Known before start, or produced by running

Every value is one or the other.

- **Known before anything starts:** a port's number, a setting, a generated secret, a file's path, anything about the environment, an endpoint, and any `gives` built only from these. The planner works all of them out first. A circle among them (A's value built from B's, B's from A's) is an error.
- **Produced by running:** the names a `command` unit lists under `produces`, and its `outputs`. A `gives` built from a produced value is itself produced. A command hands its produced values to tay by writing them as one JSON object to the file named in the environment variable `TAY_VALUES`; both `check` and `run` may do so, so a unit that was already done still gives its values. That file is in the place the command runs, and tay fetches it from there.

What using a value does to the plan depends on whose value it is. A value built from other values uses those too. What a unit gives is used by whoever uses it, not by the unit that gives it.

| Using | Does |
|---|---|
| Any value of a unit that is made (a `port`, a `file`, a `secret`) | Brings that unit into the plan and is a need for it. A made unit built only from values known before start is made before anything starts |
| A produced value | Brings the producing unit into the plan and is a need for it. The plan says so |
| Any other value of a unit that runs: an endpoint, or a `gives` known before start | Nothing. It orders nothing and does not bring the unit up, and it is allowed when the unit is not in the plan |

Waiting for a unit that runs is written with `needs`, or follows from a command's `in`. Using a unit's address is never a reason to wait for it.

### Endpoints: an address depends on who is asking

A value written as a mapping is an endpoint, and must have `port`. It may have `scheme`, `path` and `host`. A unit that is made runs nowhere, so an endpoint it gives must write its `host`.

```yaml
gives:
  api: { port: "${api-port.number}", scheme: http, path: "/v1" }
```

For a `process` unit, `port` is the host port it listens on, and the process must listen there on `${host.listen}`: that is the one address which both this machine's containers and the host itself, tay's own checks included, can reach on every system. For a `container` unit, `port` is a port inside the container; if `publish` maps it, the endpoint also has a host port.

An endpoint is used through its parts.

| Reference | Gives |
|---|---|
| `${unit.api}` | `host:port` |
| `${unit.api.url}` | `scheme://host:port/path`. The port is left out when it is the scheme's usual one (80 for `http`, 443 for `https`, and no other), and with no `path` nothing follows the port. An endpoint with no `scheme` has no `.url` |
| `${unit.api.host}`, `${unit.api.port}` | The two halves |

Unless the endpoint writes its `host`, each part depends on where the unit that finally uses it runs. A `gives` built from an endpoint, such as `"${self.api.url}/health"`, is carried the same way: it is worked out for whoever uses it, not for the unit that gives it.

| The unit that uses it runs | The endpoint belongs to a `process` | The endpoint belongs to a `container` |
|---|---|---|
| On the host (a `process`, a host `command`, or a readiness check tay makes itself) | The address `${host.listen}` names, and its port | `127.0.0.1` and the published host port. If the port is not in `publish`, that is an error naming both units |
| In a container (a `container`, or a `command` with `in` or `image`) | The name containers have for the host, and its port | The container's unit name and the port inside it |

A view may be asked for by name, whatever the user is. It may be written on a reference to any value of a unit, and settles every endpoint that value is built from.

- `${unit.api.url@host}` is the address as the host sees it.
- `${unit.api.url@net}` is the address as a container of this environment sees it.
- `${wiki.web.url@proxy}` is the address as the unit `proxy` sees it, under the way that unit is running. It is for an address one unit writes down and another uses: a setup step that records, for the proxy, where the wiki is. The unit named must be one that runs.

A unit that is made runs nowhere, so an address used in a `file` unit's content must name its view, unless the endpoint writes its `host`.

**An address that is the same for everyone.** An endpoint that writes its `host` has no views: every user gets that host and that port. This is the form for an address a browser uses.

```yaml
gives:
  public: { host: "portal${environment.label_suffix}.${settings.base_domain}", port: "${settings.https_port}", scheme: https }
```

In a checkout named `example-fix-login`, with the port set to 8443, `${portal.public.url}` is `https://portal-fix-login.example.test:8443`. In the checkout named `example`, with the port 443, it is `https://portal.example.test`.

**When a published port is opened.** A port in `publish` is opened on the host only if something in the plan uses that endpoint from the host: a unit on the host that uses it, a readiness check tay makes itself, or a reference that asks with `@host` or with `@` a unit that runs on the host. It is opened on 127.0.0.1, or on the long form's `address`. `always: true` in the long form opens it with no such use, for a port that a person or another machine uses. The plan says, for each published port, whether it is opened and what opens it. An entry that is not opened uses nothing: a `port` unit named only there does not come into the plan.

## Readiness and restarts

```yaml
ready: { http: "${self.api.url}/health", within: 2m }
ready: { tcp: "${self.db}", within: 30s }
ready: { command: ["pg_isready", "-U", "postgres"], within: 2m }
ready: { log: "listening on", within: 1m }
```

- One check per unit: `http` (any 2xx answer), `tcp` (a connection opens), `command` (exits 0), or `log` (a line matches). `within` is the time allowed, as a number with `s`, `m` or `h`, and is required.
- `http` and `tcp` checks are made by tay from the host, so the endpoint they use is seen from the host. For a `container`, a `command` check runs inside the container. *An `http` check of an `https` address is not built yet: it is an error where the address is known when the stack is read, and fails the unit as it starts where it is known only then.*
- With no `ready`: a `command` is ready when it exits 0; a `port`, `file` or `secret` is ready when it exists; a `container` is ready when its image's own health check passes, or when it has started if the image has none; a `process` is ready when it has started.
- `restart` is `{ on: failure | always | never, attempts: N }`. Default: `failure` for `process` and `container`, `never` for everything else. `attempts` defaults to 5.

`watch` is a list of files and folders. `tay plan` checks that they exist and reports them; what watching does is for the engine.

## Settings

A setting is a named value the stack declares and an environment may set.

```yaml
# tay.yaml
settings:
  base_domain:   { default: example.test, doc: "The domain every hostname is built on." }
  log.web:       { default: info }
  images.server: {}                       # no default: must be set where it is used
  licence_key:   { secret: true }
```

- A name is dotted, each part lower-case letters, digits, hyphens or underscores. A value is text, a whole number or true/false; its type is its default's, or `type: text | number | boolean` when there is no default, or text when it has neither.
- A setting's value is literal. A `${` in it is not a reference.
- Using or setting a setting the stack does not declare is an error.
- A setting with no value and no default is an error only if a unit in the plan uses it. The error names the declaration and every use.
- `secret: true` makes its value secret.

## Choices: profiles and the environment

What comes up, which way each unit runs, and the values of settings are choices. They are written in the same shape in four places, later ones winning:

1. the stack's defaults (`up`, `run` and the defaults of `settings` in `tay.yaml`);
2. the profile (`stack/profiles/<name>.yaml`);
3. this person's own file (`~/.config/tay/<stack>.yaml`), read only when the profile is not a production one;
4. this environment's file (`.tay/choices.yaml`), then what the environment remembers being asked for by earlier `tay up`s, and then each `--set` on the command line, in the order written.

Which profile is used is itself chosen: by `--profile` on the command line, now or remembered from an earlier `tay up`; otherwise by `profile: <name>` in this environment's file; otherwise by the stack's `default_profile`. The environment's file is the one place a person writes it, because that file is kept when everything else the environment keeps is wiped: a server whose profile was only remembered would plan the default profile after `tay wipe --all`. Written in a profile or in a person's own file, `profile` is an error. Every profile of the stack is checked against it, whichever is chosen: a unit, a way or a setting that a profile names and the stack does not have is an error on every machine, so a mistake in the profile a server uses shows where the stack is written.

```yaml
# stack/profiles/dev.yaml
production: false
up: [core, tools]                          # units and groups to bring up
run:
  default: [source-in-container, source]   # the way to use: the first of these a unit has
  units: { server: image }                 # and exceptions
settings:
  base_domain: example.test
```

- `up` from a later place replaces the earlier list. `up_add` and `up_remove` adjust it instead. If no place gives `up`, that is an error: tay does not guess what a stack's whole is.
- `run.default` is one name or a list of them, and from a later place replaces the earlier one. `run.units` is joined, later entries winning. A name in `run.default` that no unit of the stack has as a way is an error.
- `settings` is joined, later values winning.
- `label` sets the environment's label, in place of the one worked out from the folder's name. It may be empty.
- `--set` takes a place in this shape and a value: `up`, `up_add`, `up_remove`, `run.default`, `run.units.<unit>`, `settings.<name>` or `label`. A list is written with commas.
- `production` and `require` may be written only in a profile.
- A unit with ways uses `run.units.<unit>` if given, otherwise the first name in `run.default` that is one of its ways. If the plan reaches the unit and neither gives a way, that is an error that lists the unit's ways.
- **What comes up** is the units in `up`, everything they need, everything those need, under the ways chosen, and every made unit whose value one of them uses. `after` never brings a unit in. A unit with `action: true` comes up only when named.

A profile may also say which settings it cannot do without:

```yaml
# stack/profiles/deploy.yaml
production: true
require: [base_domain, licence_key]
```

A setting in `require` must be given a value that is not empty by a choice: the profile, this person's own file where it is read, the environment's file or `--set`. The stack's default does not count, and it does not matter whether a unit in the plan uses the setting.

### Production profiles

A profile with `production: true` is checked after every choice is applied, and the whole plan is refused, naming each unit, if it contains any of:

- a `process` unit, or a `command` unit that runs on the host;
- a `container` with `build`;
- a unit with `watch`;
- a mount whose source is neither under the stack's root nor in the environment's shared folder, and does not begin with a setting's value, written there directly or reached through a value that begins with one. A source that continues with a value produced by running is refused as well, since where it leads cannot be known before anything starts.

## Secrets

A value is secret if it comes from a `secret` unit (a key pair's `public` excepted), from a setting declared `secret: true`, or from a `produces` entry declared `secret: true`, or is built from one.

- A secret may be used in `secret_env`, in a `file` unit's content (the file is then mode `0600` unless its `mode` is written), and in another unit's `gives`. Used anywhere else (plain `env`, a command line, a label, a readiness check) it is an error: those places can be read by other programs.
- `tay plan` never prints a secret's value. It prints `<secret>`.
- Every value in `secret_env` is a reference to a secret, or text built around one. A value there with no secret in it is an error, in every profile: a secret written into a definition is not a secret.

## What `tay plan` does

`tay plan [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]`

It reads everything above, applies the choices, and prints the plan: each unit that would come up, in the order it could start, with the module it uses (and which way), where it runs, what it needs and why (a need it wrote, a command's `in`, or a value it uses), what it waits for with `after`, how it is known to be ready, the host ports it opens and what opens each, and the values it gives. Of `.tay/` it reads `choices.yaml` and what the environment was last asked for (see `engine.md`), and it never writes.

- `tay plan` allocates nothing and runs nothing, so what only allocating or running can tell prints as a placeholder: a port as `<port: unit>`, a produced value as `<produced: unit.name>`, a key pair's public half as `<generated: unit.public>`, an action's argument as `<arg: name>`, and `<environment.id>`, `<environment.network>` and `<host.listen>`. A secret prints as `<secret>`. A built file prints as its whole path. All but the produced values and the built files still count as known before start.
- Errors that can be judged only on a sound reading (an unset setting, a circle, an address with no view, a production refusal) are reported once the reading itself has no errors.
- `--json` prints the same plan in a versioned schema, for programs.
- Exit codes: 0 the plan is sound; 2 the definitions have errors; 3 this tay is too old for them.
- An error is one line, `file:line:column: what is wrong`, followed where it helps by a second line pointing at the other place involved (the first definition of a duplicate, the declaration of a setting, the other end of a circle).

## Not in version 1

Units written in anything but YAML. Templates that use templates. Lists and mappings as setting values. Reading the invoking shell's environment. Conditions, and operations on text such as escaping a value for JSON or a URL. A way for a stack to add modules of its own: anything the six do not cover is a `command`.

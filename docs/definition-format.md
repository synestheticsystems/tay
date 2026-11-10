# The definition format, version 1

A stack's definitions say what the stack is made of and what each part needs. tay reads them, works out every value that can be known before anything starts, puts the units in order, and only then starts anything. This document is the contract for the first half of that: what a definition may contain and what `tay plan` does with it. Nothing described here starts a process or a container.

Two rules run through all of it.

- **Strict.** An unknown key, a wrong type, a duplicate name, or a reference to something that does not exist is an error with the file, line and column. tay reports every error it can find in one run, and starts nothing if there is any.
- **Blind.** The planner knows units, their needs and their values. It never asks which module a unit uses, except to hand that module its own settings to check.

Words used here: a **stack** is everything one `tay.yaml` describes. An **environment** is one running copy of a stack. A **unit** is one thing tay brings up. A **module** is the mechanism a unit uses.

## Files

| File | What it holds |
|---|---|
| `tay.yaml` | Marks the root of a stack. tay finds it by walking up from the current directory |
| `stack/units/<name>/unit.yaml` | One unit, named after its folder. Other files in the folder are that unit's own |
| `stack/units/<anything>.yaml` | Several units in one file: a mapping from unit name to unit |
| `stack/templates/<name>.yaml` | A unit shape the stack repeats |
| `stack/profiles/<name>.yaml` | A named set of choices: what comes up, which way units run, values of settings |
| `.tay/` | This environment's own choices and state. Never committed. `tay plan` reads it and never writes it |

### `tay.yaml`

```yaml
format: 1                    # required; the version of this document
requires: ">=0.1.0"          # optional; the oldest tay that may read these definitions
stack: example               # required; the stack's name
default_profile: dev         # required if there is more than one profile
units_dir: stack/units       # optional; this is the default
templates_dir: stack/templates
profiles_dir: stack/profiles
settings:                    # optional; the settings this stack has (see Settings)
  log.web: { default: info }
units:                       # optional; units written here instead of in files
  web-port: { uses: port }
```

A small project may keep everything in `tay.yaml` and have no `stack/` folder. Units from `units:` and from files form one set.

A tay that is older than `requires`, or that does not know `format`, stops with the version it would need. It does not guess.

### Names

A unit, template, profile or alternative name is lower-case letters, digits and hyphens, starts with a letter, and is at most 40 characters. Unit names are unique across the whole stack; a duplicate is an error that names both places. The names `settings`, `environment`, `host`, `self` and `params` are not available to units, because references use them.

## A unit

Every unit has the same fields, whatever it uses.

| Field | Meaning |
|---|---|
| `doc` | One line saying what the unit is for |
| `uses`, `with` | The one module, and that module's own settings |
| `run` | In place of `uses`: the ways this unit can run, by name, each with its own `uses` and `with` |
| `from`, `params` | In place of `uses`: a template of this stack, and the values to fill it with |
| `needs` | Units that must be ready before this one starts |
| `after` | Units to wait for, ready or failed, without depending on them |
| `ready` | How readiness is checked, and the time allowed |
| `restart` | What to do when it exits |
| `watch` | Folders whose change runs the unit again. Development only |
| `gives` | Named values other units may use |
| `action` | `true` for a unit that runs only when asked for by name |

A unit has exactly one of `uses`, `run` or `from`, or none of them. A unit with none is a **group**: it may have only `doc` and `needs`, and asking for it asks for everything it needs.

```yaml
# stack/units/web/unit.yaml
doc: The site's development server.
needs: [web-port]
uses: process
with:
  command: ["node", "node_modules/vite/bin/vite.js", "--port", "${web-port.number}", "--strictPort"]
ready: { http: "${self.site.url}", within: 2m }
gives:
  site: { port: "${web-port.number}", scheme: http }
```

### More than one way to run

`run` lists alternatives. The rest of the stack refers to the unit, never to an alternative.

```yaml
needs: [database]
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

- An alternative may have `uses`, `with`, `needs`, `after`, `ready`, `restart`, `watch` and `gives`. The unit's own `needs`, `after` and `gives` apply to every alternative and are joined with the alternative's.
- After joining, every alternative of a unit must give the same value names. If one gives `api` and another does not, that is an error: a consumer must not break when the unit is switched.
- Which alternative is used is a choice of the environment (see Choices). A unit with `run` and no choice that names one of its alternatives is an error.

### Templates

A template is a unit shape with holes.

```yaml
# stack/templates/postgres-database.yaml
params:
  name: {}                      # no default: required
  owner: { default: app }
unit:
  uses: command
  needs: [postgres]
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

A unit made `from` a template may have only `doc`, `from`, `params`, `needs` and `after`; its `needs` and `after` are joined with the template's. A missing required parameter, or a parameter the template does not declare, is an error. A template may not itself use `from`.

## Modules

Six modules ship with tay. Each is a mechanism; none knows any product. A module's settings are checked strictly against the tables below.

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
| `volumes` | Named volumes this unit owns: a mapping from a name to a path in the container, or to `{ to, owner }` where `owner` is the numeric user the volume is given to |
| `mounts` | A list of `{ from, to, readonly }`: a host file or folder made visible in the container. `readonly` defaults to `true` |
| `publish` | A mapping from a container port to a host port. Published on 127.0.0.1 unless the long form `{ port, address }` says otherwise |
| `aliases` | Further names for this container on the environment's network. Its unit name is always one |
| `labels` | A mapping |
| `user`, `workdir`, `platform` | As Docker means them |
| `read_only`, `security_opt`, `pids_limit` | As Docker means them |

Every container joins the environment's one network. That network belongs to the environment, not to any unit, and has no definition.

### `command`: something run once

| Setting | Meaning |
|---|---|
| `run` | Required. A list (program and arguments), or `{ shell: "..." }` when a shell is wanted |
| `check` | Same form. Exit 0: already done, do not run. Exit 1: not done, run. Any other exit: cannot tell, and the unit fails |
| `undo` | Same form. What wiping this unit runs |
| `in` | Run inside this container unit's container. At most one of `in` and `image` |
| `image` | Run as a one-off container from this image, on the environment's network. With it: `mounts`, `user`, `workdir` as for `container` |
| `env`, `secret_env`, `workdir` | As for `process` |
| `stdin` | A file fed to `run` on standard input |
| `produces` | A list of names: values the command writes out when it runs (see Values) |
| `outputs` | A mapping from a name to a file the command builds |

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
| `name` | The file's name. Default: the unit's name |
| `mode` | Permissions, as a string such as `"0600"`. Default `"0644"`, or `"0600"` if the content uses a secret |

The file is written inside the environment's state folder, never inside the checkout. Gives `path` and `dir`.

### `secret`: a value generated once and kept

| Setting | Meaning |
|---|---|
| `kind` | `password`, `hex` or `keypair` |
| `length` | For `password` and `hex`: how long. Defaults 32 and 32 bytes |
| `algorithm` | For `keypair`: `ed25519` or `rsa-2048` |

Gives `value` (for `password` and `hex`), or `public` and `private` (for `keypair`, as PEM text). Everything a `secret` unit gives is secret.

## Values and references

A unit gives values; other units use them by reference. A reference is `${...}` inside a string. `$${` writes a literal `${`.

| Reference | Is |
|---|---|
| `${unit.name}` | A value the unit gives |
| `${unit.outputs.name}` | A file a `command` unit builds |
| `${self.name}` | A value this same unit gives |
| `${settings.a.b}` | A setting |
| `${environment.name}` `.id` `.label` `.root` `.state` `.profile` | This environment: its name, its id, its name cut down to what a hostname allows, the stack's root folder, the state folder, the profile in use |
| `${host.listen}` | The address a host process should listen on so that this machine's containers can reach it and other machines cannot |
| `${params.name}` | A template's parameter. Templates only |

- References are resolved on the parsed structure, never by pasting text into YAML. A value cannot change the shape of a definition.
- A string that is one reference and nothing else keeps the type of what it refers to, so `"${web-port.number}"` is a number where a number is wanted. Anything else becomes text.
- References are allowed in values, not in keys, and not in `uses`, `from`, `needs`, `after`, `action` or names.
- Nothing reads the environment variables tay was started with. A definition gets outside values through settings.

### Known before start, or produced by running

Every value is one or the other, and the difference decides what waits for what.

- **Known before anything starts:** a port's number, a setting, a generated secret, anything about the environment, an endpoint, and any `gives` built only from these. Using one orders nothing. The planner works all of them out first. A circle among them (A's value built from B's, B's from A's) is an error.
- **Produced by running:** the names a `command` unit lists under `produces`, and its `outputs`. Using one is a need: the planner adds the producing unit to the user's needs and says so in the plan. A `gives` built from a produced value is itself produced. A command hands its produced values to tay by writing them as one JSON object to the file named in the environment variable `TAY_VALUES`; both `check` and `run` may do so, so a unit that was already done still gives its values.

Waiting for a unit to be ready is always written with `needs`. Using a unit's address is never a reason to wait for it.

### Endpoints: an address depends on who is asking

A value written as a mapping with `port` is an endpoint.

```yaml
gives:
  api: { port: "${api-port.number}", scheme: http, path: "/v1" }
```

For a `process` unit, `port` is the host port it listens on. For a `container` unit, `port` is a port inside the container; if `publish` maps it, the endpoint also has a host port.

An endpoint is used through its parts, and each part depends on where the user of it runs:

| Reference | Gives |
|---|---|
| `${unit.api}` | `host:port` |
| `${unit.api.url}` | `scheme://host:port/path` |
| `${unit.api.host}`, `${unit.api.port}` | The two halves |

| The unit that uses it runs | The endpoint belongs to a `process` | The endpoint belongs to a `container` |
|---|---|---|
| On the host (a `process`, a host `command`, or a readiness check tay makes itself) | `127.0.0.1` and its port | `127.0.0.1` and the published host port. If the port is not published, that is an error naming both units |
| In a container (a `container`, or a `command` with `in` or `image`) | The name containers have for the host, and its port | The container's unit name and the port inside it |

`@host` or `@net` after the name asks for one view whatever the user is: `${unit.api.url@host}`. The address a browser uses is not an endpoint. It is an ordinary value the stack builds from its hostnames and its proxy's port.

## Readiness and restarts

```yaml
ready: { http: "${self.api.url}/health", within: 2m }
ready: { tcp: "${self.db}", within: 30s }
ready: { command: ["pg_isready", "-U", "postgres"], within: 2m }
ready: { log: "listening on", within: 1m }
```

- One check per unit: `http` (any 2xx answer), `tcp` (a connection opens), `command` (exits 0), or `log` (a line matches). `within` is the time allowed, as a number with `s`, `m` or `h`.
- `http` and `tcp` checks are made by tay from the host, so the endpoint they use is seen from the host. For a `container`, a `command` check runs inside the container.
- With no `ready`: a `command` is ready when it exits 0; a `port`, `file` or `secret` is ready when it exists; a `container` is ready when its image's own health check passes, or when it has started if the image has none; a `process` is ready when it has started.
- `restart` is `{ on: failure | always | never, attempts: N }`. Default: `failure` with 5 attempts for `process` and `container`, `never` for everything else.

`watch` is a list of folders. `tay plan` checks that they exist and reports them; what watching does is for the engine.

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

- A name is dotted, each part lower-case letters, digits, hyphens or underscores. A value is text, a whole number or true/false; its type is its default's, or `type: text | number | boolean` when there is no default.
- Using or setting a setting the stack does not declare is an error.
- A setting with no value and no default is an error only if a unit in the plan uses it. The error names the declaration and every use.
- `secret: true` makes its value secret.

## Choices: profiles and the environment

What comes up, which way each unit runs, and the values of settings are choices. They are written in the same shape in four places, later ones winning:

1. the stack's defaults (`settings` in `tay.yaml`);
2. the profile (`stack/profiles/<name>.yaml`);
3. this person's own file (`~/.config/tay/<stack>.yaml`), read only when the profile is not a production one;
4. this environment's file (`.tay/choices.yaml`), and then `--set` on the command line.

```yaml
# stack/profiles/dev.yaml
production: false
up: [core, tools]                 # units and groups to bring up
run:
  default: source                 # the alternative to use where a unit has it
  units: { server: image }        # and exceptions
settings:
  base_domain: example.test
```

- `up` from a later place replaces the earlier list. `up_add` and `up_remove` adjust it instead.
- `run.default` from a later place replaces the earlier one; `run.units` is joined, later entries winning.
- `settings` is joined, later values winning.
- A unit with alternatives uses `run.units.<unit>` if given, otherwise `run.default` if the unit has an alternative of that name; otherwise it is an error that lists the unit's alternatives.
- **What comes up** is the units in `up`, everything they need, and everything those need, under the alternatives chosen. `after` never brings a unit in. A unit with `action: true` comes up only when named.

### Production profiles

A profile with `production: true` is checked after every choice is applied, and the whole plan is refused, naming each unit, if it contains any of:

- a `process` unit, or a `command` unit that runs on the host;
- a `container` or `command` with `build`, or a `process` or `command` with `prepare`;
- a unit with `watch`;
- a mount whose source is outside the stack's own folders and is not named by a setting.

## Secrets

A value is secret if it comes from a `secret` unit, from a setting declared `secret: true`, or is built from one.

- A secret may be used in `secret_env`, in a `file` unit's content (the file is then mode `0600`), and in another unit's `gives`. Used anywhere else (plain `env`, a command line, a label, a readiness check) it is an error: those places can be read by other programs.
- `tay plan` never prints a secret's value. It prints `<secret>`.
- `secret_env` takes only references to secret values. A literal there is an error, in every profile: a secret written into a definition is not a secret.

## What `tay plan` does

`tay plan [--profile NAME] [--set KEY=VALUE]... [--json]`

It reads everything above, applies the choices, and prints the plan: each unit that would come up, in the order it could start, with the module it uses (and which alternative), where it runs, what it needs and why (a need it wrote, or a produced value it uses), what it waits for with `after`, how it is known to be ready, and the values it gives. It reads the environment's state if there is any, and never writes.

- A port that has not been allocated yet prints as `<port: unit>`, and a secret as `<secret>`; both still count as known before start.
- `--json` prints the same plan in a versioned schema, for programs.
- Exit codes: 0 the plan is sound; 2 the definitions have errors; 3 this tay is too old for them.
- An error is one line, `file:line:column: what is wrong`, followed where it helps by a second line pointing at the other place involved (the first definition of a duplicate, the declaration of a setting, the other end of a circle).

## Not in version 1

Units written in anything but YAML. Templates that use templates. Lists and mappings as setting values. Reading the invoking shell's environment. A way for a stack to add modules of its own: anything the six do not cover is a `command`.

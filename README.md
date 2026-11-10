# tay

Topology Automation via YAML.

tay brings a whole stack up from one command and then keeps telling the truth about it: what is building, what is ready, what failed and why. A stack is described as units (host processes, containers, one-off commands, ports, files, secrets) with the dependencies between them, in YAML kept in the project's own repository. tay runs many copies of a stack on one machine at once, one per folder, and the same definitions on a server.

## Status

tay reads a stack's definitions and plans it: `tay plan` checks the definitions strictly, works out every value known before anything starts, and prints the units that would come up in an order they could start in. `tay up` brings a plan up through the environment's daemon: it makes ports, secrets and files, runs processes and commands on the host, each under a keeper that outlives the daemon, and runs containers, and commands in them, through Docker. `tay down`, `tay status`, `tay logs` and `tay ls` are built. The definitions are written as [docs/definition-format.md](docs/definition-format.md) describes, and the engine as [docs/engine.md](docs/engine.md) does.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

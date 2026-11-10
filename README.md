# tay

Topology Automation via YAML.

tay brings a whole stack up from one command and then keeps telling the truth about it: what is building, what is ready, what failed and why. A stack is described as units (host processes, containers, one-off commands, ports, files, secrets) with the dependencies between them, in YAML kept in the project's own repository. tay runs many copies of a stack on one machine at once, one per folder, and the same definitions on a server.

## Status

This is the placeholder era. The name is taken and the work has begun; there is nothing to install yet. Version 0.0.0 is a binary that says so and exits with a failure code.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

# cargo-pros

> Easily manage vexide projects

cargo-pros is a command line tool that makes it simple to build, upload, and simulate [vexide](https://vexide.dev) projects.

## Install

```bash
cargo install cargo-pros
```

## Usage

Build a vexide project:

```bash
cargo pros build
```

Upload a vexide project over USB:

```bash
cargo pros upload --slot 1
```

Upload, run, and view the serial output of a vexide project:

```bash
cargo pros upload --slot 1
```

## Config

Run `cargo pros config print` to find the TOML configuration file for your platform.

```toml
[defaults]
slot = 1
```

### Properties

- `defaults.slot` (integer): Set the default program slot to upload to.

## Compatability

By default cargo-pros only supports [vexide](https://crates.io/crates/vexide) projects.
In order to support pros-rs projects you must enable the `legacy-pros-rs-support` feature.

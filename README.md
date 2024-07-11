# cargo-v5

> Easily manage vexide projects

cargo-v5 is a command line tool that makes it simple to build, upload, and simulate [vexide](https://vexide.dev) projects.

## Install

```bash
cargo install cargo-v5
```

## Usage

Build a vexide project:

```bash
cargo v5 build
```

Upload a vexide project over USB:

```bash
cargo v5 upload --slot 1
```

Upload, run, and view the serial output of a vexide project:

```bash
cargo v5 upload --slot 1
```

## Config

Run `cargo pros config print` to find the TOML configuration file for your platform.

```toml
[defaults]
slot = 1
```

### Properties

- `defaults.slot` (integer): Set the default program slot to upload to.

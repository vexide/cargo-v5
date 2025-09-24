# cargo-v5

> Build, upload, run, and simulate Rust projects written for VEX!

cargo-v5 is a command line tool that simplifies working with VEX projects written in Rust (with a focus on the [vexide runtime](https://github.com/vexide/vexide)).

## Installation

### Install via [cargo binstall](https://github.com/cargo-bins/cargo-binstall#cargo-binaryinstall)

> *Supports all platforms*

```sh
cargo binstall cargo-v5
```

### Install via [Homebrew](https://brew.sh)

> *Supports macOS & Linux*

```sh
brew install vexide/vexide/cargo-v5
```

### Install via shell script

> *Supports macOS & Linux (probably not NixOS)*

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/vexide/cargo-v5/releases/latest/download/cargo-v5-installer.sh | sh
```

### Install via PowerShell script

> *Supports Windows*

```powershell
irm https://github.com/vexide/cargo-v5/releases/latest/download/cargo-v5-installer.ps1 | iex
```

### Build from source

cargo-v5 comes with 2 optional features that enable extra functionality:

- `field-control`: Adds a field control tui accesible through `cargo v5 field-control` or `cargo v5 fc`.
- `fetch-template`: With this feature enabled, `cargo v5 new` will attempt to fetch the most recent upstream version of vexide-template instead of a built-in one. The command will always fall back to the built-in template.

If you wish to enable both, you can simply enable the `full` feature.

> [!TIP]
> Pre-built versions of cargo-v5 have all optional features enabled.

#### All Features (enabled by default)

```bash
cargo install cargo-v5
```

#### Specific Feature

```bash
cargo install cargo-v5 --no-default-features --features "field-control"
```

#### No Features

```bash
cargo install cargo-v5 --no-default-features
```

## Usage

Build a vexide project for the V5's platform target:

```bash
cargo v5 build --release
```

Upload a vexide project over USB (you may be prompted to provide a slot number):

```bash
cargo v5 upload
```

View serial output from the current user program:

```bash
cargo v5 terminal
```

## Configuration

Upload behavior can be configured through either your `Cargo.toml` file or by providing arguments to `cargo-v5`.

`cargo-v5` will attempt to find `Cargo.toml` files with the following structure for providing defaults to some upload options.

```toml
[package.metadata.v5]
slot = 1
icon = "cool-x"
compress = true
```

### Properties

- `package.metadata.v5.slot` (integer): Set the default program slot to upload to.
- `package.metadata.v5.icon` (string) (default `"question-mark"`): Set the default program icon. (see `cargo v5 upload -h` for a list of icon strings)
- `package.metadata.v5.compress` (boolean) (default `true`): Configure if program binaries should be gzipped before uploading. It is strongly recommended to keep this at default (`true`), as disabling compression will greatly increase upload times.

`cargo-v5` will also use your project's `package.name` and `package.description` fields for program name/description if nothing is explicitly provided.

For a full list of arguments, check

```
cargo v5 help
```

## Uninstall

If you installed cargo-v5 using the standalone shell or PowerShell scripts above, you can remove it by running this command in your Unix shell:

```sh
rm ~/.local/bin/cargo-v5 ~/.config/cargo-v5/cargo-v5-receipt.json
```

Or by running this in PowerShell:

```powershell
rm ~\.local\bin\cargo-v5 "$env:LOCALAPPDATA\cargo-v5\cargo-v5-receipt.json"
```

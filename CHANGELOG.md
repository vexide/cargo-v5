# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!--
Before releasing:

- change versions in Cargo.toml
- change Unreleased to the version number
- create new Unreleased section
- update links at the end of the document
-->

## [0.12.0]

### Changed

- Updated the builtin template to the latest version.
- `cargo v5 migrate` will migrate to `vexide` 0.8.0 rather than 0.8.0-rc.1.

## [0.12.0]

### Added

- Added a new `migrate` command that updates your project configuration to be compatible with vexide 0.8.0.
- Added a new `kv` command for changing on-device configuration like team number or robot name.
- Added support for the builtin `armv7a-vex-v5` target.

## [0.11.0]

### Added

- Added `self-update` command which downloads the latest version of cargo-v5.

### Fixed

- Fixed builds for `rustc` versions 1.91.0 and above, which made a [breaking change](https://github.com/rust-lang/rust/pull/144443) to the custom target spec JSON schema.

## [0.4.0]

### Added

- Added upload command

### Fixed

### Changed

### Removed

## [0.3.0] - 2024-01-08

### Added

- Added `cargo pros sim` command to easily simulate the current project. Cargo-v5 will use Cargo's project metadata to provide a better simulator experience.

### Removed
[0.12.0]: https://github.com/vexide/cargo-v5/compare/v0.12.0..v0.12.1
[0.12.0]: https://github.com/vexide/cargo-v5/compare/v0.11.0..v0.12.0
[0.11.0]: https://github.com/vexide/cargo-v5/compare/v0.4.0..v0.11.0
[0.4.0]: https://github.com/vexide/cargo-v5/compare/v0.3.0..v0.4.0
[0.3.0]: https://github.com/vexide/cargo-v5/releases/tag/v0.3.0

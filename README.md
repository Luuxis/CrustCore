# CrustCore

[![crates.io](https://img.shields.io/crates/v/crust_core.svg)](https://crates.io/crates/crust_core)
[![docs.rs](https://docs.rs/crust_core/badge.svg)](https://docs.rs/crust_core)
[![license](https://img.shields.io/crates/l/crust_core.svg)](LICENSE)

Core library to download, install and launch Minecraft, written in Rust.

It is meant to be the engine behind a launcher: resolving version manifests,
downloading assets, libraries and the Java runtime, then building and spawning
the game process.

> **Status:** early development. This first release only reserves the crate
> name; the download and launch APIs are not implemented yet.

## Installation

```toml
[dependencies]
crust_core = "1.0"
```

## Usage

```rust
use crust_core::version;

println!("CrustCore {}", version());
```

## Roadmap

- Vanilla version manifest resolution
- Assets, libraries and natives download with hash verification
- Java runtime download (Mojang JRE manifests)
- Launch command construction and process spawning
- Mod loaders: Forge, NeoForge, Fabric, Quilt

## License

MIT. See [LICENSE](LICENSE).

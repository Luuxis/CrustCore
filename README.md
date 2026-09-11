# CrustCore

[![crates.io](https://img.shields.io/crates/v/crust_core.svg)](https://crates.io/crates/crust_core)
[![docs.rs](https://docs.rs/crust_core/badge.svg)](https://docs.rs/crust_core)
[![license](https://img.shields.io/crates/l/crust_core.svg)](LICENSE)

Core library to download, install and launch Minecraft, written in Rust.

It is meant to be the engine behind a launcher: resolving version manifests,
downloading assets, libraries and the Java runtime, then building and spawning
the game process.

> **Status:** early development. Account sign-in is implemented; download and
> launch APIs are not available yet.

## Installation

```toml
[dependencies]
crust_core = "1.0"
```

## Signing in

Sign-in goes through Microsoft OAuth, Xbox Live, XSTS and the Minecraft
services API. The official launcher client id is used by default, so no Azure
registration is needed. To use your own Azure application (public client,
`consumers` tenant, approved by Mojang through <https://aka.ms/mce-reviewappid>)
call `Authenticator::with_client_id`. Game Pass subscribers are supported:
playability is decided by the Minecraft profile, not by store entitlements.

```rust
use crust_core::authenticator::Authenticator;

let auth = Authenticator::new()?;

// Device code flow: show the code to the user, then wait.
let flow = auth.device_code().await?;
println!("Open {} and enter {}", flow.verification_uri(), flow.user_code());
let account = flow.wait().await?;

println!("{} ({}) - {:?}", account.username(), account.uuid(), account.ownership);

// Later, refresh silently from the stored account.
let account = auth.refresh(&account).await?;
```

The authorization code flow is available through `authorize_request` and
`login_with_code` when you prefer a browser redirect.

## Architecture

| Module          | Role                                                             |
|-----------------|------------------------------------------------------------------|
| `foundation`    | Pure types and utilities (Maven, hashes, rules, OS/arch), no I/O |
| `network`       | HTTP engine: requests, download pool, progress                   |
| `providers`     | One typed client per external API (Mojang, Forge, Fabric, ...)   |
| `authenticator` | Session logic: refresh, expiration, offline mode                 |
| `resolver`      | Builds the full list of expected files for a version             |
| `checker`       | Checks what already exists on disk                               |
| `loader`        | Installs mod loaders: Forge, NeoForge, Fabric, Quilt             |
| `launcher`      | Builds the arguments and spawns the game process                 |

## License

MIT. See [LICENSE](LICENSE).

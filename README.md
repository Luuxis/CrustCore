# CrustCore

[![crates.io](https://img.shields.io/crates/v/crust_core.svg)](https://crates.io/crates/crust_core)
[![docs.rs](https://docs.rs/crust_core/badge.svg)](https://docs.rs/crust_core)
[![license](https://img.shields.io/crates/l/crust_core.svg)](LICENSE)

Core library to download, install and launch Minecraft, written in Rust.

It is meant to be the engine behind a launcher: signing in to a Microsoft
account, resolving version manifests, downloading assets, libraries and the
Java runtime, then building the arguments and spawning the game process.

> **Status:** vanilla Minecraft and every mod loader supported by
> [minecraft-java-core](https://github.com/luuxis/minecraft-java-core) are
> implemented with the same logic: Forge, NeoForge, Fabric, LegacyFabric and
> Quilt. Verified on macOS: Forge 1.7.10, 1.12.2, 1.16.5 and 1.20.1, NeoForge
> 1.21.1, Fabric and Quilt 1.20.1, LegacyFabric 1.12.2. Java comes from the
> Mojang runtimes with an Azul Zulu fallback. Microsoft, offline, Yggdrasil and
> AZauth accounts are supported, and the server status ping speaks every
> protocol from Beta 1.8 to the latest release.

## Installation

```toml
[dependencies]
crust_core = "1.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Quick start

```rust
use std::sync::Arc;

use crust_core::authenticator::Authenticator;
use crust_core::foundation::events::Event;
use crust_core::launcher::{Launch, LaunchOptions, LoaderKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Sign in with a device code.
    let auth = Authenticator::new()?;
    let flow = auth.device_code().await?;
    println!("Open {}", flow.verification_uri_complete());
    let account = flow.wait().await?;

    // 2. Describe the launch, the same options as minecraft-java-core.
    let mut options = LaunchOptions::new("./minecraft", "1.20.1");
    options.loader.kind = Some(LoaderKind::Forge);
    options.loader.build = "latest".to_owned();
    options.loader.enable = true;
    options.memory.max = "4G".to_owned();
    options.download_concurrency = 30;

    // 3. Download, install the loader, then start the game.
    let launch = Launch::new(options, account)?.with_events(Arc::new(|event| match event {
        Event::Progress { downloaded, total, element } => println!("{element}: {downloaded}/{total}"),
        Event::Patch(line) => println!("{line}"),
        Event::Error(message) => eprintln!("{message}"),
        _ => {}
    }));
    let mut child = launch.run().await?;
    child.wait().await?;
    Ok(())
}
```

A complete runner lives in [examples/launch.rs](examples/launch.rs) and can be
started with `cargo start`. It mirrors the minecraft-java-core test script: it
stores the account in `data/account.json`, reuses it silently on later runs
through `Authenticator::refresh`, fetches the instance list from a launcher backend, picks
an instance by name and builds the launch options from it (version, loader,
custom files URL, `ignored`, `verify`, `mcp`). The environment variables
`CRUSTCORE_API_URL`, `CRUSTCORE_INSTANCE` and `CRUSTCORE_JVM_ARGS` override
its constants.

## Signing in

Sign-in goes through Microsoft OAuth, Xbox Live, XSTS and the Minecraft
services API. The official launcher client id (`00000000402b5328`) is used by
default on `login.live.com`, so no Azure registration is needed. To use your
own Azure application (public client, `consumers` tenant, approved by Mojang
through <https://aka.ms/mce-reviewappid>) call `Authenticator::with_client_id`.

Two flows are available:

- **Device code**: `device_code()` returns a code and a pre-filled link
  (`verification_uri_complete`), then `wait()` polls until the user signs in.
- **Authorization code**: `authorize_request()` gives the URL to open in a
  browser, then `AuthorizeRequest::login` completes the sign-in from the
  redirect URL.

`Authenticator::refresh` applies the same rule as minecraft-java-core: when the
Minecraft token is still valid for more than two hours only the profile (skins
and capes) is fetched again, otherwise the Microsoft refresh token is used to
sign in again through Xbox Live and the Minecraft services.

The resulting `Account` is serializable and mirrors the shape used by
[minecraft-java-core](https://github.com/luuxis/minecraft-java-core):
`access_token`, `client_token`, `uuid`, `name`, `refresh_token`,
`user_properties`, `meta` (`type`, `access_token_expires_in`, `demo`,
`ownership`, `entitlements`), `xboxAccount` (`xuid`, `gamertag`, `ageGroup`)
and `profile` (`skins`, `capes`). `meta.type` is `Xbox`, `Mojang` or `AZauth`
and drives the `${user_type}` game argument (`msa`, `Mojang`, `AZauth`).

Game Pass subscribers are supported: they do not own the game in the store, so
playability is decided by the Minecraft profile rather than by entitlements.
`meta.ownership` reports `owned`, `game_pass` or `unknown`.

### Offline, Yggdrasil and AZauth accounts

`Yggdrasil` ports the `Mojang` module of minecraft-java-core. `Yggdrasil::offline`
builds an offline account from a username (random hex UUID, `meta.online`
false), and `login` with a password authenticates against a Yggdrasil server:
`https://authserver.mojang.com` by default, or any compatible server given to
`Yggdrasil::with_api` (the `ChangeAuthApi` equivalent). `refresh`, `validate`
and `signout` call `/refresh`, `/validate` and `/invalidate`.

`AzAuth::new(http, "https://my-site.example")` targets an
[Azuriom](https://azuriom.com) site. `login(email, password, code)` returns
`AzAuthLogin::TwoFactorRequired` when the site asks for a 2FA code, otherwise an
`Account` with `user_info` (`id`, `banned`, `money`, `role`, `verified`) and the
skin from the skin API embedded as base64. `verify` refreshes the session and
`signout` invalidates it. The optional `clientId` field of an `Account` feeds the
`${clientid}` game argument, as in the Node library.

## Launching

`Launch` reproduces the `Launch` class of minecraft-java-core step by step, with
the same argument order and the same filtering:

1. `Resolver` reads the Mojang manifest and the version JSON. On Linux ARM the
   LWJGL and JInput libraries are swapped for ARM builds, as in the Node
   library.
2. The version JSON and the asset index are written to disk, then every file
   (libraries, natives, client jar, log config, custom files, assets, Mojang
   Java runtime) is checked by existence, size and SHA-1. Only the missing or
   corrupted ones are downloaded, in parallel with retries.
3. When `loader.enable` is set, the loader is installed under `loader.path`
   (default `./loader`): Forge and NeoForge download the installer, extract the
   universal jar and `client.lzma`, fetch the libraries through the Maven
   mirrors, then run the installer processors with the game's Java. Fabric,
   LegacyFabric and Quilt download the profile JSON and its libraries.
4. When `verify` is set, files that are neither in the bundle nor in `ignored`
   are removed from the instance, exactly like `checkFiles`.
5. Legacy natives are extracted into `versions/<id>/natives` and, for
   `legacy`/`pre-1.6` asset indexes, assets are copied into `resources/`.
6. The command line follows the official launcher: the loader JVM arguments,
   then the `arguments.jvm` list of the version JSON evaluated with Mojang's
   rules (`os.name`, `os.arch`, `os.version`, `features`) and placeholders
   (`${natives_directory}`, `${launcher_name}`, `${launcher_version}`,
   `${classpath}`), which carries `-XstartOnFirstThread`, the Windows heap dump
   trick, `-Xss1M` for 32-bit JVMs, the Java 25 access flags of 26.1+ and
   `-cp`. Versions with `minecraftArguments` get `-Djava.library.path` and
   `-cp` synthesized instead. The launcher's own options (memory, G1 flags, the
   natives directories, `-Xdock`, the log4j configuration, `default-user-jvm`
   and `jvm_args`) follow, then the main class, the game arguments (with
   `is_demo_user` and `has_custom_resolution` features) and the loader game
   arguments. Library rules use the same Mojang semantics, so the
   `allow`/`disallow osx` pairs of 1.14 to 1.18 pick exactly one LWJGL build.

On macOS, JNA older than 5.13.0 aborts the game with `snprintf() output has
been truncated` as soon as the game directory path is long. The `jna` and
`jna-platform` libraries below 5.13.0 are therefore replaced by 5.13.0, both in
the vanilla version JSON and in the Forge and NeoForge loader JSON (Forge ships
its own copy that takes precedence on the classpath), and the `-DmergeModules`
argument is rewritten to name the replaced jars.

On macOS, Forge's early loading window initializes GLFW before the game does
and, on Apple Silicon, leaves a pending `Cocoa: Failed to find service port for
display` error that makes Minecraft 1.14 and 1.15 abort with `GLFW error before
init`. The launcher therefore passes `-Dfml.earlyprogresswindow=false` to Forge
on macOS, which is the fix recommended by Forge itself.

### Java

The Mojang runtime matching the version's `javaVersion.component` is used by
default. On Apple Silicon and Windows ARM64, when Mojang has no ARM build of the
component (`jre-legacy`, `java-runtime-alpha` and `java-runtime-beta`, so every
version before 1.19), the x64 runtime is used under Rosetta or Windows
emulation, exactly like the official launcher, because those versions only ship
x64 natives. Like minecraft-java-core, the launcher falls back to an Azul Zulu build
from <https://api.azul.com> when Mojang has no runtime for the platform or the
component (for example `jre-legacy` on Apple Silicon without
`intel_enabled_mac`), when the runtime manifest has no Java executable, or when
`java.version` forces a major version. The first package returned for the
platform, architecture and `java.kind` (`jre` by default, `jdk` accepted) is
downloaded into `runtime/jre-<major>/`, extracted with its symlinks and
permissions, and the `bin/java` executable is located inside the package, macOS
bundles included. Zip archives are requested first and `tar.gz` is used when no
zip exists, which covers Linux ARM builds. `java.path` bypasses everything.

`LaunchOptions` mirrors the Node options one for one:

| Option                 | Role                                                            | Default        |
|------------------------|-----------------------------------------------------------------|----------------|
| `root`                 | Minecraft root directory (`path`)                               | required       |
| `version`              | Version id, `latest_release` or `latest_snapshot`               | required       |
| `url`                  | Backend URL returning custom files (`path`, `hash`, `size`, `url`) | none        |
| `instance`             | Instance name, files go to `instances/<name>`                   | none           |
| `loader.kind`          | `Forge`, `NeoForge`, `Fabric`, `LegacyFabric`, `Quilt`          | none           |
| `loader.build`         | `latest`, `recommended` or an exact build                       | `latest`       |
| `loader.path`          | Loader directory relative to `root`                             | `./loader`     |
| `loader.enable`        | Install and use the loader                                      | `false`        |
| `mcp`                  | Jar replacing the client jar on the classpath                   | none           |
| `verify`               | Remove unexpected files                                         | `false`        |
| `ignored`              | Paths skipped by the check and by `verify`                      | empty          |
| `jvm_args`, `game_args`| Extra arguments appended last                                   | empty          |
| `java.path`            | Use this Java instead of the Mojang runtime                     | none           |
| `java.version`         | Force a major version, fetched from Azul (`"17"`)               | none           |
| `java.kind`            | Azul package type: `jre`, `jdk`, ...                            | `jre`          |
| `memory.min`, `memory.max` | `-Xms` and `-Xmx`                                           | `1G`, `2G`     |
| `screen.width`, `screen.height` | `--width` and `--height`                               | none           |
| `download_concurrency` | Parallel downloads, clamped to 1..30                            | `5`            |
| `intel_enabled_mac`    | Use the x64 Java runtime on Apple Silicon                       | `false`        |
| `bypass_offline`, `ignore_log4j`, `detached`, `timeout` | As in minecraft-java-core     | `false`, 10 s  |

Events (`Progress`, `Check`, `Speed`, `Estimated`, `Extract`, `Patch`, `Error`)
are delivered to the handler passed to `with_events`.

The lower-level pieces stay public: `Resolver`, `checker`, `Downloader`,
`loader::install`, `launcher::plan` and `launcher::spawn_with`.

Files are laid out like the official launcher: `versions/`, `libraries/`,
`assets/`, `runtime/<component>/`, plus `loader/` for mod loaders.

## Server status

`network::Status` pings a server directly over TCP, without any third-party
service, and returns the same fields as the Node `Status` class (`error`, `ms`,
`version`, `playersConnect`, `playersMax`) plus `protocol`, `description`,
`favicon`, the player sample and a `legacy` flag.

```rust
use crust_core::network::Status;

let status = Status::new("mc.hypixel.net", 25565).get_status().await?;
println!("{} {}/{} in {} ms", status.version, status.players_connect, status.players_max, status.ms);
```

The modern Server List Ping (1.7 and later) is tried first, then the legacy
probes in order: the 1.6 `MC|PingHost` packet, the 1.4 to 1.5 `FE 01` packet
and the Beta 1.8 to 1.3 `FE` packet. Text component descriptions are flattened
to plain text. The timeout defaults to 3 seconds and can be changed with
`with_timeout`. Verified against official 1.2.5, 1.4.7, 1.5.2, 1.6.4 and 1.7.10
servers and against public modern servers.

## Architecture

| Module          | Role                                                              |
|-----------------|-------------------------------------------------------------------|
| `foundation`    | Pure types and utilities (OS/arch, rules, Maven, SHA-1, JWT, PKCE) |
| `network`       | HTTP client, parallel downloader with progress, server status ping |
| `providers`     | One typed client per external API: Microsoft, Xbox, Mojang services, Yggdrasil, AZauth, launcher meta, Forge, NeoForge, Fabric, Azul |
| `authenticator` | Microsoft → Xbox → Minecraft sign-in, refresh, offline, Yggdrasil and AZauth accounts, account model |
| `resolver`      | Builds the full list of expected files for a version              |
| `checker`       | Checks what already exists on disk                                |
| `loader`        | Forge, NeoForge, Fabric, LegacyFabric and Quilt installation      |
| `launcher`      | `Launch` orchestrator, natives, arguments, process spawning       |

## Roadmap

- Game output as events with secret redaction (`data`, `close`)
- Skins and capes of Microsoft accounts embedded as base64
- `mjc-zip` resource pack compressor

## License

MIT. See [LICENSE](LICENSE).

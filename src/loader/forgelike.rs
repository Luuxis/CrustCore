use std::path::{Path, PathBuf};

use crate::foundation::events::{Event, EventHandler};
use crate::foundation::hash;
use crate::foundation::maven::library_path;
use crate::foundation::options::LoaderKind;
use crate::foundation::os::Platform;
use crate::foundation::rules;
use crate::network::{DownloadItem, Downloader, HttpClient};
use crate::providers::forge::ForgeMeta;
use crate::providers::launchermeta::Library;
use crate::providers::neoforge::{self, NeoForgeMeta};

use super::patcher::Patcher;
use super::{
    Error, InstallConfig, InstallSection, LoaderJson, archive, emit, mirrors, write_bytes,
    write_version_files,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Forge,
    NeoForge,
}

#[derive(Debug, Clone)]
struct Installer {
    file_path: PathBuf,
    old_api: bool,
}

#[derive(Debug, Clone)]
struct Profile {
    install: InstallSection,
    version: serde_json::Value,
}

pub async fn install(
    http: &HttpClient,
    config: &InstallConfig,
    flavor: Flavor,
    events: &EventHandler,
) -> Result<LoaderJson, Error> {
    let installer = match flavor {
        Flavor::Forge => download_forge_installer(http, config, events).await?,
        Flavor::NeoForge => download_neoforge_installer(http, config, events).await?,
    };
    let profile = extract_profile(&installer.file_path, flavor)?;
    if flavor == Flavor::NeoForge
        && profile.install.libraries.is_empty()
        && profile.install.processors.is_empty()
        && profile.install.path.is_none()
        && profile.install.file_path.is_none()
    {
        return Err(Error::Invalid("Invalid neoForge profile".into()));
    }

    let mut json = LoaderJson::from_value(profile.version.clone(), config.loader_dir.clone())?;
    super::patch_libraries(&mut json, Platform::current());
    if let Some(id) = &json.id {
        write_version_files(
            &config.loader_dir,
            id,
            &profile.version,
            &config.minecraft_jar,
        )?;
    }

    let prefix = universal_prefix(flavor, installer.old_api);
    let skip_filter = extract_universal_jar(
        &profile.install,
        &installer.file_path,
        config,
        prefix,
        events,
    )?;
    download_libraries(http, config, &profile, &json, skip_filter, flavor, events).await?;
    patch(config, &profile.install, prefix, events).await?;
    Ok(json)
}

fn universal_prefix(flavor: Flavor, old_api: bool) -> &'static str {
    match flavor {
        Flavor::Forge => "net.minecraftforge:forge",
        Flavor::NeoForge if old_api => "net.neoforged:forge",
        Flavor::NeoForge => "net.neoforged:neoforge",
    }
}

async fn download_forge_installer(
    http: &HttpClient,
    config: &InstallConfig,
    events: &EventHandler,
) -> Result<Installer, Error> {
    let meta = ForgeMeta::new(http.clone());
    let metadata = meta.maven_metadata().await;
    let Some(builds) = metadata.get(&config.minecraft_version) else {
        return Err(Error::Unsupported(format!(
            "Forge {} not supported",
            config.minecraft_version
        )));
    };

    let build = match config.build.as_str() {
        "latest" => {
            let promotions = meta.promotions().await?;
            let key = format!("{}-latest", config.minecraft_version);
            promotions
                .promos
                .get(&key)
                .and_then(|promo| builds.iter().find(|b| b.contains(promo.as_str())))
                .cloned()
        }
        "recommended" => {
            let promotions = meta.promotions().await?;
            let promo = promotions
                .promos
                .get(&format!("{}-recommended", config.minecraft_version))
                .or_else(|| {
                    promotions
                        .promos
                        .get(&format!("{}-latest", config.minecraft_version))
                });
            promo
                .and_then(|promo| builds.iter().find(|b| b.contains(promo.as_str())))
                .cloned()
        }
        other => Some(other.to_owned()),
    };
    let chosen = build
        .as_ref()
        .and_then(|build| builds.iter().find(|b| *b == build))
        .cloned();
    let Some(chosen) = chosen else {
        return Err(Error::Unsupported(format!(
            "Build {} not found, Available builds: {}",
            build.unwrap_or_else(|| "undefined".to_owned()),
            builds.join(", ")
        )));
    };

    let build_meta = meta.build_meta(&chosen).await?;
    let candidates = [
        ("installer", ForgeMeta::installer_url(&chosen)),
        ("client", ForgeMeta::client_url(&chosen)),
        ("universal", ForgeMeta::universal_url(&chosen)),
    ];
    let mut selected = None;
    for (classifier, base_url) in candidates {
        if let Some(files) = build_meta.classifiers.get(classifier)
            && let Some((ext, md5)) = files.iter().next()
        {
            selected = Some((base_url, ext.clone(), md5.clone()));
            break;
        }
    }
    let Some((base_url, ext, expected_md5)) = selected else {
        return Err(Error::Invalid("Invalid forge installer".into()));
    };

    let url = format!("{base_url}.{ext}");
    let folder = config
        .loader_dir
        .join("libraries")
        .join("net")
        .join("minecraftforge")
        .join("installer");
    let file_name = url.rsplit('/').next().unwrap_or_default().to_owned();
    let installer_path = folder.join(&file_name);
    if !installer_path.exists() {
        download_single(http, config, &url, &installer_path, &file_name, events).await?;
    }
    let actual = hash::md5_file(&installer_path)
        .map_err(|source| super::io_error(&installer_path, source))?;
    if !actual.eq_ignore_ascii_case(&expected_md5) {
        let _ = std::fs::remove_file(&installer_path);
        return Err(Error::Invalid("Invalid hash".into()));
    }
    Ok(Installer {
        file_path: installer_path,
        old_api: true,
    })
}

async fn download_neoforge_installer(
    http: &HttpClient,
    config: &InstallConfig,
    events: &EventHandler,
) -> Result<Installer, Error> {
    let meta = NeoForgeMeta::new(http.clone());
    let legacy = meta.legacy_versions().await?;
    let modern = meta.versions().await?;
    let needle = format!("{}-", config.minecraft_version);
    let mut old_api = true;
    let mut versions: Vec<String> = legacy
        .versions
        .into_iter()
        .filter(|version| version.contains(&needle))
        .collect();
    if versions.is_empty() {
        versions = modern
            .versions
            .into_iter()
            .filter(|version| {
                neoforge::minecraft_version_of(version).as_deref()
                    == Some(config.minecraft_version.as_str())
            })
            .collect();
        old_api = false;
    }
    if versions.is_empty() {
        return Err(Error::Unsupported(format!(
            "NeoForge doesn't support Minecraft {}",
            config.minecraft_version
        )));
    }
    let build = match config.build.as_str() {
        "latest" | "recommended" => versions.last().cloned(),
        other => versions.iter().find(|v| *v == other).cloned(),
    };
    let Some(build) = build else {
        return Err(Error::Unsupported(format!(
            "NeoForge Loader {} not found, Available builds: {}",
            config.build,
            versions.join(", ")
        )));
    };
    let url = if old_api {
        NeoForgeMeta::legacy_installer_url(&build)
    } else {
        NeoForgeMeta::installer_url(&build)
    };
    let folder = config
        .loader_dir
        .join("libraries")
        .join("net")
        .join("neoforged")
        .join("installer");
    let file_name = format!("neoForge-{build}-installer.jar");
    let installer_path = folder.join(&file_name);
    if !installer_path.exists() {
        download_single(http, config, &url, &installer_path, &file_name, events).await?;
    }
    Ok(Installer {
        file_path: installer_path,
        old_api,
    })
}

async fn download_single(
    http: &HttpClient,
    config: &InstallConfig,
    url: &str,
    path: &Path,
    file_name: &str,
    events: &EventHandler,
) -> Result<(), Error> {
    let events = events.clone();
    let label = file_name.to_owned();
    Downloader::new(http.clone(), config.concurrency)
        .download_all(
            vec![DownloadItem {
                url: url.to_owned(),
                path: path.to_path_buf(),
                sha1: None,
                size: None,
                executable: false,
                label: label.clone(),
            }],
            std::sync::Arc::new(move |progress| {
                events(Event::Progress {
                    downloaded: progress.downloaded,
                    total: progress.total,
                    element: label.clone(),
                });
            }),
        )
        .await?;
    Ok(())
}

fn extract_profile(installer: &Path, flavor: Flavor) -> Result<Profile, Error> {
    let name = match flavor {
        Flavor::Forge => "forge",
        Flavor::NeoForge => "neoForge",
    };
    let Some(content) = archive::read_entry(installer, "install_profile.json")? else {
        return Err(Error::Invalid(format!("Invalid {name} installer")));
    };
    let origin: serde_json::Value = serde_json::from_slice(&content)?;
    if let Some(install) = origin.get("install") {
        let install: InstallSection = serde_json::from_value(install.clone())?;
        let version = origin
            .get("versionInfo")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(Profile { install, version });
    }
    let install: InstallSection = serde_json::from_value(origin)?;
    let json_name = install
        .json
        .as_deref()
        .unwrap_or("/version.json")
        .rsplit('/')
        .next()
        .unwrap_or("version.json")
        .to_owned();
    let Some(extra) = archive::read_entry(installer, &json_name)? else {
        return Err(Error::Invalid(match flavor {
            Flavor::Forge => "Invalid additional JSON in forge installer".to_owned(),
            Flavor::NeoForge => "Unable to read additional JSON from neoForge installer".to_owned(),
        }));
    };
    let version: serde_json::Value = serde_json::from_slice(&extra)?;
    Ok(Profile { install, version })
}

fn extract_universal_jar(
    install: &InstallSection,
    installer: &Path,
    config: &InstallConfig,
    universal_prefix: &str,
    events: &EventHandler,
) -> Result<bool, Error> {
    let libraries_dir = config.loader_dir.join("libraries");
    let mut skip_filter = true;

    if let Some(file_path) = &install.file_path {
        let info = library_path(install.path.as_deref().unwrap_or_default(), None, None);
        emit(
            events,
            Event::Extract(format!("Extracting {}...", info.name)),
        );
        if let Some(content) = archive::read_entry(installer, file_path)? {
            write_bytes(&libraries_dir.join(&info.path).join(&info.name), &content)?;
        }
    } else if let Some(path) = &install.path {
        let info = library_path(path, None, None);
        let entries = archive::list_entries_containing(installer, &format!("maven/{}", info.path))?;
        for entry in entries {
            let file_name = entry.rsplit('/').next().unwrap_or_default().to_owned();
            emit(events, Event::Extract(format!("Extracting {file_name}...")));
            let Some(content) = archive::read_entry(installer, &entry)? else {
                continue;
            };
            write_bytes(&libraries_dir.join(&info.path).join(&file_name), &content)?;
        }
    } else {
        skip_filter = false;
    }

    if !install.processors.is_empty() {
        let universal = install
            .libraries
            .iter()
            .find(|library| library.name.starts_with(universal_prefix))
            .map(|library| library.name.as_str());
        if let Some(client_data) = archive::read_entry(installer, "data/client.lzma")? {
            let coordinate = install.path.as_deref().or(universal).unwrap_or_default();
            let info = library_path(coordinate, Some("-clientdata"), Some(".lzma"));
            write_bytes(
                &libraries_dir.join(&info.path).join(&info.name),
                &client_data,
            )?;
            emit(
                events,
                Event::Extract(format!("Extracting {}...", info.name)),
            );
        }
    }
    Ok(skip_filter)
}

async fn download_libraries(
    http: &HttpClient,
    config: &InstallConfig,
    profile: &Profile,
    json: &LoaderJson,
    skip_filter: bool,
    flavor: Flavor,
    events: &EventHandler,
) -> Result<(), Error> {
    let mut libraries: Vec<Library> = json.libraries.clone();
    libraries.extend(profile.install.libraries.iter().cloned());
    let mut seen = std::collections::HashSet::new();
    libraries.retain(|library| seen.insert(library.name.clone()));

    let skip_prefixes: &[&str] = match flavor {
        Flavor::Forge => &[
            "net.minecraftforge:forge:",
            "net.minecraftforge:minecraftforge:",
        ],
        Flavor::NeoForge => &[
            "net.minecraftforge:neoforged:",
            "net.minecraftforge:minecraftforge:",
        ],
    };
    let platform = Platform::current();
    let total = libraries.len();
    let mut checked = 0usize;
    let mut queue = Vec::new();
    let mut total_size = 0u64;
    let libraries_dir = config.loader_dir.join("libraries");

    for library in &libraries {
        let artifact_url = library
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .map(|a| a.url.as_str())
            .filter(|url| !url.is_empty());
        if skip_filter
            && skip_prefixes
                .iter()
                .any(|prefix| library.name.contains(prefix))
            && artifact_url.is_none()
        {
            emit(events, check(checked, total));
            checked += 1;
            continue;
        }

        let skip = match flavor {
            Flavor::Forge => rules::skip_library_node(library.rules.as_deref(), platform),
            Flavor::NeoForge => library.rules.is_some(),
        };
        if skip {
            emit(events, check(checked, total));
            checked += 1;
            continue;
        }

        let natives_suffix = match flavor {
            Flavor::Forge => library
                .natives
                .as_ref()
                .and_then(|natives| natives.get(platform.mojang_name()))
                .cloned(),
            Flavor::NeoForge => None,
        };
        let suffix = natives_suffix.as_ref().map(|s| format!("-{s}"));
        let info = library_path(&library.name, suffix.as_deref(), None);
        let file_path = libraries_dir.join(&info.path).join(&info.name);

        if !file_path.exists() {
            let base = if natives_suffix.is_some() {
                format!("{}/", info.path)
            } else {
                format!("{}/{}", info.path, info.name)
            };
            let mut url = None;
            let mut size = 0u64;
            if let Some((mirror_url, mirror_size)) =
                mirrors::check_mirror(http, &base, config.timeout).await
            {
                url = Some(mirror_url);
                size = mirror_size;
                total_size += size;
            } else if let Some(artifact) =
                library.downloads.as_ref().and_then(|d| d.artifact.as_ref())
            {
                url = Some(artifact.url.clone());
                size = artifact.size;
                total_size += size;
            }
            let Some(url) = url.filter(|u| !u.is_empty()) else {
                match flavor {
                    Flavor::Forge => {
                        emit(events, check(checked, total));
                        checked += 1;
                        emit(
                            events,
                            Event::Error(format!("Library {} not found", info.name)),
                        );
                        continue;
                    }
                    Flavor::NeoForge => {
                        return Err(Error::Invalid(format!(
                            "Impossible to download {}",
                            info.name
                        )));
                    }
                }
            };
            queue.push(DownloadItem {
                url,
                path: file_path,
                sha1: None,
                size: Some(size),
                executable: false,
                label: "libraries".to_owned(),
            });
        }
        emit(events, check(checked, total));
        checked += 1;
    }

    if !queue.is_empty() {
        let events_for_progress = events.clone();
        let failures = Downloader::new(http.clone(), config.concurrency)
            .download_all_lenient(
                queue,
                std::sync::Arc::new(move |progress| {
                    events_for_progress(Event::Progress {
                        downloaded: progress.downloaded,
                        total: total_size.max(progress.total),
                        element: "libraries".to_owned(),
                    });
                }),
            )
            .await?;
        for failure in failures {
            emit(events, Event::Error(failure.to_string()));
        }
    }
    Ok(())
}

async fn patch(
    config: &InstallConfig,
    install: &InstallSection,
    universal_prefix: &str,
    events: &EventHandler,
) -> Result<(), Error> {
    if install.processors.is_empty() {
        return Ok(());
    }
    let patcher = Patcher::new(config, universal_prefix);
    if !patcher.check(install) {
        patcher.patch(install, events).await?;
    }
    Ok(())
}

fn check(checked: usize, total: usize) -> Event {
    Event::Check {
        checked,
        total,
        element: "libraries".to_owned(),
    }
}

#[allow(dead_code)]
fn kind_name(kind: LoaderKind) -> &'static str {
    kind.name()
}

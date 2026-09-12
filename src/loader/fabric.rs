use std::path::Path;

use crate::foundation::events::{Event, EventHandler};
use crate::foundation::maven::library_path;
use crate::foundation::options::LoaderKind;
use crate::network::{DownloadItem, Downloader, HttpClient};
use crate::providers::fabric::{Endpoints, FabricMeta};

use super::{Error, InstallConfig, LoaderJson, emit, mirrors, write_version_files};

pub async fn install(
    http: &HttpClient,
    config: &InstallConfig,
    events: &EventHandler,
) -> Result<LoaderJson, Error> {
    let endpoints = Endpoints::for_kind(config.kind)
        .ok_or_else(|| Error::Unsupported(format!("Loader {} not found", config.kind)))?;
    let meta = FabricMeta::new(http.clone(), endpoints);
    let raw = download_json(&meta, config).await?;
    let json = LoaderJson::from_value(raw, config.loader_dir.clone())?;

    if let Some(id) = &json.id {
        write_version_files(&config.loader_dir, id, &json.raw, &config.minecraft_jar)?;
    }
    download_libraries(http, config, &json, events).await?;
    Ok(json)
}

async fn download_json(
    meta: &FabricMeta,
    config: &InstallConfig,
) -> Result<serde_json::Value, Error> {
    let versions = meta.versions().await?;
    let label = match config.kind {
        LoaderKind::Quilt => "QuiltMC",
        _ => "FabricMC",
    };
    if !versions
        .game
        .iter()
        .any(|game| game.version == config.minecraft_version)
    {
        return Err(Error::Unsupported(format!(
            "{label} doesn't support Minecraft {}",
            config.minecraft_version
        )));
    }
    let available: Vec<&str> = versions
        .loader
        .iter()
        .map(|loader| loader.version.as_str())
        .collect();
    let selected = match (config.kind, config.build.as_str()) {
        (LoaderKind::Quilt, "latest") => versions.loader.first(),
        (LoaderKind::Quilt, "recommended") => versions
            .loader
            .iter()
            .find(|loader| !loader.version.contains("beta")),
        (_, "latest" | "recommended") => versions.loader.first(),
        (_, build) => versions
            .loader
            .iter()
            .find(|loader| loader.version == build),
    };
    let Some(selected) = selected else {
        let name = match config.kind {
            LoaderKind::Quilt => "QuiltMC Loader",
            _ => "Fabric Loader",
        };
        return Err(Error::Unsupported(format!(
            "{name} {} not found, Available builds: {}",
            config.build,
            available.join(", ")
        )));
    };
    Ok(meta
        .profile(&config.minecraft_version, &selected.version)
        .await?)
}

async fn download_libraries(
    http: &HttpClient,
    config: &InstallConfig,
    json: &LoaderJson,
    events: &EventHandler,
) -> Result<(), Error> {
    let total = json.libraries.len();
    let mut checked = 0usize;
    let mut queue = Vec::new();
    let mut total_size = 0u64;

    for library in &json.libraries {
        if library.rules.is_some() {
            emit(events, check(checked, total));
            checked += 1;
            continue;
        }
        let info = library_path(&library.name, None, None);
        let file_path = config
            .loader_dir
            .join("libraries")
            .join(&info.path)
            .join(&info.name);
        if !file_path.exists() {
            let url = format!(
                "{}{}/{}",
                library.url.as_deref().unwrap_or_default(),
                info.path,
                info.name
            );
            let mut size = 0;
            if let Some(found) = mirrors::check_url(http, &url, config.timeout).await {
                size = found;
                total_size += found;
            }
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

fn check(checked: usize, total: usize) -> Event {
    Event::Check {
        checked,
        total,
        element: "libraries".to_owned(),
    }
}

#[allow(dead_code)]
fn library_file(loader_dir: &Path, name: &str) -> std::path::PathBuf {
    let info = library_path(name, None, None);
    loader_dir.join("libraries").join(info.path).join(info.name)
}

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use super::Error;

pub async fn extract_natives(archives: &[PathBuf], target: &Path) -> Result<(), Error> {
    if archives.is_empty() {
        return Ok(());
    }
    let archives = archives.to_vec();
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&target).map_err(|source| io_error(&target, source))?;
        for archive in &archives {
            extract_one(archive, &target)?;
        }
        Ok(())
    })
    .await?
}

fn extract_one(archive_path: &Path, target: &Path) -> Result<(), Error> {
    let file = File::open(archive_path).map_err(|source| io_error(archive_path, source))?;
    let mut archive = ZipArchive::new(file).map_err(|source| Error::Zip {
        path: archive_path.display().to_string(),
        source,
    })?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|source| Error::Zip {
            path: archive_path.display().to_string(),
            source,
        })?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        if relative.starts_with("META-INF") {
            continue;
        }
        let destination = target.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&destination)
                .map_err(|source| io_error(&destination, source))?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        let mut output =
            File::create(&destination).map_err(|source| io_error(&destination, source))?;
        io::copy(&mut entry, &mut output).map_err(|source| io_error(&destination, source))?;
    }
    Ok(())
}

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

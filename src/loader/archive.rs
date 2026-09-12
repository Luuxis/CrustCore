use std::fs::File;
use std::io::Read;
use std::path::Path;

use zip::ZipArchive;

use super::Error;

pub fn read_entry(archive_path: &Path, name: &str) -> Result<Option<Vec<u8>>, Error> {
    let mut archive = open(archive_path)?;
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(source) => return Err(zip_error(archive_path, source)),
    };
    if entry.is_dir() {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(archive_path, source))?;
    Ok(Some(bytes))
}

pub fn list_entries_containing(archive_path: &Path, needle: &str) -> Result<Vec<String>, Error> {
    let mut archive = open(archive_path)?;
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|source| zip_error(archive_path, source))?;
        if !entry.is_dir() && entry.name().contains(needle) {
            names.push(entry.name().to_owned());
        }
    }
    Ok(names)
}

pub fn main_class(jar: &Path) -> Result<Option<String>, Error> {
    let Some(manifest) = read_entry(jar, "META-INF/MANIFEST.MF")? else {
        return Ok(None);
    };
    let content = String::from_utf8_lossy(&manifest);
    let Some(rest) = content.split("Main-Class: ").nth(1) else {
        return Ok(None);
    };
    let line = rest.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return Ok(None);
    }
    Ok(Some(line.to_owned()))
}

fn open(archive_path: &Path) -> Result<ZipArchive<File>, Error> {
    let file = File::open(archive_path).map_err(|source| io_error(archive_path, source))?;
    ZipArchive::new(file).map_err(|source| zip_error(archive_path, source))
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

fn zip_error(path: &Path, source: zip::result::ZipError) -> Error {
    Error::Zip {
        path: path.display().to_string(),
        source,
    }
}

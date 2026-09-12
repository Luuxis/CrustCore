use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;
use zip::ZipArchive;

use super::Error;
use crate::foundation::events::{Event, EventHandler};
use crate::foundation::os::{Arch, Platform};
use crate::network::{DownloadItem, Downloader, HttpClient};
use crate::providers::azul::{self, AzulMeta, Package, PackageQuery};
use crate::resolver::AzulRequest;

const ARCHIVE_TYPES: [&str; 2] = ["zip", "tar.gz"];

pub struct AzulInstall<'a> {
    pub http: &'a HttpClient,
    pub root: &'a Path,
    pub platform: Platform,
    pub arch: Arch,
    pub concurrency: usize,
}

pub async fn install_azul(
    install: &AzulInstall<'_>,
    request: &AzulRequest,
    events: &EventHandler,
) -> Result<PathBuf, Error> {
    let meta = AzulMeta::new(install.http.clone());
    let mut query = PackageQuery {
        java_version: request.major_version.clone(),
        os: azul::platform_name(install.platform).to_owned(),
        arch: azul::arch_name(install.arch).to_owned(),
        archive_type: ARCHIVE_TYPES[0].to_owned(),
        java_package_type: request.package_type.clone(),
    };
    let mut package: Option<Package> = None;
    for archive_type in ARCHIVE_TYPES {
        query.archive_type = archive_type.to_owned();
        if let Some(found) = meta.packages(&query).await?.into_iter().next() {
            package = Some(found);
            break;
        }
    }
    let Some(package) = package else {
        return Err(Error::JavaUnavailable(
            "No Java versions found for the specified parameters.".to_owned(),
        ));
    };

    let folder = install
        .root
        .join("runtime")
        .join(format!("jre-{}", request.major_version));
    let archive_path = folder.join(&package.name);
    let extracted = folder.join(package_directory(&package.name));

    if let Some(java) = locate_java(&extracted, install.platform) {
        return Ok(java);
    }

    if !archive_path.exists() {
        let events = events.clone();
        let label = package.name.clone();
        Downloader::new(install.http.clone(), install.concurrency)
            .download_all(
                vec![DownloadItem {
                    url: package.download_url.clone(),
                    path: archive_path.clone(),
                    sha1: None,
                    size: None,
                    executable: false,
                    label: label.clone(),
                }],
                Arc::new(move |progress| {
                    events(Event::Progress {
                        downloaded: progress.downloaded,
                        total: progress.total,
                        element: label.clone(),
                    });
                }),
            )
            .await?;
    }

    events(Event::Extract(format!("Extracting {}...", package.name)));
    let archive_for_task = archive_path.clone();
    let folder_for_task = folder.clone();
    tokio::task::spawn_blocking(move || extract_archive(&archive_for_task, &folder_for_task))
        .await??;

    locate_java(&extracted, install.platform).ok_or_else(|| {
        Error::JavaUnavailable(format!(
            "java executable not found in {}",
            extracted.display()
        ))
    })
}

pub fn package_directory(name: &str) -> String {
    for suffix in [".tar.gz", ".zip"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    name.to_owned()
}

pub fn locate_java(directory: &Path, platform: Platform) -> Option<PathBuf> {
    let executable = match platform {
        Platform::Windows => "java.exe",
        _ => "java",
    };
    let direct = directory.join("bin").join(executable);
    if direct.is_file() {
        return Some(direct);
    }
    let bundle = directory
        .join("Contents")
        .join("Home")
        .join("bin")
        .join(executable);
    if bundle.is_file() {
        return Some(bundle);
    }
    let link_file = directory.join("bin");
    if link_file.is_file()
        && let Ok(target) = std::fs::read_to_string(&link_file)
    {
        let candidate = directory.join(target.trim()).join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    walk(directory, 0, &mut |path| {
        if path.file_name().and_then(|n| n.to_str()) == Some(executable)
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("bin")
            && path.is_file()
        {
            found.push(path.to_path_buf());
        }
    });
    found.sort_by_key(|path| path.components().count());
    found.into_iter().next()
}

fn walk(directory: &Path, depth: usize, visit: &mut dyn FnMut(&Path)) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, visit);
        } else {
            visit(&path);
        }
    }
}

fn extract_archive(archive: &Path, target: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(target).map_err(|source| io_error(target, source))?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, target)
    } else {
        extract_zip(archive, target)
    }
}

fn extract_tar_gz(archive: &Path, target: &Path) -> Result<(), Error> {
    let file = File::open(archive).map_err(|source| io_error(archive, source))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);
    tar.unpack(target)
        .map_err(|source| io_error(archive, source))
}

fn extract_zip(archive_path: &Path, target: &Path) -> Result<(), Error> {
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
        if entry.is_symlink() {
            let mut link = String::new();
            entry
                .read_to_string(&mut link)
                .map_err(|source| io_error(&destination, source))?;
            write_symlink(link.trim(), &destination)?;
            continue;
        }
        let mut output =
            File::create(&destination).map_err(|source| io_error(&destination, source))?;
        io::copy(&mut entry, &mut output).map_err(|source| io_error(&destination, source))?;
        drop(output);
        set_mode(&destination, entry.unix_mode())?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_symlink(link: &str, destination: &Path) -> Result<(), Error> {
    let _ = std::fs::remove_file(destination);
    std::os::unix::fs::symlink(link, destination).map_err(|source| io_error(destination, source))
}

#[cfg(not(unix))]
fn write_symlink(link: &str, destination: &Path) -> Result<(), Error> {
    std::fs::write(destination, link).map_err(|source| io_error(destination, source))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let mode = mode
        .map(|m| m & 0o7777)
        .filter(|m| *m != 0)
        .unwrap_or(0o755);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|source| io_error(path, source))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), Error> {
    Ok(())
}

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crustcore-java-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn strips_archive_extensions() {
        assert_eq!(
            package_directory("zulu17.68.203-ca-jre17.0.20.1-macosx_aarch64.zip"),
            "zulu17.68.203-ca-jre17.0.20.1-macosx_aarch64"
        );
        assert_eq!(
            package_directory("zulu21.52.203-ca-jre21.0.12.1-linux_aarch64.tar.gz"),
            "zulu21.52.203-ca-jre21.0.12.1-linux_aarch64"
        );
    }

    #[test]
    fn locates_plain_bin_layout() {
        let dir = temp("plain");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("java"), b"").unwrap();
        assert_eq!(
            locate_java(&dir, Platform::Linux),
            Some(dir.join("bin").join("java"))
        );
        assert_eq!(locate_java(&dir, Platform::Windows), None);
    }

    #[test]
    fn locates_macos_bundle_layout() {
        let dir = temp("bundle");
        let bin = dir.join("Contents").join("Home").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("java"), b"").unwrap();
        assert_eq!(locate_java(&dir, Platform::MacOs), Some(bin.join("java")));
    }

    #[test]
    fn locates_through_node_style_link_file() {
        let dir = temp("linkfile");
        let bin = dir
            .join("zulu-8.jre")
            .join("Contents")
            .join("Home")
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("java"), b"").unwrap();
        std::fs::write(dir.join("bin"), "zulu-8.jre/Contents/Home/bin").unwrap();
        assert_eq!(locate_java(&dir, Platform::MacOs), Some(bin.join("java")));
    }

    #[test]
    fn locates_nested_layout_by_walking() {
        let dir = temp("nested");
        let bin = dir.join("jdk").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("java"), b"").unwrap();
        assert_eq!(locate_java(&dir, Platform::Linux), Some(bin.join("java")));
        assert_eq!(locate_java(&temp("empty"), Platform::Linux), None);
    }

    #[test]
    fn extracts_zip_with_directories_files_and_symlinks() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = temp("zip");
        let archive_path = dir.join("runtime.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        let options = SimpleFileOptions::default().unix_permissions(0o755);
        writer.add_directory("jre/bin/", options).unwrap();
        writer.start_file("jre/bin/java", options).unwrap();
        writer.write_all(b"#!/bin/sh\n").unwrap();
        writer.start_file("META-INF/MANIFEST.MF", options).unwrap();
        writer.write_all(b"ignored").unwrap();
        #[cfg(unix)]
        writer
            .add_symlink("jre/bin/java-link", "java", options)
            .unwrap();
        writer.finish().unwrap();

        let target = dir.join("out");
        extract_archive(&archive_path, &target).unwrap();
        let java = target.join("jre").join("bin").join("java");
        assert!(java.is_file());
        assert!(!target.join("META-INF").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                std::fs::metadata(&java).unwrap().permissions().mode() & 0o111,
                0
            );
            let link = target.join("jre").join("bin").join("java-link");
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        assert_eq!(
            locate_java(&target.join("jre"), Platform::Linux),
            Some(java)
        );
    }
}

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::foundation::hash;
use crate::network::DownloadItem;

const HASH_CONCURRENCY: usize = 64;

#[derive(Debug, Clone)]
pub struct CheckProgress {
    pub checked: usize,
    pub total: usize,
}

pub type CheckHandler = Arc<dyn Fn(&CheckProgress) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub root: PathBuf,
    pub instance: Option<String>,
    pub ignored: Vec<String>,
}

impl CheckOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            instance: None,
            ignored: Vec::new(),
        }
    }

    fn base(&self) -> PathBuf {
        match &self.instance {
            Some(instance) => self.root.join("instances").join(instance),
            None => self.root.clone(),
        }
    }

    fn ignored_set(&self) -> HashSet<String> {
        self.ignored
            .iter()
            .map(|p| normalize(p).trim_start_matches('/').to_owned())
            .collect()
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn relative_to(path: &Path, base: &Path) -> String {
    let path = normalize(&path.display().to_string());
    let base = format!(
        "{}/",
        normalize(&base.display().to_string()).trim_end_matches('/')
    );
    path.strip_prefix(&base).unwrap_or(&path).to_owned()
}

pub async fn find_missing(
    items: Vec<DownloadItem>,
    options: &CheckOptions,
    on_progress: CheckHandler,
) -> Result<Vec<DownloadItem>, tokio::task::JoinError> {
    let mut missing = Vec::new();
    let mut to_hash = Vec::new();
    let ignored = options.ignored_set();
    let base = options.base();

    for item in items {
        let Ok(metadata) = std::fs::metadata(&item.path) else {
            missing.push(item);
            continue;
        };
        if ignored.contains(&relative_to(&item.path, &base)) {
            continue;
        }
        if item.sha1.is_some() {
            match item.size {
                Some(size) if size != 0 && metadata.len() != size => missing.push(item),
                _ => to_hash.push(item),
            }
        }
    }

    if to_hash.is_empty() {
        return Ok(missing);
    }

    let total = to_hash.len();
    let checked = Arc::new(AtomicUsize::new(0));
    let semaphore = Arc::new(Semaphore::new(HASH_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for item in to_hash {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed");
        let checked = checked.clone();
        let on_progress = on_progress.clone();
        tasks.spawn_blocking(move || {
            let _permit = permit;
            let expected = item.sha1.as_deref().unwrap_or_default();
            let matches = hash::sha1_file(&item.path)
                .map(|actual| actual.eq_ignore_ascii_case(expected))
                .unwrap_or(false);
            let done = checked.fetch_add(1, Ordering::Relaxed) + 1;
            on_progress(&CheckProgress {
                checked: done,
                total,
            });
            (!matches).then_some(item)
        });
    }
    while let Some(result) = tasks.join_next().await {
        if let Some(item) = result? {
            missing.push(item);
        }
    }
    Ok(missing)
}

pub fn clean_unexpected(options: &CheckOptions, keep: &[PathBuf]) -> std::io::Result<usize> {
    let root = &options.root;
    if options.instance.is_some() {
        std::fs::create_dir_all(root.join("instances"))?;
    }
    let base = options.base();
    let all_files = collect_files(&base);

    let mut ignored: Vec<PathBuf> = Vec::new();
    ignored.extend(collect_files(&root.join("loader")));
    ignored.extend(collect_files(&root.join("runtime")));
    for entry in &options.ignored {
        let candidate = base.join(normalize(entry).trim_start_matches('/'));
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_dir() => ignored.extend(collect_files(&candidate)),
            Ok(_) => ignored.push(candidate),
            Err(_) => {}
        }
    }
    ignored.extend(keep.iter().cloned());
    let ignored_set: HashSet<String> = ignored
        .iter()
        .map(|p| normalize(&p.display().to_string()))
        .collect();

    let mut deleted = 0;
    for file in all_files {
        if ignored_set.contains(&normalize(&file.display().to_string())) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        if metadata.is_dir() {
            if std::fs::remove_dir_all(&file).is_ok() {
                deleted += 1;
            }
            continue;
        }
        if std::fs::remove_file(&file).is_err() {
            continue;
        }
        deleted += 1;
        let mut current = file.parent().map(Path::to_path_buf);
        while let Some(dir) = current {
            if dir == base || dir == *root {
                break;
            }
            let empty = std::fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(&dir);
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    }
    Ok(deleted)
}

fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    walk(dir, &mut collected);
    collected
}

fn walk(dir: &Path, collected: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    if entries.is_empty() {
        collected.push(dir.to_path_buf());
        return;
    }
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, collected);
        } else {
            collected.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: PathBuf, sha1: Option<&str>, size: Option<u64>) -> DownloadItem {
        DownloadItem {
            url: String::new(),
            path,
            sha1: sha1.map(str::to_owned),
            size,
            executable: false,
            label: "test".to_owned(),
        }
    }

    #[tokio::test]
    async fn classifies_files() {
        let dir = std::env::temp_dir().join("crustcore-checker-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.txt");
        std::fs::write(&good, b"hello world").unwrap();
        let bad = dir.join("bad.txt");
        std::fs::write(&bad, b"hello world").unwrap();
        let wrong_size = dir.join("size.txt");
        std::fs::write(&wrong_size, b"hello").unwrap();
        let ignored = dir.join("ignored.txt");
        std::fs::write(&ignored, b"hello").unwrap();

        let items = vec![
            item(
                good.clone(),
                Some("2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"),
                Some(11),
            ),
            item(
                bad.clone(),
                Some("0000000000000000000000000000000000000000"),
                Some(11),
            ),
            item(wrong_size.clone(), Some("x"), Some(11)),
            item(ignored.clone(), Some("x"), Some(11)),
            item(dir.join("missing.txt"), None, None),
        ];
        let mut options = CheckOptions::new(&dir);
        options.ignored = vec!["ignored.txt".into()];
        let missing = find_missing(items, &options, Arc::new(|_| {}))
            .await
            .unwrap();
        let paths: Vec<_> = missing.iter().map(|i| i.path.clone()).collect();
        assert_eq!(paths.len(), 3);
        assert!(!paths.contains(&good));
        assert!(!paths.contains(&ignored));
        assert!(paths.contains(&bad));
        assert!(paths.contains(&wrong_size));
    }

    #[test]
    fn cleans_unexpected_files_but_keeps_ignored_and_bundle() {
        let root = std::env::temp_dir().join("crustcore-clean-test");
        let _ = std::fs::remove_dir_all(&root);
        let instance = root.join("instances").join("hypixel");
        std::fs::create_dir_all(instance.join("mods")).unwrap();
        std::fs::create_dir_all(instance.join("saves").join("world")).unwrap();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let keep = instance.join("mods").join("keep.jar");
        std::fs::write(&keep, b"k").unwrap();
        std::fs::write(instance.join("mods").join("stale.jar"), b"s").unwrap();
        std::fs::write(instance.join("saves").join("world").join("level.dat"), b"w").unwrap();
        std::fs::write(root.join("runtime").join("java"), b"j").unwrap();

        let mut options = CheckOptions::new(&root);
        options.instance = Some("hypixel".into());
        options.ignored = vec!["saves".into()];
        clean_unexpected(&options, std::slice::from_ref(&keep)).unwrap();

        assert!(keep.exists());
        assert!(!instance.join("mods").join("stale.jar").exists());
        assert!(
            instance
                .join("saves")
                .join("world")
                .join("level.dat")
                .exists()
        );
        assert!(root.join("runtime").join("java").exists());
    }
}

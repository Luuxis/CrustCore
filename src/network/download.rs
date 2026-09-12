use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::{Error, HttpClient};

const RETRY_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadItem {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub executable: bool,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub label: String,
}

pub type ProgressHandler = Arc<dyn Fn(&Progress) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct Downloader {
    http: HttpClient,
    concurrency: usize,
}

struct Counters {
    downloaded: AtomicU64,
    total: AtomicU64,
    files_done: AtomicUsize,
    files_total: usize,
}

trait Sink: Sync {
    fn bytes(&self, count: u64);
    fn size(&self, total: u64);
}

struct AttemptSink<'a> {
    counters: &'a Counters,
    item: &'a DownloadItem,
    on_progress: &'a ProgressHandler,
    announced: AtomicU64,
}

impl Sink for AttemptSink<'_> {
    fn bytes(&self, count: u64) {
        self.counters.downloaded.fetch_add(count, Ordering::Relaxed);
        (self.on_progress)(&self.counters.snapshot(&self.item.label));
    }

    fn size(&self, total: u64) {
        if self.item.size.is_none() {
            self.counters.total.fetch_add(total, Ordering::Relaxed);
            self.announced.fetch_add(total, Ordering::Relaxed);
        }
    }
}

impl Downloader {
    pub fn new(http: HttpClient, concurrency: usize) -> Self {
        Self {
            http,
            concurrency: concurrency.clamp(1, 64),
        }
    }

    pub async fn download_all(
        &self,
        items: Vec<DownloadItem>,
        on_progress: ProgressHandler,
    ) -> Result<(), Error> {
        let failures = self.run(items, on_progress).await?;
        match failures.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub async fn download_all_lenient(
        &self,
        items: Vec<DownloadItem>,
        on_progress: ProgressHandler,
    ) -> Result<Vec<Error>, Error> {
        self.run(items, on_progress).await
    }

    async fn run(
        &self,
        items: Vec<DownloadItem>,
        on_progress: ProgressHandler,
    ) -> Result<Vec<Error>, Error> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let counters = Arc::new(Counters {
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(items.iter().filter_map(|i| i.size).sum()),
            files_done: AtomicUsize::new(0),
            files_total: items.len(),
        });
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let mut tasks = JoinSet::new();

        for item in items {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore closed");
            let http = self.http.clone();
            let counters = counters.clone();
            let on_progress = on_progress.clone();
            tasks.spawn(async move {
                let _permit = permit;
                let result = download_with_retry(&http, &item, &counters, &on_progress).await;
                counters.files_done.fetch_add(1, Ordering::Relaxed);
                on_progress(&counters.snapshot(&item.label));
                result
            });
        }

        let mut failures = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error),
                Err(error) => return Err(Error::Join(error)),
            }
        }
        Ok(failures)
    }

    pub async fn download_file(&self, item: &DownloadItem) -> Result<u64, Error> {
        download_once(&self.http, item, None).await
    }
}

impl Counters {
    fn snapshot(&self, label: &str) -> Progress {
        Progress {
            downloaded: self.downloaded.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            label: label.to_owned(),
        }
    }
}

async fn download_with_retry(
    http: &HttpClient,
    item: &DownloadItem,
    counters: &Arc<Counters>,
    on_progress: &ProgressHandler,
) -> Result<(), Error> {
    let mut last_error = None;
    for attempt in 1..=RETRY_ATTEMPTS {
        let before = counters.downloaded.load(Ordering::Relaxed);
        let sink = AttemptSink {
            counters,
            item,
            on_progress,
            announced: AtomicU64::new(0),
        };
        match download_once(http, item, Some(&sink)).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                let now = counters.downloaded.load(Ordering::Relaxed);
                counters
                    .downloaded
                    .fetch_sub(now.saturating_sub(before), Ordering::Relaxed);
                counters
                    .total
                    .fetch_sub(sink.announced.load(Ordering::Relaxed), Ordering::Relaxed);
                last_error = Some(error);
                if attempt < RETRY_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64))
                        .await;
                }
            }
        }
    }
    Err(Error::Download {
        url: item.url.clone(),
        attempts: RETRY_ATTEMPTS,
        source: Box::new(last_error.expect("at least one attempt")),
    })
}

async fn download_once(
    http: &HttpClient,
    item: &DownloadItem,
    sink: Option<&dyn Sink>,
) -> Result<u64, Error> {
    if let Some(parent) = item.path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent, source))?;
    }
    let mut response = http.inner().get(&item.url).send().await?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Error::Status {
            status,
            url: item.url.clone(),
        });
    }
    if let Some(sink) = sink
        && let Some(length) = response.content_length()
    {
        sink.size(length);
    }
    let mut file = tokio::fs::File::create(&item.path)
        .await
        .map_err(|source| io_error(&item.path, source))?;
    let mut written = 0u64;
    let result: Result<(), Error> = async {
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk)
                .await
                .map_err(|source| io_error(&item.path, source))?;
            written += chunk.len() as u64;
            if let Some(sink) = sink {
                sink.bytes(chunk.len() as u64);
            }
        }
        file.flush()
            .await
            .map_err(|source| io_error(&item.path, source))?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&item.path).await;
        return Err(error);
    }
    if item.executable {
        set_executable(&item.path).await?;
    }
    Ok(written)
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|source| io_error(path, source))?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    tokio::fs::set_permissions(path, permissions)
        .await
        .map_err(|source| io_error(path, source))
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<(), Error> {
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

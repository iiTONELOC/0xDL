use crate::progress_bar::default_with_progress;
use crate::validator::{is_valid_file_path, is_valid_sha256, is_valid_url, verify_file_sha256};
use reqwest::Client;
use std::{
    fmt,
    fs::{self, File},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tempfile::NamedTempFile;
use tokio::{
    fs as tokio_fs,
    io::AsyncWriteExt,
    time::{Duration, MissedTickBehavior, interval},
};

pub type _0xdlDownloadResult = Result<(), DownloadError>;
pub type _0xdlUpdateFnType = Box<dyn Fn(f32) + Send + Sync + 'static>;
pub type _0xdlErrorType = Box<dyn std::error::Error + Send + Sync>;

const _TEST_URLS: [&str; 2] = [
    "https://raw.githubusercontent.com/iiTONELOC/0xdl/refs/heads/main/LICENSE.md",
    "https://raw.githubusercontent.com/iiTONELOC/0xdl/refs/heads/main/README.md",
];

#[derive(Copy, Clone, Debug)]
pub enum DownloadError {
    InvalidUrl,
    InvalidPath,
    InvalidSha256,
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::InvalidUrl => write!(f, "invalid URL"),
            DownloadError::InvalidPath => write!(f, "invalid file path"),
            DownloadError::InvalidSha256 => write!(f, "invalid SHA-256 checksum"),
        }
    }
}

impl std::error::Error for DownloadError {}

pub struct Downloader {
    pub url: String,
    pub path: String,
    pub sha256: Option<String>,
    pub current_progress: Arc<AtomicU64>,
    pub on_update: Option<_0xdlUpdateFnType>,
}

pub async fn get_file_size(url: &str) -> Result<u64, _0xdlErrorType> {
    let resp = Client::new().head(url).send().await?;
    Ok(resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0))
}

pub async fn download_file(
    url: &str,
    progress: Arc<AtomicU64>,
    sha256: Option<String>,
    on_update: Option<_0xdlUpdateFnType>,
    final_path: Option<&str>,
) -> Result<String, _0xdlErrorType> {
    let size = get_file_size(url).await?;

    let temp_path = {
        let temp = NamedTempFile::new()?;
        let (_, buf) = temp.keep()?;
        buf.to_string_lossy().to_string()
    };

    let mut resp = Client::new().get(url).send().await?;
    let mut file = tokio_fs::File::create(&temp_path).await?;

    // spawn updater and keep handle
    let updater = on_update.map(|cb| {
        let p = progress.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_millis(16));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let val = p.load(Ordering::Relaxed);
                if size > 0 {
                    cb((val as f32 / size as f32) * 100.0);
                }

                if val >= size {
                    break;
                }
            }
            cb(100.0);
            println!();
        })
    });

    // stream download
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush().await?;

    if let Some(h) = updater {
        let _ = h.await;
    }

    // verify after progress completes (printing now ordered)
    if let Some(expected) = sha256 {
        println!("Verifying SHA256 hash...");
        let ok = verify_file_sha256(&temp_path, &expected).await?;
        if !ok {
            let _ = tokio_fs::remove_file(&temp_path).await;
            if let Some(fp) = final_path {
                let _ = tokio_fs::remove_file(fp).await;
            }
            return Err("SHA256 hash mismatch".into());
        }
    }

    Ok(temp_path)
}

fn finalize_temp_file(temp: &str, final_path: &str) -> Result<(), _0xdlErrorType> {
    let final_path = std::path::Path::new(final_path);

    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut target = final_path.to_path_buf();
    if fs::metadata(&target).is_ok() {
        let mut count = 1;
        loop {
            let candidate = target.with_extension(format!(
                "{}.{}",
                target.extension().and_then(|e| e.to_str()).unwrap_or(""),
                count
            ));
            if fs::metadata(&candidate).is_err() {
                target = candidate;
                break;
            }
            count += 1;
        }
    }

    fs::copy(temp, &target)?;
    if let Ok(f) = File::open(&target) {
        let _ = f.sync_all();
    }
    let _ = fs::remove_file(temp);
    Ok(())
}

impl Downloader {
    pub fn new(url: &str, path: &str) -> Result<Self, DownloadError> {
        if !is_valid_url(url) {
            return Err(DownloadError::InvalidUrl);
        }
        if !is_valid_file_path(path) {
            return Err(DownloadError::InvalidPath);
        }

        Ok(Self {
            url: url.to_string(),
            path: path.to_string(),
            sha256: None,
            current_progress: Arc::new(AtomicU64::new(0)),
            on_update: None,
        })
    }

    pub fn enable_default_updater(mut self) -> Result<Self, DownloadError> {
        self.on_update = Some(Box::new(default_with_progress));
        Ok(self)
    }

    pub fn on_update<F>(mut self, f: F) -> Self
    where
        F: Fn(f32) + Send + Sync + 'static,
    {
        self.on_update = Some(Box::new(f));
        self
    }

    pub fn with_sha256(mut self, sha: &str) -> Result<Self, DownloadError> {
        if !is_valid_sha256(sha) {
            return Err(DownloadError::InvalidSha256);
        }
        self.sha256 = Some(sha.to_string());
        Ok(self)
    }

    pub async fn execute(self) -> Result<(), _0xdlErrorType> {
        let temp = download_file(
            &self.url,
            self.current_progress.clone(),
            self.sha256,
            self.on_update,
            Some(&self.path),
        )
        .await?;

        finalize_temp_file(&temp, &self.path)?;
        Ok(())
    }
}

pub async fn download(url: &str, path: &str, hash: Option<&str>) -> Result<(), DownloadError> {
    let mut builder = Downloader::new(url, path)?;

    if let Some(h) = hash {
        builder = builder.with_sha256(h)?;
    }

    builder
        .execute()
        .await
        .map_err(|_| DownloadError::InvalidPath)
}

pub async fn download_with_updates(
    url: &str,
    path: &str,
    updater_fn: Option<Box<dyn Fn(f32) + Send + Sync>>,
    hash: Option<&str>,
) -> Result<(), DownloadError> {
    let mut builder = Downloader::new(url, path)?.enable_default_updater()?;

    if let Some(f) = updater_fn {
        builder = builder.on_update(f);
    }

    if let Some(h) = hash {
        builder = builder.with_sha256(h)?;
    }

    builder
        .execute()
        .await
        .map_err(|_| DownloadError::InvalidPath)
}

// ---------------------
// TESTS (FULL FILE WITH FIXES)
// ---------------------

#[cfg(all(test, feature = "tests"))]
mod tests {
    use super::*;

    fn url() -> &'static str {
        "http://example.com/file"
    }
    fn path() -> &'static str {
        "downloaded_file.bin"
    }

    #[test]
    fn test_segmented_downloader_new_valid() {
        assert!(Downloader::new(url(), path()).is_ok());
    }

    #[test]
    fn test_segmented_downloader_new_invalid_url() {
        assert!(matches!(
            Downloader::new("invalid_url", path()),
            Err(DownloadError::InvalidUrl)
        ));
    }

    #[test]
    fn test_segmented_downloader_new_invalid_path() {
        assert!(matches!(
            Downloader::new(url(), "bad\0name.txt"),
            Err(DownloadError::InvalidPath)
        ));

        if cfg!(windows) {
            assert!(matches!(
                Downloader::new(url(), "invalid|path.txt"),
                Err(DownloadError::InvalidPath)
            ));
        }
    }

    #[test]
    fn test_with_sha256_valid() {
        assert!(
            Downloader::new(url(), path())
                .unwrap()
                .with_sha256("a3c1e2f4b5d6c7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2")
                .is_ok()
        );
    }

    #[test]
    fn test_with_sha256_invalid() {
        assert!(matches!(
            Downloader::new(url(), path())
                .unwrap()
                .with_sha256("invalid_sha256"),
            Err(DownloadError::InvalidSha256)
        ));
    }
}

#[cfg(all(test, feature = "net-tests"))]
mod net_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    // URL[0] — download_file variants
    #[tokio::test]
    async fn test_download_file_variants_url0() {
        let url = _TEST_URLS[0];

        // ---- no hash ----
        let path1 = "downloaded_test_file.bin";
        let downloader = Downloader::new(url, path1).unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            None,
            None,
            Some(path1),
        )
        .await
        .unwrap();

        assert!(Path::new(&result).exists());
        fs::remove_file(&result).unwrap();

        // ---- correct hash ----
        let path2 = "downloaded_test_file1.bin";
        let expected = "4f70c8056282511057ff5d99a5df660d06bcccac4dbb0383ce61477d5ab32581";

        let downloader = Downloader::new(url, path2).unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            Some(expected.to_string()),
            None,
            Some(path2),
        )
        .await
        .unwrap();

        assert!(Path::new(&result).exists());
        fs::remove_file(&result).unwrap();

        // ---- incorrect hash ----
        let path3 = "downloaded_test_file2.bin";
        let bad = "4f70c8056282511057ff5d99a5df660d06bcccac4dbb0383ce61477d5ab32582";

        let downloader = Downloader::new(url, path3).unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            Some(bad.to_string()),
            None,
            Some(path3),
        )
        .await;

        assert!(result.is_err());
        assert!(!Path::new(path3).exists());
    }

    // URL[1] — download_with_updates variants
    #[tokio::test]
    async fn test_download_with_updates_variants_url1() {
        let url = _TEST_URLS[1];

        // ---- no hash ----
        let path1 = "test_download.bin";
        let result = download_with_updates(url, path1, None, None).await;
        assert!(result.is_ok());
        assert!(Path::new(path1).exists());
        fs::remove_file(path1).unwrap();

        // ---- correct hash ----
        let path2 = "test_download_with_hash.bin";
        let sha256 = "7fcd8e3e464ff3874cc94327da3b6c269e723f9ec8952ffe6f85ce42c627bd45";

        let result = download_with_updates(url, path2, None, Some(sha256)).await;
        assert!(result.is_ok());
        assert!(Path::new(path2).exists());
        fs::remove_file(path2).unwrap();

        // ---- incorrect hash ----
        let path3 = "test_download_incorrect_hash.bin";
        let bad_sha = "e3aa82fb36dc042fb00b5e7fbe5f3e79d1dc5547ff94cdec1782aa43d9cde8e4";

        let result = download_with_updates(url, path3, None, Some(bad_sha)).await;
        assert!(result.is_err());
        assert!(!Path::new(path3).exists());
    }
}

#[cfg(all(test, feature = "dl-iso-test"))]
mod iso_tests {
    use super::*;
    use std::path::Path;

    const DL_URL: &str = "https://enterprise.proxmox.com/iso/proxmox-ve_9.1-1.iso";
    const DL_SHA256: &str = "6d8f5afc78c0c66812d7272cde7c8b98be7eb54401ceb045400db05eb5ae6d22";

    #[tokio::test]
    async fn test_download_opnsense_iso() {
        let path = "proxmox-ve_9.1-1.iso";
        let file_size = get_file_size(DL_URL).await.unwrap();

        println!("Downloading {} ({} MB)\n", path, file_size / (1024 * 1024));

        let start = std::time::Instant::now();
        let result = download_with_updates(DL_URL, path, None, Some(DL_SHA256)).await;
        let duration = start.elapsed();

        println!("\nDownload completed in {:.2?} seconds", duration);
        let speed_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        println!("Average download speed: {:.2} Mbps\n", speed_mbps);

        assert!(result.is_ok());
        assert!(Path::new(path).exists());
        let _ = std::fs::remove_file(path);
    }
}

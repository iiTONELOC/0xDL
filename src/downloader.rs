use crate::progress_bar::default_with_progress;
use crate::validator::{is_valid_file_path, is_valid_sha256, is_valid_url, verify_file_sha256};
use reqwest::Client;
use std::{
    fs::{self, File},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tempfile::NamedTempFile;
use tokio::{
    fs as tokio_fs,
    io::AsyncWriteExt,
    time::{Duration, MissedTickBehavior, interval},
};

pub type DownloadResult = Result<(), DownloadError>;

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

pub struct Downloader {
    pub url: String,
    pub path: String,
    pub sha256: Option<String>,
    pub current_progress: Arc<AtomicU64>,
    pub on_update: Option<Box<dyn Fn(f32) + Send + Sync + 'static>>,
}

pub async fn get_file_size(url: &str) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
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
    on_update: Option<Box<dyn Fn(f32) + Send + Sync + 'static>>,
    final_path: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let exited = Arc::new(AtomicBool::new(false));
    // HEAD request
    let size = get_file_size(url).await?;

    // temp file
    let temp_path = {
        let temp = NamedTempFile::new()?;
        let (_, buf) = temp.keep()?;
        buf.to_string_lossy().to_string()
    };

    let mut resp = Client::new().get(url).send().await?;
    let mut file = tokio_fs::File::create(&temp_path).await?;

    // progress callback loop
    if let Some(cb) = on_update {
        let p = progress.clone();
        let e = exited.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_millis(16));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tick.tick().await;

                let val = p.load(Ordering::Relaxed);
                if size > 0 {
                    cb((val as f32 / size as f32) * 100.0);
                }

                if size > 0 && val >= size {
                    break;
                }
            }
            cb(100.0);
            e.store(true, Ordering::Relaxed);
        });
    }

    // streaming download
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }

    file.flush().await?;

    // SHA256 check
    if let Some(expected) = sha256 {
        // wait for the progress callback to exit
        while !exited.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // print out a message indicating that verification is in progress
        println!("\nVerifying SHA256 hash...");
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

fn finalize_temp_file(
    temp: &str,
    final_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    fs::copy(temp, final_path)?;
    if let Ok(f) = File::open(final_path) {
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

    pub async fn execute(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

#[cfg(feature = "tests")]
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tokio::runtime::Runtime;

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
            Downloader::new(url(), "invalid|path.txt"),
            Err(DownloadError::InvalidPath)
        ));
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

#[cfg(feature = "net-tests")]
#[cfg(test)]
mod net_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[tokio::test]
    async fn test_download_file() {
        let url = _TEST_URLS[0];
        let downloader = Downloader::new(url, "downloaded_test_file.bin").unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            None,
            None,
            Some("downloaded_test_file.bin"),
        )
        .await
        .unwrap();

        assert!(Path::new(&result).exists());
        fs::remove_file(&result).unwrap();
    }

    #[tokio::test]
    async fn test_download_file_with_sha256() {
        let url = _TEST_URLS[0];
        let expected = "4f70c8056282511057ff5d99a5df660d06bcccac4dbb0383ce61477d5ab32581";

        let downloader = Downloader::new(url, "downloaded_test_file1.bin").unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            Some(expected.to_string()),
            None,
            Some("downloaded_test_file1.bin"),
        )
        .await
        .unwrap();

        assert!(Path::new(&result).exists());
        fs::remove_file(&result).unwrap();
    }

    #[tokio::test]
    async fn test_download_file_with_incorrect_sha256() {
        let url = _TEST_URLS[0];
        let bad = "4f70c8056282511057ff5d99a5df660d06bcccac4dbb0383ce61477d5ab32582";

        let downloader = Downloader::new(url, "downloaded_test_file2.bin").unwrap();

        let result = download_file(
            &downloader.url,
            downloader.current_progress.clone(),
            Some(bad.to_string()),
            None,
            Some("downloaded_test_file2.bin"),
        )
        .await;

        assert!(result.is_err());
        assert!(!Path::new("downloaded_test_file2.bin").exists());
    }

    #[tokio::test]
    async fn test_real_download() {
        let url = _TEST_URLS[1];
        let path = "test_download.bin";

        let result = download_with_updates(url, path, None, None).await;

        assert!(result.is_ok());
        assert!(Path::new(path).exists());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_real_download_with_hash() {
        let url = _TEST_URLS[1];
        let path = "test_download_with_hash.bin";
        let sha256 = "c7f262ffef3b3ad983cfddf4c41dc465ba6697e1396aaacba7212655a52627b5";

        let result = download_with_updates(url, path, None, Some(sha256)).await;

        assert!(result.is_ok());
        assert!(Path::new(path).exists());

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_real_download_with_incorrect_hash() {
        let url = _TEST_URLS[1];
        let path = "test_download_incorrect_hash.bin";
        let bad_sha = "e3aa82fb36dc042fb00b5e7fbe5f3e79d1dc5547ff94cdec1782aa43d9cde8e4";

        let result = download_with_updates(url, path, None, Some(bad_sha)).await;

        assert!(result.is_err());
        assert!(!Path::new(path).exists());
    }
}

#[cfg(feature = "dl-iso-test")]
#[cfg(test)]
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

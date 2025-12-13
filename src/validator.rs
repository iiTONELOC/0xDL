use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use url::Url;

/// src/validator.rs
/// This module provides validation functions for user input.

pub fn is_valid_url(url: &str) -> bool {
    let correct_protocol = url.starts_with("http://") || url.starts_with("https://");
    if !correct_protocol {
        return false;
    }
    Url::parse(url).is_ok()
}

pub fn is_valid_file_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') {
        return false;
    }

    #[cfg(windows)]
    {
        let illegal = ['<', '>', ':', '"', '\\', '|', '?', '*'];
        let reserved = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];

        std::path::Path::new(path).components().all(|c| {
            if let std::path::Component::Normal(os) = c {
                let s = os.to_string_lossy();
                !s.ends_with([' ', '.'])
                    && !illegal.iter().any(|ch| s.contains(*ch))
                    && !reserved.iter().any(|r| s.eq_ignore_ascii_case(r))
            } else {
                true
            }
        })
    }

    #[cfg(not(windows))]
    {
        // POSIX: only NUL and '/' are invalid inside a path component
        !path.split('/').any(|c| c.contains('\0'))
    }
}

pub fn sha256_regex() -> Regex {
    Regex::new(r"^[A-Fa-f0-9]{64}$").unwrap()
}

pub fn is_valid_sha256(s: &str) -> bool {
    sha256_regex().is_match(s)
}

pub fn sha256_matches(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
}

pub async fn verify_file_sha256(path: &str, expected_sha256: &str) -> Result<bool, std::io::Error> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let computed = format!("{:x}", hasher.finalize());
    Ok(sha256_matches(expected_sha256, &computed))
}

#[cfg(all(feature = "tests", test))]
mod tests {
    use super::*;
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    fn sha256() -> &'static str {
        "a3c1e2f4b5d6c7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2"
    }

    #[test]
    fn test_is_valid_url() {
        assert!(is_valid_url("http://example.com"));
        assert!(is_valid_url("https://example.com"));
        assert!(!is_valid_url("ftp://example.com"));
        assert!(!is_valid_url("invalid_url"));
    }

    #[test]
    fn test_is_valid_file_path() {
        assert!(is_valid_file_path("valid_path/file.txt"));
        assert!(!is_valid_file_path(""));

        if cfg!(windows) {
            assert!(!is_valid_file_path("invalid|path.txt"));
        } else {
            assert!(is_valid_file_path("invalid|path.txt"));
        }
    }

    #[test]
    fn test_is_valid_sha256() {
        assert!(is_valid_sha256(sha256()));
        assert!(!is_valid_sha256("invalid_sha256"));
    }

    #[test]
    fn test_sha256_matches() {
        let expected = sha256();
        let actual = sha256().to_ascii_uppercase();
        assert!(sha256_matches(expected, &actual));

        let non_matching = "b3c1e2f4b5d6c7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2";
        assert!(!sha256_matches(expected, non_matching));
    }

    #[tokio::test]
    async fn test_verify_file_sha256() -> Result<(), Box<dyn std::error::Error>> {
        let test_file_path = "hash_test_file.txt";

        // async file creation + write
        let mut file = File::create(test_file_path).await?;
        file.write_all(b"Hello, World!\n").await?;
        file.flush().await?;

        // compute expected hash for "Hello, World!\n"
        let expected = "c98c24b677eff44860afea6f493bbaec5bb1c4cbb209c6fc2bbb47f66ff2ad31";

        let result = verify_file_sha256(test_file_path, expected).await?;
        assert!(result);

        // incorrect hash
        let bad_expected = "c98c24b677eff44860afea6f493bbaec5bb1c4cbb209c6fc2bbb47f66ff2ad35";
        let bad_result = verify_file_sha256(test_file_path, bad_expected).await?;
        assert!(!bad_result);

        tokio::fs::remove_file(test_file_path).await?;
        Ok(())
    }
}

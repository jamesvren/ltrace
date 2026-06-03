//! A `std::io::Write` implementation that manages a log file with automatic rotation.
//!
//! The writer supports:
//! - Time-based rotation (MINUTELY, HOURLY, DAILY)
//! - Size-based rotation (configurable max file size)
//! - Optional gzip compression of rotated files (via the `compression` feature),
//!   performed on a background thread so it does not block writes
//! - Automatic pruning of old log files (configurable max count)
//!
//! The returned `RollingWriter` implements `std::io::Write`, which can be
//! used directly with `tracing_appender::non_blocking()` for non-blocking
//! writes, or with the `log_layer` module for `log` crate integration.

use crate::config::{Compression, Rotation, RotationConfig, Timezone};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Internal shared state of a `RollingWriter`.
struct RollingWriterInner {
    /// Path to the current log file.
    base_path: PathBuf,
    /// Configuration for rotation.
    config: RotationConfig,
    /// Currently open log file.
    current_file: File,
    /// Number of bytes written to `current_file`.
    current_size: u64,
    /// When the next time-based rotation should occur.
    next_time_rotation: chrono::DateTime<chrono::FixedOffset>,
}

/// A `std::io::Write` implementation that writes to a log file with
/// automatic rotation by time and/or size.
///
/// Use [`RollingWriter::builder`] to construct one.
pub struct RollingWriter {
    inner: Mutex<RollingWriterInner>,
}

impl RollingWriter {
    /// Create a new [`RollingWriterBuilder`] for the given log file path.
    pub fn builder<P: AsRef<Path>>(path: P) -> RollingWriterBuilder {
        RollingWriterBuilder::new(path)
    }

    fn inner(&self) -> MutexGuard<'_, RollingWriterInner> {
        self.inner.lock().expect("rolling writer mutex poisoned")
    }

    /// Rotate the current log file and open a new one.
    fn rotate(&self) -> io::Result<()> {
        let inner = self.inner();
        let path = inner.base_path.clone();
        let config = inner.config.clone();

        // Determine the timestamp suffix for the rotated file.
        // Use nanoseconds since UNIX epoch (monotonically increasing) instead of
        // nanosecond() (0-999999999 which can wrap around and cause incorrect sort order).
        let now = config.timezone.now();
        let nanos = now
            .timestamp_nanos_opt()
            .unwrap_or_else(|| now.timestamp_millis());
        let suffix = config.rotation.suffix_format();
        let timestamp = if suffix.is_empty() {
            format!("{:019}", nanos)
        } else {
            format!("{}T{:019}", now.format(suffix), nanos)
        };

        let rotated_path = path.with_file_name(format!(
            "{}.{}",
            path.file_name().expect("has filename").to_string_lossy(),
            timestamp
        ));

        drop(inner);

        // Rename the current file to the rotated path.
        if let Err(e) = fs::rename(&path, &rotated_path) {
            if e.kind() == io::ErrorKind::NotFound {
                // First write after open: file may not exist yet.
            } else {
                return Err(e);
            }
        }

        // Compress and prune in the background thread, after compression.
        if config.compression == Compression::Gzip {
            #[cfg(feature = "compression")]
            {
                let gz_path = PathBuf::from(format!("{}.gz", rotated_path.display()));
                let rotated_clone = rotated_path.clone();
                let base_path_clone = path.clone();
                let max_files = config.max_files;
                let prune_config = config.clone();
                std::thread::Builder::new()
                    .spawn(move || {
                        match compress_file(&rotated_clone, &gz_path) {
                            Ok(_) => {
                                let _ = fs::remove_file(&rotated_clone);
                            }
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                                // Already pruned by another rotation.
                            }
                            Err(_) => {}
                        }
                        // Prune AFTER compression completes, so .gz files are
                        // visible and deduplication works correctly.
                        if let Some(max) = max_files {
                            prune_old_files(&base_path_clone, max, &prune_config);
                        }
                    })
                    .ok();
            }
            #[cfg(not(feature = "compression"))]
            {
                let _ = rotated_path;
            }
        } else if let Some(max) = config.max_files {
            // No compression: prune immediately.
            prune_old_files(&path, max, &config);
        }

        // Reopen the current file.
        let new_file = open_log_file(&path)?;
        let mut inner = self.inner();
        inner.current_file = new_file;
        inner.current_size = 0;
        inner.next_time_rotation = config.rotation.next_rotation(config.timezone.now());

        Ok(())
    }

    /// Check if rotation is needed and perform it if so.
    ///
    /// When both time and size are configured, rotation only happens when
    /// the size limit is reached. Time-only rotation applies when no size
    /// limit is configured.
    fn check_rotation(&self) -> io::Result<()> {
        let inner = self.inner();
        let timezone = inner.config.timezone;
        let max_file_size = inner.config.max_file_size;
        let current_size = inner.current_size;
        drop(inner);

        let needs_time_rotation = timezone.now() >= self.inner().next_time_rotation;
        let needs_size_rotation = max_file_size.map_or(false, |max| current_size >= max);

        // When size limit is configured, skip time-only rotation.
        // Time rotation only applies when no size limit is set.
        let should_rotate = if max_file_size.is_some() {
            needs_size_rotation
        } else {
            needs_time_rotation
        };

        if should_rotate {
            self.rotate()?;
        }
        Ok(())
    }
}

impl Write for RollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check_rotation()?;
        let mut inner = self.inner();
        let written = inner.current_file.write(buf)?;
        inner.current_size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner().current_file.flush()
    }
}

impl Write for &RollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check_rotation()?;
        let mut inner = self.inner();
        let written = inner.current_file.write(buf)?;
        inner.current_size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner().current_file.flush()
    }
}

impl std::fmt::Debug for RollingWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RollingWriter")
            .field("path", &self.inner().base_path)
            .finish_non_exhaustive()
    }
}

use std::sync::MutexGuard;

/// Builder for [`RollingWriter`].
pub struct RollingWriterBuilder {
    path: PathBuf,
    config: RotationConfig,
}

impl RollingWriterBuilder {
    /// Create a new builder for the given log file path.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: RotationConfig::default(),
        }
    }

    /// Set the time-based rotation frequency.
    pub fn rotation(mut self, rotation: Rotation) -> Self {
        self.config.rotation = rotation;
        self
    }

    /// Set the maximum file size before rotation (in bytes).
    pub fn max_file_size(mut self, size: u64) -> Self {
        self.config.max_file_size = Some(size);
        self
    }

    /// Set the maximum number of archived log files to keep.
    /// When this limit is exceeded, the oldest files are deleted.
    pub fn max_files(mut self, max: usize) -> Self {
        self.config.max_files = Some(max);
        self
    }

    /// Set the compression mode for rotated files.
    pub fn compression(mut self, compression: Compression) -> Self {
        self.config.compression = compression;
        self
    }

    /// Set the timezone for rotation timestamps and filenames.
    /// Defaults to UTC.
    pub fn timezone(mut self, timezone: Timezone) -> Self {
        self.config.timezone = timezone;
        self
    }

    /// Build the [`RollingWriter`] and open the log file.
    pub fn build(self) -> io::Result<RollingWriter> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = open_log_file(&self.path)?;
        let current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let next_time_rotation = self
            .config
            .rotation
            .next_rotation(self.config.timezone.now());

        Ok(RollingWriter {
            inner: Mutex::new(RollingWriterInner {
                base_path: self.path,
                config: self.config,
                current_file: file,
                current_size,
                next_time_rotation,
            }),
        })
    }
}

fn open_log_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn prune_old_files(base_path: &Path, max_files: usize, config: &RotationConfig) {
    let parent = match base_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };

    let base_name = match base_path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };

    let prefix = format!("{}.", base_name);

    let mut archives: Vec<(PathBuf, i64, String)> = Vec::new();

    let entries = match fs::read_dir(&parent) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }

        let suffix = &name[prefix.len()..];
        let ts_str = suffix.strip_suffix(".gz").unwrap_or(suffix);

        if let Some(ts) = parse_timestamp(ts_str, config.rotation) {
            let nano_key = extract_nano_key(ts_str);
            archives.push((entry.path(), ts, nano_key));
        }
    }

    // When both a compressed (.gz) and uncompressed version of the same
    // rotation exist (compression in progress), only count the .gz file.
    let gz_keys: std::collections::HashSet<String> = archives
        .iter()
        .filter(|(_, _, key)| key.ends_with(".gz") || !key.is_empty())
        .filter_map(|(path, _, key)| {
            if path.extension().is_some_and(|e| e == "gz") {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect();

    archives.retain(|(path, _, key)| {
        if path.extension().is_some_and(|e| e == "gz") {
            true
        } else {
            // Keep uncompressed file only if no matching .gz exists.
            !gz_keys.contains(key)
        }
    });

    // Sort newest first.
    archives.sort_by(|a, b| b.1.cmp(&a.1));

    let to_delete: Vec<_> = archives.iter().skip(max_files).cloned().collect();
    if to_delete.is_empty() {
        return;
    }

    for (path, _, _) in &to_delete {
        if let Err(e) = fs::remove_file(path) {
            let _ = e;
        }
    }
}

/// Extract a key from a timestamp suffix for deduplicating
/// uncompressed files with their compressed (.gz) counterparts.
fn extract_nano_key(ts_str: &str) -> String {
    // Use the full timestamp string as the key.
    // Uncompressed and .gz files share the same base name.
    ts_str.to_string()
}

/// Parse the timestamp from a rotated filename.
///
/// For time-based rotation, filenames use the format
/// `{rotation_suffix}T{nanos_since_epoch:019}` (e.g. `app.log.2024-01-15T1780358400000000000`).
/// For size-only rotation (Never), filenames use the full nanoseconds since epoch.
fn parse_timestamp(s: &str, rotation: Rotation) -> Option<i64> {
    // For Never rotation, the filename is just the full nanoseconds since epoch.
    if matches!(rotation, Rotation::Never) {
        return s.parse::<i64>().ok();
    }

    // Time-based rotation: "{rotation_suffix}T{nanos_since_epoch:019}"
    let t_pos = s.rfind('T')?;
    let nano_part = &s[t_pos + 1..];
    if nano_part.len() != 19 || !nano_part.chars().all(|c| c.is_ascii_digit()) {
        return s.parse::<i64>().ok();
    }
    nano_part.parse::<i64>().ok()
}

#[cfg(feature = "compression")]
fn compress_file(input: &Path, output: &Path) -> io::Result<()> {
    use flate2::Compression as GzipCompression;
    use flate2::write::GzEncoder;

    let mut input_file = File::open(input)?;
    let output_file = File::create(output)?;
    let mut encoder = GzEncoder::new(output_file, GzipCompression::default());

    std::io::copy(&mut input_file, &mut encoder)?;
    encoder.finish()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_write_and_flush() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let mut writer = RollingWriterBuilder::new(&path).build().unwrap();
        write!(writer, "hello ").unwrap();
        write!(writer, "world\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.inner().current_size, 12);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[test]
    fn test_size_based_rotation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let mut writer = RollingWriterBuilder::new(&path)
            .max_file_size(10)
            .rotation(Rotation::Never)
            .build()
            .unwrap();

        write!(writer, "0123456789").unwrap();
        write!(writer, "ABCDEFGHIJ").unwrap();
        writer.flush().unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "ABCDEFGHIJ");
    }

    #[test]
    fn test_unique_names_on_rapid_rotation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let mut writer = RollingWriterBuilder::new(&path)
            .max_file_size(1)
            .rotation(Rotation::Never)
            .build()
            .unwrap();

        // Force 6 rotations by writing 1 byte each time.
        // After the first write (no rotation), every subsequent write triggers rotation.
        for i in 0..6 {
            write!(writer, "{}", i).unwrap();
            writer.flush().unwrap();
        }

        // Collect all rotated files in the directory.
        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| {
                let name = e.ok()?.file_name().to_string_lossy().to_string();
                if name.starts_with("test.log.") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        // 6 writes, first doesn't rotate. 5 rotations = 5 rotated files.
        assert_eq!(files.len(), 5, "expected 5 rotated files, got {:?}", files);
        // Verify all names are unique.
        let mut unique = files.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            files.len(),
            "duplicate names found: {:?}",
            files
        );
    }
}

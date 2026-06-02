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

use chrono::Timelike;

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
    /// Monotonically increasing counter for same-second disambiguation.
    rotation_seq: u64,
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
        let mut inner = self.inner();
        let path = inner.base_path.clone();
        let config = inner.config.clone();
        let seq = inner.rotation_seq;

        // Determine the timestamp suffix for the rotated file.
        // Append "-{seq}" to guarantee uniqueness even when multiple rotations
        // occur within the same microsecond (e.g., rapid size-triggered rotations).
        let now = config.timezone.now();
        let suffix = config.rotation.suffix_format();
        let timestamp = if suffix.is_empty() {
            format!(
                "{}-{:06}-{:03}",
                now.timestamp(),
                now.nanosecond() / 1000,
                seq
            )
        } else {
            format!(
                "{}-{:06}-{:03}",
                now.format(suffix),
                now.nanosecond() / 1000,
                seq
            )
        };

        // Build the rotated path: "app.log.2024-01-15-123456"
        // The microsecond suffix avoids collisions when multiple rotations
        // happen within the same time window (e.g., size-triggered rotations
        // within the same day).
        let rotated_path = path.with_file_name(format!(
            "{}.{}",
            path.file_name().expect("has filename").to_string_lossy(),
            timestamp
        ));

        inner.rotation_seq += 1;
        drop(inner);

        // Rename the current file to the rotated path.
        if let Err(e) = fs::rename(&path, &rotated_path) {
            // If the file doesn't exist yet (first write), that's fine.
            if e.kind() != io::ErrorKind::NotFound {
                return Err(e);
            }
        } else {
            // Compress if gzip is configured.
            let should_compress = config.compression == Compression::Gzip;

            if should_compress {
                #[cfg(feature = "compression")]
                {
                    let gz_path = PathBuf::from(format!("{}.gz", rotated_path.display()));
                    let rotated_clone = rotated_path.clone();
                    std::thread::Builder::new()
                        .name(format!("ltrace-gz-{:04}", seq % 10000))
                        .spawn(move || match compress_file(&rotated_clone, &gz_path) {
                            Ok(_) => fs::remove_file(&rotated_clone).ok(),
                            Err(e) => {
                                eprintln!(
                                    "ltrace: failed to compress {}: {}",
                                    rotated_clone.display(),
                                    e
                                );
                                None
                            }
                        })
                        .map_err(|e| eprintln!("ltrace: failed to spawn compress thread: {e}"))
                        .ok();
                }
                #[cfg(not(feature = "compression"))]
                {
                    let _ = rotated_path;
                }
            }

            // Enforce max file count.
            if let Some(max) = config.max_files {
                prune_old_files(&path, max, &config);
            }
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

    /// Returns the current file size in bytes.
    pub fn current_size(&self) -> u64 {
        self.inner().current_size
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
        let next_time_rotation = self
            .config
            .rotation
            .next_rotation(self.config.timezone.now());

        Ok(RollingWriter {
            inner: Mutex::new(RollingWriterInner {
                base_path: self.path,
                config: self.config,
                current_file: file,
                current_size: 0,
                next_time_rotation,
                rotation_seq: 0,
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

    let mut archives: Vec<(PathBuf, i64)> = Vec::new();

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
            archives.push((entry.path(), ts));
        }
    }

    // Sort newest first.
    archives.sort_by(|a, b| b.1.cmp(&a.1));

    for (path, _) in archives.iter().skip(max_files) {
        fs::remove_file(path).ok();
    }
}

/// Parse the timestamp from a rotated filename.
///
/// Filenames include a microsecond suffix and a seq number
/// (e.g. `app.log.2024-01-15-123456-001`), so we strip the trailing
/// `-NNNNNN-NNN` before parsing the date part.
fn parse_timestamp(s: &str, rotation: Rotation) -> Option<i64> {
    // The suffix is "-{micros:6}-{seq:3}". Strip the last 4 chars ("-NNN") for seq.
    let last_dash = s.rfind('-')?;
    if s.len() - last_dash - 1 != 3 {
        return s.parse::<i64>().ok();
    }
    let micros_str = &s[..last_dash];

    // Now strip the last 4 chars ("-NNNNNN") for microseconds.
    let second_last_dash = micros_str.rfind('-')?;
    let seq_micros = &micros_str[second_last_dash + 1..];
    if seq_micros.len() != 6 || !seq_micros.chars().all(|c| c.is_ascii_digit()) {
        return s.parse::<i64>().ok();
    }

    let date_part = &micros_str[..second_last_dash];

    let naive: chrono::NaiveDateTime = match rotation {
        Rotation::Minutely => {
            chrono::NaiveDateTime::parse_from_str(date_part, "%Y-%m-%d-%H-%M").ok()?
        }
        Rotation::Hourly => {
            let date = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d-%H").ok()?;
            chrono::NaiveDateTime::new(date, chrono::NaiveTime::MIN)
        }
        Rotation::Daily => {
            let date = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
            chrono::NaiveDateTime::new(date, chrono::NaiveTime::MIN)
        }
        Rotation::Never => {
            let base = date_part.parse::<i64>().ok()?;
            let micros: u32 = seq_micros.parse().unwrap_or(0);
            return Some(base * 1_000_000 + micros as i64);
        }
    };
    Some(naive.and_utc().timestamp())
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
        assert_eq!(writer.current_size(), 12);
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

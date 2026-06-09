//! Integration with the `log` crate.
//!
//! This module provides a [`LogWriter`] that implements the `log::Log` trait,
//! allowing it to be set as the global logger. It supports dynamic log level
//! changes at runtime using `log::LevelFilter`.
//!
//! The recommended approach is to call [`LogWriter::init`] directly:
//!
//! ```ignore
//! use ltrace::LogWriter;
//!
//! let handle = LogWriter::init("app.log")?;
//! ```
//!
//! Or use the builder for full configuration:
//!
//! ```ignore
//! use ltrace::{LogWriter, Rotation};
//!
//! let handle = LogWriter::builder("app.log")
//!     .rotation(Rotation::Daily)
//!     .level(log::LevelFilter::Info)
//!     .init()?;
//! ```

#[cfg(feature = "compression")]
use crate::config::Compression;
use crate::config::{Rotation, RotationConfig, Timezone};
use crate::handle;
use crate::rolling_writer::RollingWriter;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use handle::LogHandle;

/// Builder for creating a [`LogWriter`] and optionally initializing it as the global logger.
pub struct LogWriterBuilder {
    path: PathBuf,
    config: RotationConfig,
    level: log::LevelFilter,
}

impl LogWriterBuilder {
    /// Create a new builder for the given log file path.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: RotationConfig::default(),
            level: log::LevelFilter::Info,
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
    pub fn max_files(mut self, max: usize) -> Self {
        self.config.max_files = Some(max);
        self
    }

    /// Set the compression mode for rotated files.
    #[cfg(feature = "compression")]
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

    /// Set the initial log level.
    ///
    /// This level is used both for the writer and for `log::set_max_level()`.
    pub fn level(mut self, level: log::LevelFilter) -> Self {
        self.level = level;
        self
    }

    /// Build the [`LogWriter`] and register it as the global logger.
    ///
    /// This is the recommended way to initialize the logger. It combines
    /// [`build`](Self::build) and [`init`](Self::init) into a single call.
    ///
    /// Returns a [`LogHandle`] for dynamic level changes at runtime.
    /// The handle is `Clone + Send + Sync`, so it can be moved to other threads.
    pub fn init(self) -> io::Result<LogHandle> {
        self::init(self.build()?)
    }

    /// Build the [`LogWriter`] without registering it.
    ///
    /// If the log file's parent directory doesn't exist, it will be created
    /// automatically. Returns an error if the directory cannot be created or
    /// the log file cannot be opened.
    ///
    /// Use [`LogWriterBuilder::init`] to register as the global logger and
    /// get a [`LogHandle`] for dynamic level changes.
    pub fn build(self) -> io::Result<LogWriter> {
        #[cfg(feature = "compression")]
        let mut builder = RollingWriter::builder(&self.path)
            .rotation(self.config.rotation)
            .compression(self.config.compression)
            .timezone(self.config.timezone);
        #[cfg(not(feature = "compression"))]
        let mut builder = RollingWriter::builder(&self.path)
            .rotation(self.config.rotation)
            .timezone(self.config.timezone);
        if let Some(size) = self.config.max_file_size {
            builder = builder.max_file_size(size);
        }
        if let Some(max) = self.config.max_files {
            builder = builder.max_files(max);
        }
        let rolling = builder.build()?;

        let level = Arc::new(std::sync::atomic::AtomicU8::new(handle::level_to_u8(
            self.level,
        )));

        Ok(LogWriter {
            writer: rolling,
            level,
            timezone: self.config.timezone,
        })
    }
}

/// A logger that writes to rolling log files.
///
/// Implements `log::Log` so it can be set as the global logger via
/// [`log::set_logger`]. Supports dynamic log level changes at runtime.
///
/// Typically constructed via [`LogWriterBuilder`].
pub struct LogWriter {
    writer: RollingWriter,
    level: Arc<std::sync::atomic::AtomicU8>,
    timezone: Timezone,
}

impl LogWriter {
    /// Initialize the global logger with default settings (rotation: Daily, level: Info).
    ///
    /// This is a convenience method for the most common use case.
    /// Use [`builder`](Self::builder) for full configuration options.
    ///
    /// Returns a [`LogHandle`] for dynamic level changes at runtime.
    pub fn init<P: AsRef<Path>>(path: P) -> io::Result<LogHandle> {
        LogWriterBuilder::new(path).init()
    }

    /// Create a new builder for the given log file path.
    pub fn builder<P: AsRef<Path>>(path: P) -> LogWriterBuilder {
        LogWriterBuilder::new(path)
    }

    /// Get a handle to dynamically change the log level.
    ///
    /// The handle shares ownership of the level via `Arc`, so it can be
    /// cloned and sent across threads.
    pub fn handle(&self) -> LogHandle {
        LogHandle {
            level: self.level.clone(),
        }
    }

    /// Get the current log level.
    pub fn max_level(&self) -> log::LevelFilter {
        handle::u8_to_level(self.level.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl log::Log for LogWriter {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level()
            <= handle::u8_to_level(self.level.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => " WARN",
            log::Level::Info => " INFO",
            log::Level::Debug => "DEBUG",
            log::Level::Trace => "TRACE",
        };

        let timestamp = self
            .timezone
            .now()
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

        let line = format!(
            "{} {} {} {}\r\n",
            timestamp,
            level,
            record.target(),
            record.args(),
        );

        let _ = (&self.writer).write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _ = (&self.writer).flush();
    }
}

/// Initialize the global logger with a [`LogWriter`].
///
/// Sets the given writer as the global logger. The initial log level
/// is taken from the writer's configured level (set via [`LogWriterBuilder::level`]).
/// Returns a [`LogHandle`] for dynamic level changes at runtime.
///
/// In most cases, you should prefer [`LogWriterBuilder::init`] which combines
/// building and initialization in one call.
pub(crate) fn init(writer: LogWriter) -> io::Result<LogHandle> {
    let initial_level = writer.max_level();

    // Clone the Arc before moving writer into the Box, so we can return it.
    let level = writer.level.clone();

    let boxed = Box::new(writer);

    log::set_max_level(initial_level);

    log::set_boxed_logger(boxed).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // Ensure the level stored in the writer matches what we set globally.
    level.store(
        handle::level_to_u8(initial_level),
        std::sync::atomic::Ordering::Relaxed,
    );

    Ok(LogHandle { level })
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use tempfile::tempdir;

    #[test]
    fn test_log_writer_builder() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let writer = LogWriterBuilder::new(&path)
            .rotation(Rotation::Never)
            .max_file_size(1024)
            .max_files(3)
            .level(log::LevelFilter::Debug)
            .build()?;
        assert_eq!(writer.max_level(), log::LevelFilter::Debug);

        let handle = writer.handle();
        handle.set_level(log::LevelFilter::Trace);
        assert_eq!(handle.get_level(), log::LevelFilter::Trace);
        Ok(())
    }

    #[test]
    fn test_log_writer_log_trait() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let writer = LogWriterBuilder::new(&path)
            .rotation(Rotation::Never)
            .level(log::LevelFilter::Info)
            .build()?;

        assert!(
            writer.enabled(
                &log::Record::builder()
                    .level(log::Level::Info)
                    .target("test")
                    .build()
                    .metadata()
            )
        );

        assert!(
            !writer.enabled(
                &log::Record::builder()
                    .level(log::Level::Debug)
                    .target("test")
                    .build()
                    .metadata()
            )
        );

        let handle = writer.handle();
        handle.set_level(log::LevelFilter::Debug);
        assert!(
            writer.enabled(
                &log::Record::builder()
                    .level(log::Level::Debug)
                    .target("test")
                    .build()
                    .metadata()
            )
        );
        Ok(())
    }

    #[test]
    fn test_log_handle_is_clone_send_sync() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let writer = LogWriterBuilder::new(&path)
            .rotation(Rotation::Never)
            .level(log::LevelFilter::Info)
            .build()?;

        let handle = writer.handle();
        let handle2 = handle.clone();

        // Verify Send + Sync (compile-time check).
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LogHandle>();
        assert_sync::<LogHandle>();

        // Verify clone shares the same level.
        handle2.set_level(log::LevelFilter::Debug);
        assert_eq!(handle.get_level(), log::LevelFilter::Debug);
        Ok(())
    }

    #[test]
    fn test_concurrent_writes_from_threads() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let writer = Arc::new(
            LogWriterBuilder::new(&path)
                .rotation(Rotation::Never)
                .level(log::LevelFilter::Debug)
                .build()?,
        );

        let mut jhs = Vec::new();
        for i in 0..8 {
            let w = writer.clone();
            jhs.push(std::thread::spawn(move || {
                for j in 0..50 {
                    w.log(
                        &log::Record::builder()
                            .level(log::Level::Info)
                            .target("test")
                            .args(format_args!("thread-{i}-msg-{j}"))
                            .build(),
                    );
                }
                w.flush();
            }));
        }
        for jh in jhs {
            jh.join().unwrap();
        }

        let content = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.lines().collect();
        // 8 threads * 50 messages = 400 lines
        assert_eq!(lines.len(), 400, "expected 400 lines, got {}", lines.len());
        Ok(())
    }

    #[test]
    fn test_concurrent_level_change_during_writes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let writer = Arc::new(
            LogWriterBuilder::new(&path)
                .rotation(Rotation::Never)
                .level(log::LevelFilter::Info)
                .build()?,
        );
        let handle = writer.handle();

        // Writer threads: log at all levels
        let mut wjhs = Vec::new();
        for i in 0..4 {
            let w = writer.clone();
            wjhs.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    // Log at Info level so it's always accepted (min level is Info).
                    w.log(
                        &log::Record::builder()
                            .level(log::Level::Info)
                            .target("test")
                            .args(format_args!("msg-{i}"))
                            .build(),
                    );
                    std::thread::yield_now();
                }
                w.flush();
            }));
        }

        // Level changer thread: toggle between Info and Debug
        let h = handle.clone();
        let ljh = std::thread::spawn(move || {
            for _ in 0..100 {
                h.set_level(log::LevelFilter::Debug);
                std::thread::yield_now();
                h.set_level(log::LevelFilter::Info);
                std::thread::yield_now();
            }
        });

        for jh in wjhs {
            jh.join().unwrap();
        }
        ljh.join().unwrap();

        // Verify no panic/drop issues: the file exists and has some content.
        assert!(path.exists());
        let content = std::fs::read_to_string(&path)?;
        // At least Info-level messages should be present regardless of level toggling
        assert!(!content.is_empty());
        Ok(())
    }
}

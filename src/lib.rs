//! # ltrace — Rolling file writer and multi-target logger for `tracing` and `log`
//!
//! This crate provides a high-performance rolling file writer with:
//!
//! - **Log rotation by time**: MINUTELY, HOURLY, DAILY
//! - **Log rotation by size**: configurable max file size (independent of time rotation)
//! - **Time + size combined rotation**: even when using time-based rotation (e.g. Daily),
//!   a size limit still triggers additional rotations within that window (e.g., multiple
//!   rotations per day if the log exceeds the size threshold)
//! - **Compression**: optional gzip compression for rotated files (feature `compression`),
//!   performed on a background thread so it does not block writes
//! - **Max file count**: automatically prune old log files
//! - **Dynamic log level**: change the log level at runtime via [`LogHandle`]
//! - **Timezone configuration**: UTC (default) or local timezone for rotation timestamps
//! - **Auto-create directories**: parent directories are created automatically if they don't exist
//!
//! ## Using with `tracing`
//!
//! Since [`RollingWriter`] implements `std::io::Write`, you can use it with
//! `tracing_appender::non_blocking()` to get non-blocking writes:
//!
//! ```ignore
//! use ltrace::{RollingWriter, Rotation, Compression};
//!
//! let writer = RollingWriter::builder("/var/log/myapp/app.log")
//!     .rotation(Rotation::Daily)
//!     .max_file_size(10 * 1024 * 1024)  // 10 MB
//!     .max_files(10)
//!     .compression(Compression::Gzip)
//!     .build()
//!     .unwrap();
//!
//! let (non_blocking, _guard) = tracing_appender::non_blocking(writer);
//! let subscriber = tracing_subscriber::fmt()
//!     .with_writer(non_blocking)
//!     .finish()
//!     .init()?;
//! ```
//!
//! ## Output targets
//!
//! ltrace supports multiple log output targets that can be combined:
//!
//! - **Rolling file logs**: [`LogWriter`] for time/size-based rotating file logs
//! - **Console output**: [`ConsoleWriter`] for colored terminal output (feature `console`)
//! - **Multi-target output**: [`MultiLog`] to combine multiple writers together
//!
//! ## Rotated file naming
//!
//! Rotated filenames use a nanosecond-precision timestamp (nanoseconds since UNIX epoch)
//! to guarantee strict ordering and uniqueness, with an optional date prefix:
//!
//! - Daily rotation: `app.log.2024-01-15T1780358400000000000`
//! - Hourly rotation: `app.log.2024-01-15-14T1780358400000000000`
//! - Minutely rotation: `app.log.2024-01-15-14-30T1780358400000000000`
//! - Size-only (Never): `app.log.1780358400000000000`
//!
//! With gzip compression enabled: `app.log.2024-01-15T1780358400000000000.gz`
//!
//! # Quick start
//!
//! Enable the `log` feature in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! ltrace = { version = "0.1", features = ["log"] }
//! ```
//!
//! The recommended way is to call [`LogWriter::init`] directly:
//!
//! ```ignore
//! use ltrace::LogWriter;
//!
//! let handle = LogWriter::init("app.log")?;
//!
//! // Dynamically change level at runtime:
//! handle.set_level(log::LevelFilter::Debug);
//! ```
//!
//! # Full builder configuration
//!
//! Use [`LogWriter::builder`] for complete control over rotation, compression, and levels:
//!
//! ```ignore
//! use ltrace::{LogWriter, Rotation, Compression};
//!
//! let handle = LogWriter::builder("/var/log/myapp/app.log")
//!     .rotation(Rotation::Daily)
//!     .max_file_size(10 * 1024 * 1024)  // also rotate within the same day if size exceeded
//!     .max_files(5)
//!     .compression(Compression::Gzip)
//!     .level(log::LevelFilter::Info)
//!     .init()?;
//! ```
//!
//! # Console output (feature `console`)
//!
//! Enable the `console` feature in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! ltrace = { version = "0.1", features = ["console"] }
//! ```
//!
//! ```ignore
//! use ltrace::ConsoleWriter;
//!
//! let handle = ConsoleWriter::builder()
//!     .level(log::LevelFilter::Debug)
//!     .init()?;
//! ```
//!
//! # Multi-target output (console + file)
//!
//! ```ignore
//! use ltrace::{MultiLog, ConsoleWriter, LogWriter, Rotation, Compression};
//!
//! let multi = MultiLog::new()
//!     .writer(ConsoleWriter::builder()
//!         .level(log::LevelFilter::Info)
//!         .build()?)
//!     .writer(LogWriter::builder("/var/log/myapp/app.log")
//!         .rotation(Rotation::Daily)
//!         .compression(Compression::Gzip)
//!         .level(log::LevelFilter::Debug)
//!         .build()?)
//!     .init()?;
//! ```
//!
//! # Multi-target with per-writer level control
//!
//! ```ignore
//! use ltrace::{MultiLog, ConsoleWriter, LogWriter};
//!
//! let (multi, handles) = MultiLog::new()
//!     .writer_with_handle(ConsoleWriter::builder().build()?)
//!     .writer_with_handle(LogWriter::builder("app.log").build()?)
//!     .build_with_handles()?;
//!
//! // Adjust each writer's level independently:
//! handles[0].set_level(log::LevelFilter::Warn); // console: only warn+
//! handles[1].set_level(log::LevelFilter::Debug); // file: debug+
//! ```
//!
//! # Dynamic log level control
//!
//! [`LogHandle`] is `Clone + Send + Sync`, so you can move it to other threads:
//!
//! ```ignore
//! let handle = LogWriter::init("app.log")?;
//!
//! // Spawn a thread that listens for signals to change the log level.
//! let handle_clone = handle.clone();
//! std::thread::spawn(move || {
//!     // e.g., listen for SIGHUP or an HTTP endpoint
//!     handle_clone.set_level(log::LevelFilter::Debug);
//! });
//! ```

// Core types (always available)
pub use config::{Rotation, Timezone};
pub use rolling_writer::RollingWriter;

mod config;
mod rolling_writer;

#[cfg(feature = "compression")]
pub use config::Compression;

// Log layer types (feature: log)
#[cfg(feature = "log")]
pub use handle::LogHandle;

#[cfg(feature = "log")]
mod handle;

#[cfg(feature = "log")]
pub use log_layer::{LogWriter, LogWriterBuilder};

#[cfg(feature = "log")]
mod log_layer;

// Console writer types (feature: console)
#[cfg(feature = "console")]
pub use console_writer::{ConsoleWriter, ConsoleWriterBuilder};

#[cfg(feature = "console")]
mod console_writer;

// Multi writer types (feature: log)
#[cfg(feature = "log")]
pub use multi_writer::{MultiHandle, MultiLog, MultiLogBuilder, MultiLogBuilderWithHandles};

#[cfg(feature = "log")]
mod multi_writer;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_basic_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let mut writer = RollingWriter::builder(&path).build().unwrap();
        use std::io::Write;
        write!(writer, "hello world\n").unwrap();
        writer.flush().unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[test]
    fn test_auto_create_parent_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subdir/deep/app.log");
        assert!(!path.parent().unwrap().exists());
        let _writer = RollingWriter::builder(&path).build().unwrap();
        assert!(path.parent().unwrap().exists());
        assert!(path.exists());
    }
}

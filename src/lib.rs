//! # ltrace — Rolling file writer for `tracing` and `log`
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
//! - **Dynamic log level**: change the `log` crate log level at runtime via [`log_layer::LogHandle`]
//! - **Timezone configuration**: UTC (default) or local timezone for rotation timestamps
//! - **Auto-create directories**: parent directories are created automatically if they don't exist
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
//! # Using with `tracing`
//!
//! Since `RollingWriter` implements `std::io::Write`, you can use it with
//! `tracing_appender::non_blocking()` to get non-blocking writes:
//!
//! ```ignore
//! use ltrace::{RollingWriter, Rotation};
//!
//! let writer = RollingWriter::builder("/var/log/myapp/app.log")
//!     .rotation(Rotation::Daily)
//!     .max_file_size(10 * 1024 * 1024)  // 10 MB
//!     .max_files(10)
//!     .compression(ltrace::Compression::Gzip)
//!     .build()
//!     .unwrap();
//!
//! let (non_blocking, _guard) = tracing_appender::non_blocking(writer);
//! tracing_subscriber::fmt()
//!     .with_ansi(false)
//!     .with_writer(non_blocking)
//!     .finish()
//!     .try_init()
//!     .unwrap();
//! ```
//!
//! # Using with `log`
//!
//! Enable the `log` feature in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! ltrace = { version = "0.1", features = ["log"] }
//! ```
//!
//! The recommended way is to chain [`init`](log_layer::LogWriterBuilder::init) directly on the builder:
//!
//! ```ignore
//! use ltrace::log_layer::LogWriterBuilder;
//! use ltrace::{Rotation, Compression};
//!
//! let handle = LogWriterBuilder::new("/var/log/myapp/app.log")
//!     .rotation(Rotation::Daily)
//!     .max_file_size(10 * 1024 * 1024)  // optional: also rotate within the same day if size exceeded
//!     .max_files(5)
//!     .compression(Compression::Gzip)
//!     .level(log::LevelFilter::Info)
//!     .init()?;
//!
//! // Dynamically change level at runtime:
//! handle.set_level(log::LevelFilter::Debug);
//! ```
//!
//! # Dynamic log level control
//!
//! [`log_layer::LogHandle`] is `Clone + Send + Sync`, so you can move it to other threads:
//!
//! ```ignore
//! let handle = LogWriterBuilder::new("app.log")
//!     .level(log::LevelFilter::Info)
//!     .init()?;
//!
//! // Spawn a thread that listens for signals to change the log level.
//! let handle_clone = handle.clone();
//! std::thread::spawn(move || {
//!     // e.g., listen for SIGHUP or an HTTP endpoint
//!     handle_clone.set_level(log::LevelFilter::Debug);
//! });
//! ```

pub use config::{Compression, Rotation, Timezone};
pub use rolling_writer::RollingWriter;

mod config;
mod rolling_writer;

#[cfg(feature = "log")]
pub mod log_layer;

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

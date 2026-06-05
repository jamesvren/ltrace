//! Console log writer with colored output.
//!
//! This module provides [`ConsoleWriter`] that implements `log::Log`,
//! outputting logs to the terminal with ANSI colors. It is designed to
//! be used standalone or as part of a [`MultiLog`].
//!
//! # Standalone usage
//!
//! ```ignore
//! use ltrace::ConsoleWriter;
//!
//! let handle = ConsoleWriter::builder()
//!     .level(log::LevelFilter::Debug)
//!     .init()?;
//! ```
//!
//! # With MultiLog
//!
//! ```ignore
//! use ltrace::{MultiLog, ConsoleWriter, LogWriter};
//!
//! let multi = MultiLog::new()
//!     .writer(ConsoleWriter::builder().level(log::LevelFilter::Debug).build()?)
//!     .writer(LogWriter::builder("app.log").rotation(Rotation::Daily).build()?)
//!     .init()?;
//! ```

use crate::handle;

use std::io::{self, Write};
use std::sync::Arc;

#[cfg(feature = "console")]
use colored::Colorize;

pub use handle::LogHandle;

/// Builder for creating a [`ConsoleWriter`].
pub struct ConsoleWriterBuilder {
    level: log::LevelFilter,
    timezone: crate::config::Timezone,
}

impl ConsoleWriterBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            level: log::LevelFilter::Info,
            timezone: crate::config::Timezone::Utc,
        }
    }

    /// Set the initial log level.
    pub fn level(mut self, level: log::LevelFilter) -> Self {
        self.level = level;
        self
    }

    /// Set the timezone for log timestamps.
    pub fn timezone(mut self, timezone: crate::config::Timezone) -> Self {
        self.timezone = timezone;
        self
    }

    /// Build the [`ConsoleWriter`] without registering it.
    ///
    /// Use [`crate::log_layer::init`], [`crate::multi_writer::MultiLog::init`], or
    /// [`log::set_logger`] to register the writer as the global logger.
    pub fn build(self) -> io::Result<ConsoleWriter> {
        let level = Arc::new(std::sync::atomic::AtomicU8::new(
            handle::level_to_u8(self.level),
        ));
        Ok(ConsoleWriter {
            level,
            timezone: self.timezone,
        })
    }
}

impl Default for ConsoleWriterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A logger that writes colored logs to the console (stdout/stderr).
///
/// Implements `log::Log` so it can be set as the global logger via [`log::set_logger`],
/// or added to a [`MultiLog`](crate::multi_writer::MultiLog).
///
/// Error and warning logs go to stderr; info, debug, and trace go to stdout.
///
/// When the `colored` feature is enabled, output is ANSI-colored:
/// - **ERROR**: bold red
/// - **WARN**: yellow
/// - **INFO**: green
/// - **DEBUG**: cyan
/// - **TRACE**: dimmed/bright black
///
/// Typically constructed via [`ConsoleWriterBuilder`].
pub struct ConsoleWriter {
    level: Arc<std::sync::atomic::AtomicU8>,
    timezone: crate::config::Timezone,
}

impl ConsoleWriter {
    /// Create a new builder for the console writer.
    pub fn builder() -> ConsoleWriterBuilder {
        ConsoleWriterBuilder::new()
    }

    /// Get a handle to dynamically change the log level.
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

impl log::Log for ConsoleWriter {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level()
            <= handle::u8_to_level(self.level.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        #[cfg(feature = "console")]
        let line = {
            let timestamp = self
                .timezone
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

            let level_str = match record.level() {
                log::Level::Error => "ERROR".bold().red().to_string(),
                log::Level::Warn => " WARN".yellow().to_string(),
                log::Level::Info => " INFO".green().to_string(),
                log::Level::Debug => "DEBUG".cyan().to_string(),
                log::Level::Trace => "TRACE".bright_black().to_string(),
            };

            format!(
                "{} {} {} {}\n",
                timestamp.dimmed(),
                level_str,
                record.target().italic(),
                record.args(),
            )
        };

        #[cfg(not(feature = "console"))]
        let line = {
            let timestamp = self
                .timezone
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

            let level = match record.level() {
                log::Level::Error => "ERROR",
                log::Level::Warn => " WARN",
                log::Level::Info => " INFO",
                log::Level::Debug => "DEBUG",
                log::Level::Trace => "TRACE",
            };

            format!(
                "{} {} {} {}\n",
                timestamp,
                level,
                record.target(),
                record.args(),
            )
        };

        // Error and warning to stderr, others to stdout
        let result = match record.level() {
            log::Level::Error | log::Level::Warn => {
                let stderr = io::stderr();
                let mut handle = stderr.lock();
                handle.write_all(line.as_bytes()).and_then(|_| handle.flush())
            }
            _ => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                handle.write_all(line.as_bytes()).and_then(|_| handle.flush())
            }
        };

        let _ = result;
    }

    fn flush(&self) {
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
    }
}

/// Initialize the global logger with a [`ConsoleWriter`].
///
/// Sets the given writer as the global logger. The initial log level
/// is taken from the writer's configured level (set via [`ConsoleWriterBuilder::level`]).
/// Returns a [`LogHandle`] for dynamic level changes at runtime.
///
/// In most cases, you should prefer [`ConsoleWriterBuilder::init`] which combines
/// building and initialization in one call.
///
/// # Panics
///
/// If called without the `console` feature, this function will still work, but
/// output will not be colored.
pub fn init(writer: ConsoleWriter) -> io::Result<LogHandle> {
    let initial_level = writer.max_level();
    let level = writer.level.clone();

    let boxed = Box::new(writer);

    log::set_max_level(initial_level);

    log::set_boxed_logger(boxed).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    level.store(
        handle::level_to_u8(initial_level),
        std::sync::atomic::Ordering::Relaxed,
    );

    Ok(LogHandle { level })
}

impl ConsoleWriterBuilder {
    /// Build the [`ConsoleWriter`] and register it as the global logger.
    ///
    /// Returns a [`LogHandle`] for dynamic level changes at runtime.
    /// The handle is `Clone + Send + Sync`, so it can be moved to other threads.
    pub fn init(self) -> io::Result<LogHandle> {
        init(self.build()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_defaults() {
        let builder = ConsoleWriterBuilder::new();
        let writer = builder.build().unwrap();
        assert_eq!(writer.max_level(), log::LevelFilter::Info);
    }

    #[test]
    fn test_custom_level() {
        let builder = ConsoleWriterBuilder::new().level(log::LevelFilter::Debug);
        let writer = builder.build().unwrap();
        assert_eq!(writer.max_level(), log::LevelFilter::Debug);
    }

    #[test]
    fn test_handle_clone_send_sync() {
        let builder = ConsoleWriterBuilder::new().level(log::LevelFilter::Info);
        let writer = builder.build().unwrap();
        let handle = writer.handle();
        let handle2 = handle.clone();

        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LogHandle>();
        assert_sync::<LogHandle>();

        handle2.set_level(log::LevelFilter::Debug);
        assert_eq!(handle.get_level(), log::LevelFilter::Debug);
    }
}

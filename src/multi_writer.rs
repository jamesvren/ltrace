//! Multi-target log dispatcher that writes to multiple [`log::Log`] writers.
//!
//! This module provides [`MultiLog`] which implements `log::Log` and dispatches
//! log records to all registered writers. It allows combining different log
//! backends (e.g. console + file, multiple files with different levels).
//!
//! # Example: Console + File
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
//! # Example: Dynamic level control per writer
//!
//! ```ignore
//! use ltrace::{MultiLog, ConsoleWriter, LogWriter};
//!
//! let (multi, handles) = MultiLog::new()
//!     .writer_with_handle(ConsoleWriter::builder()
//!         .level(log::LevelFilter::Info)
//!         .build()?)
//!     .writer_with_handle(LogWriter::builder("/var/log/myapp/app.log")
//!         .level(log::LevelFilter::Debug)
//!         .build()?)
//!     .build_with_handles()?;
//!
//! // Dynamically change the first writer's level
//! handles[0].set_level(log::LevelFilter::Error);
//! ```

use crate::handle;

use std::io;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

pub use handle::LogHandle;

/// Internal wrapper that wraps a boxed writer with an external level filter.
struct WriterEntry {
    writer: Box<dyn log::Log>,
}

impl WriterEntry {
    fn new(w: impl log::Log + 'static) -> (Self, LogHandle) {
        let level = Arc::new(AtomicU8::new(handle::level_to_u8(log::LevelFilter::Trace)));
        let log_handle = LogHandle { level: level.clone() };
        (
            Self {
                writer: Box::new(Wrapper {
                    writer: Box::new(w),
                    level,
                }),
            },
            log_handle,
        )
    }

    fn new_boxed(w: Box<dyn log::Log>) -> (Self, LogHandle) {
        let level = Arc::new(AtomicU8::new(handle::level_to_u8(log::LevelFilter::Trace)));
        let log_handle = LogHandle { level: level.clone() };
        (
            Self {
                writer: Box::new(Wrapper { writer: w, level }),
            },
            log_handle,
        )
    }
}

/// A wrapper that applies dynamic level filtering via an `Arc<AtomicU8>`.
struct Wrapper {
    writer: Box<dyn log::Log>,
    level: Arc<AtomicU8>,
}

impl log::Log for Wrapper {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let current_level =
            handle::u8_to_level(self.level.load(Ordering::Relaxed));
        metadata.level() <= current_level && self.writer.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        let current_level =
            handle::u8_to_level(self.level.load(Ordering::Relaxed));
        if record.level() <= current_level {
            self.writer.log(record);
        }
    }

    fn flush(&self) {
        self.writer.flush();
    }
}

/// A logger that dispatches records to multiple [`log::Log`] writers.
///
/// Each writer receives every log record independently, so each can apply
/// its own level filtering. This enables scenarios like:
///
/// - Logging to both console and file simultaneously
/// - Writing errors to a separate file from debug logs
/// - Combining any `log::Log` implementation
///
/// Use [`MultiLogBuilder`] to construct it.
pub struct MultiLog {
    writers: Vec<Box<dyn log::Log>>,
}

impl MultiLog {
    /// Create a new empty multi-log builder.
    pub fn new() -> MultiLogBuilder {
        MultiLogBuilder::new()
    }
}

impl log::Log for MultiLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.writers
            .iter()
            .any(|w| w.enabled(metadata))
    }

    fn log(&self, record: &log::Record<'_>) {
        for writer in &self.writers {
            if writer.enabled(record.metadata()) {
                writer.log(record);
            }
        }
    }

    fn flush(&self) {
        for writer in &self.writers {
            writer.flush();
        }
    }
}

/// Builder for creating a [`MultiLog`] with multiple writers.
pub struct MultiLogBuilder {
    entries: Vec<WriterEntry>,
}

impl MultiLogBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a log writer to the multi-log.
    ///
    /// The writer will receive all log records. Each writer applies its own
    /// level filtering via its `enabled()` method.
    ///
    /// If you need to dynamically adjust the level of this writer after building,
    /// use [`writer_with_handle`](Self::writer_with_handle) instead.
    pub fn writer(mut self, w: impl log::Log + 'static) -> Self {
        let (entry, _) = WriterEntry::new(w);
        self.entries.push(entry);
        self
    }

    /// Add a boxed log writer.
    ///
    /// This is useful when you need to add writers whose concrete type is
    /// not known at compile time.
    pub fn writer_boxed(mut self, w: Box<dyn log::Log>) -> Self {
        let (entry, _) = WriterEntry::new_boxed(w);
        self.entries.push(entry);
        self
    }

    /// Add a log writer and return a [`LogHandle`] for dynamic level control.
    ///
    /// This wraps the writer with an external level filter controlled by the
    /// returned handle. The handle's default level is `Trace` (all messages pass).
    /// The writer's own internal filtering still applies — both the handle's
    /// level AND the writer's level must accept a record for it to be logged.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (multi, handles) = MultiLog::new()
    ///     .writer_with_handle(ConsoleWriter::builder().build()?)
    ///     .writer_with_handle(LogWriter::builder("app.log").build()?)
    ///     .build_with_handles()?;
    ///
    /// // Later, dynamically change levels
    /// handles[0].set_level(log::LevelFilter::Error);
    /// handles[1].set_level(log::LevelFilter::Trace);
    /// ```
    ///
    /// Note: `LogHandle` is `Clone + Send + Sync`, so you can send it to
    /// other threads for remote level control.
    pub fn writer_with_handle(self, w: impl log::Log + 'static) -> MultiLogBuilderWithHandles {
        let (entry, log_handle) = WriterEntry::new(w);
        let mut builder = MultiLogBuilderWithHandles {
            entries: Vec::new(),
            handles: Vec::new(),
        };
        builder.entries.extend(self.entries);
        builder.entries.push(entry);
        builder.handles.push(log_handle);
        builder
    }

    /// Build the [`MultiLog`].
    ///
    /// Returns an error if no writers have been added.
    pub fn build(self) -> io::Result<MultiLog> {
        if self.entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MultiLog requires at least one writer",
            ));
        }
        Ok(MultiLog {
            writers: self.entries.into_iter().map(|e| e.writer).collect(),
        })
    }

    /// Build and register as the global logger.
    ///
    /// Sets the global max level to the highest (most verbose) level among
    /// all writers.
    ///
    /// Returns an [`MultiHandle`] for dynamically changing the effective level.
    pub fn init(self) -> io::Result<MultiHandle> {
        let multi = self.build()?;

        // Set global max to the most permissive (Trace) so all writers
        // can receive the records they need; each writer applies its own filtering.
        let max_level = log::LevelFilter::Trace;

        let level = Arc::new(AtomicU8::new(handle::level_to_u8(max_level)));
        log::set_max_level(max_level);

        let boxed = Box::new(multi);
        log::set_boxed_logger(boxed).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(MultiHandle {
            level,
            writer_handles: None,
        })
    }
}

impl Default for MultiLogBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating a [`MultiLog`] with per-writer dynamic level handles.
///
/// This builder is returned by [`MultiLogBuilder::writer_with_handle`] and
/// provides [`build_with_handles`](Self::build_with_handles) and
/// [`init_with_handles`](Self::init_with_handles) methods that return
/// [`LogHandle`] for each writer.
pub struct MultiLogBuilderWithHandles {
    entries: Vec<WriterEntry>,
    handles: Vec<LogHandle>,
}

impl MultiLogBuilderWithHandles {
    /// Add another log writer and return a [`LogHandle`] for dynamic level control.
    pub fn writer_with_handle(mut self, w: impl log::Log + 'static) -> Self {
        let (entry, log_handle) = WriterEntry::new(w);
        self.entries.push(entry);
        self.handles.push(log_handle);
        self
    }

    /// Build the [`MultiLog`] and return all writer handles.
    ///
    /// Returns an error if no writers have been added.
    pub fn build_with_handles(self) -> io::Result<(MultiLog, Vec<LogHandle>)> {
        if self.entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MultiLog requires at least one writer",
            ));
        }
        let multi = MultiLog {
            writers: self.entries.into_iter().map(|e| e.writer).collect(),
        };
        Ok((multi, self.handles))
    }

    /// Build and register as the global logger, returning a handle with per-writer access.
    ///
    /// Sets the global max level to `Trace` so all writers can receive records.
    /// Returns an [`MultiHandle`] containing all writer handles.
    pub fn init_with_handles(self) -> io::Result<MultiHandle> {
        let (multi, handles) = self.build_with_handles()?;

        let max_level = log::LevelFilter::Trace;
        let level = Arc::new(AtomicU8::new(handle::level_to_u8(max_level)));
        log::set_max_level(max_level);

        let boxed = Box::new(multi);
        log::set_boxed_logger(boxed).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(MultiHandle {
            level,
            writer_handles: Some(handles),
        })
    }
}

/// A handle returned by [`MultiLogBuilder::init`] or [`MultiLogBuilderWithHandles::init_with_handles`]
/// for managing the multi-log.
///
/// When created via `init_with_handles`, you can access per-writer [`LogHandle`]s
/// to dynamically adjust their log levels via [`writer_handles`](Self::writer_handles)
/// or [`writer_handle`](Self::writer_handle).
#[derive(Clone)]
pub struct MultiHandle {
    level: Arc<AtomicU8>,
    writer_handles: Option<Vec<LogHandle>>,
}

impl MultiHandle {
    /// Get the effective max level of the multi-log.
    pub fn max_level(&self) -> log::LevelFilter {
        handle::u8_to_level(self.level.load(Ordering::Relaxed))
    }

    /// Get all per-writer handles.
    ///
    /// Returns `Some` if the multi-log was created via `init_with_handles`
    /// or `build_with_handles`. Each [`LogHandle`] can be used to
    /// dynamically adjust the level of its corresponding writer.
    ///
    /// Returns `None` if created via `init()` or `build()` (which don't track handles).
    pub fn writer_handles(&self) -> Option<&[LogHandle]> {
        self.writer_handles.as_deref()
    }

    /// Get a specific writer handle by index.
    ///
    /// Returns `None` if out of bounds or if the multi-log was not created
    /// with handle tracking.
    pub fn writer_handle(&self, index: usize) -> Option<&LogHandle> {
        self.writer_handles.as_ref()?.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use std::sync::{Arc, Mutex};

    /// A test writer that captures log records.
    struct TestWriter {
        level: log::LevelFilter,
        records: Arc<Mutex<Vec<String>>>,
    }

    impl TestWriter {
        fn new(level: log::LevelFilter) -> (Self, Arc<Mutex<Vec<String>>>) {
            let records = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    level,
                    records: records.clone(),
                },
                records,
            )
        }
    }

    impl log::Log for TestWriter {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= self.level
        }

        fn log(&self, record: &log::Record<'_>) {
            self.records.lock().unwrap().push(format!(
                "{} {}",
                record.level(),
                record.args()
            ));
        }

        fn flush(&self) {}
    }

    #[test]
    fn test_multi_dispatches_to_all_writers() {
        let (w1, records1) = TestWriter::new(log::LevelFilter::Debug);
        let (w2, records2) = TestWriter::new(log::LevelFilter::Info);

        let multi = MultiLog::new()
            .writer(w1)
            .writer(w2)
            .build()
            .unwrap();

        // Log an info record - should go to both
        let record = log::Record::builder()
            .level(log::Level::Info)
            .target("test")
            .args(format_args!("hello"))
            .build();
        multi.log(&record);

        assert_eq!(records1.lock().unwrap().len(), 1);
        assert_eq!(records2.lock().unwrap().len(), 1);

        // Log a debug record - should only go to w1
        let record = log::Record::builder()
            .level(log::Level::Debug)
            .target("test")
            .args(format_args!("debug msg"))
            .build();
        multi.log(&record);

        assert_eq!(records1.lock().unwrap().len(), 2);
        assert_eq!(records2.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_empty_builder_fails() {
        let result = MultiLog::new().build();
        assert!(result.is_err());
    }

    #[test]
    fn test_flush_all_writers() {
        let (w1, _) = TestWriter::new(log::LevelFilter::Info);
        let (w2, _) = TestWriter::new(log::LevelFilter::Info);

        let multi = MultiLog::new()
            .writer(w1)
            .writer(w2)
            .build()
            .unwrap();

        multi.flush(); // Should not panic
    }

    // --- Tests for dynamic per-writer level adjustment ---

    /// A test writer that supports dynamic level adjustment via a handle.
    struct DynamicTestWriter {
        level: Arc<AtomicU8>,
        records: Arc<Mutex<Vec<String>>>,
    }

    impl DynamicTestWriter {
        fn new(level: log::LevelFilter) -> (Self, LogHandle, Arc<Mutex<Vec<String>>>) {
            let level = Arc::new(AtomicU8::new(handle::level_to_u8(level)));
            let records = Arc::new(Mutex::new(Vec::new()));
            let log_handle = LogHandle { level: level.clone() };
            (
                Self {
                    level,
                    records: records.clone(),
                },
                log_handle,
                records,
            )
        }
    }

    impl log::Log for DynamicTestWriter {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level()
                <= handle::u8_to_level(self.level.load(Ordering::Relaxed))
        }

        fn log(&self, record: &log::Record<'_>) {
            self.records.lock().unwrap().push(format!(
                "{} {} (level={:?})",
                record.level(),
                record.args(),
                handle::u8_to_level(self.level.load(Ordering::Relaxed)),
            ));
        }

        fn flush(&self) {}
    }

    #[test]
    fn test_dynamic_level_adjust_per_writer() {
        let (w1, handle1, rec1) = DynamicTestWriter::new(log::LevelFilter::Debug);
        let (w2, handle2, rec2) = DynamicTestWriter::new(log::LevelFilter::Info);

        let multi = MultiLog::new()
            .writer(w1)
            .writer(w2)
            .build()
            .unwrap();

        // Initial state: w1=Debug, w2=Info
        assert_eq!(handle1.get_level(), log::LevelFilter::Debug);
        assert_eq!(handle2.get_level(), log::LevelFilter::Info);

        // Log a debug message — only w1 should capture
        let record = log::Record::builder()
            .level(log::Level::Debug)
            .target("test")
            .args(format_args!("debug message"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 1);
        assert_eq!(rec2.lock().unwrap().len(), 0);

        // Log an info message — both should capture
        let record = log::Record::builder()
            .level(log::Level::Info)
            .target("test")
            .args(format_args!("info message"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 2);
        assert_eq!(rec2.lock().unwrap().len(), 1);

        // Dynamically change w1 to Error, w2 to Trace
        handle1.set_level(log::LevelFilter::Error);
        handle2.set_level(log::LevelFilter::Trace);

        assert_eq!(handle1.get_level(), log::LevelFilter::Error);
        assert_eq!(handle2.get_level(), log::LevelFilter::Trace);

        // Log a warn message — w1 rejects (Error > Warn), w2 accepts
        let record = log::Record::builder()
            .level(log::Level::Warn)
            .target("test")
            .args(format_args!("warn message"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 2); // unchanged
        assert_eq!(rec2.lock().unwrap().len(), 2); // captured

        // Log a trace message — only w2 accepts (Trace)
        let record = log::Record::builder()
            .level(log::Level::Trace)
            .target("test")
            .args(format_args!("trace message"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 2); // unchanged
        assert_eq!(rec2.lock().unwrap().len(), 3); // captured info, warn, trace

        // Log an error message — both accept
        let record = log::Record::builder()
            .level(log::Level::Error)
            .target("test")
            .args(format_args!("error message"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 3); // captured
        assert_eq!(rec2.lock().unwrap().len(), 4); // captured
    }

    #[test]
    fn test_dynamic_level_disable_and_reenable() {
        let (w1, handle1, rec1) = DynamicTestWriter::new(log::LevelFilter::Debug);
        let (w2, _handle2, rec2) = DynamicTestWriter::new(log::LevelFilter::Info);

        let multi = MultiLog::new()
            .writer(w1)
            .writer(w2)
            .build()
            .unwrap();

        // Set w1 to Off — it should capture nothing
        handle1.set_level(log::LevelFilter::Off);
        assert_eq!(handle1.get_level(), log::LevelFilter::Off);

        let record = log::Record::builder()
            .level(log::Level::Error)
            .target("test")
            .args(format_args!("error after off"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 0); // w1 disabled, nothing captured
        assert_eq!(rec2.lock().unwrap().len(), 1); // w2 still captures Error

        // Re-enable w1 at Info level
        handle1.set_level(log::LevelFilter::Info);

        let record = log::Record::builder()
            .level(log::Level::Info)
            .target("test")
            .args(format_args!("info after reenable"))
            .build();
        multi.log(&record);

        assert_eq!(rec1.lock().unwrap().len(), 1); // w1 now captures
        assert_eq!(rec2.lock().unwrap().len(), 2); // w2 still captures
    }

    #[test]
    fn test_dynamic_level_affects_enabled() {
        let (w, handle, _) = DynamicTestWriter::new(log::LevelFilter::Info);

        let multi = MultiLog::new().writer(w).build().unwrap();

        let info_meta = log::MetadataBuilder::new()
            .level(log::Level::Info)
            .target("test")
            .build();
        let trace_meta = log::MetadataBuilder::new()
            .level(log::Level::Trace)
            .target("test")
            .build();

        // Initially Info: Info enabled, Trace disabled
        assert!(multi.enabled(&info_meta));
        assert!(!multi.enabled(&trace_meta));

        // Dynamically change to Debug
        handle.set_level(log::LevelFilter::Debug);

        // Now both Info and Debug are enabled, Trace still disabled
        assert!(multi.enabled(&info_meta));
        assert!(multi.enabled(
            &log::MetadataBuilder::new()
                .level(log::Level::Debug)
                .target("test")
                .build()
        ));
        assert!(!multi.enabled(&trace_meta));

        // Change to Trace — everything enabled
        handle.set_level(log::LevelFilter::Trace);
        assert!(multi.enabled(&info_meta));
        assert!(multi.enabled(&trace_meta));
    }

    #[test]
    fn test_handle_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LogHandle>();
        assert_sync::<LogHandle>();
    }

    #[test]
    fn test_writer_handle_api() {
        // Test the writer_with_handle + build_with_handles + LogHandle API
        let (w1, rec1) = TestWriter::new(log::LevelFilter::Debug);
        let (w2, rec2) = TestWriter::new(log::LevelFilter::Info);

        let (multi, handles) = MultiLog::new()
            .writer_with_handle(w1)
            .writer_with_handle(w2)
            .build_with_handles()
            .unwrap();

        // Should have 2 handles
        assert_eq!(handles.len(), 2);

        // Initially both wrappers are at Trace (all pass), inner filters still apply
        let debug_record = log::Record::builder()
            .level(log::Level::Debug)
            .target("test")
            .args(format_args!("debug"))
            .build();
        multi.log(&debug_record);
        assert_eq!(rec1.lock().unwrap().len(), 1); // w1 inner=Debug, accepts
        assert_eq!(rec2.lock().unwrap().len(), 0); // w2 inner=Info, rejects

        let info_record = log::Record::builder()
            .level(log::Level::Info)
            .target("test")
            .args(format_args!("info"))
            .build();
        multi.log(&info_record);
        assert_eq!(rec1.lock().unwrap().len(), 2);
        assert_eq!(rec2.lock().unwrap().len(), 1);

        // Dynamically change w2's wrapper to Error
        handles[1].set_level(log::LevelFilter::Error);

        // w2 wrapper now only allows Error+, but w2 inner=Info
        // A warn message: w2 wrapper (Error) rejects, w2 inner (Info) would accept
        let warn_record = log::Record::builder()
            .level(log::Level::Warn)
            .target("test")
            .args(format_args!("warn"))
            .build();
        multi.log(&warn_record);
        assert_eq!(rec1.lock().unwrap().len(), 3); // w1 wrapper=Trace, w1 inner=Debug, accepts
        assert_eq!(rec2.lock().unwrap().len(), 1); // w2 wrapper=Error, rejects warn

        // An error message: w2 wrapper (Error) accepts, w2 inner (Info) accepts
        let error_record = log::Record::builder()
            .level(log::Level::Error)
            .target("test")
            .args(format_args!("error"))
            .build();
        multi.log(&error_record);
        assert_eq!(rec1.lock().unwrap().len(), 4);
        assert_eq!(rec2.lock().unwrap().len(), 2);

        // Test LogHandle is Send + Sync
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LogHandle>();
        assert_sync::<LogHandle>();

        // Test clone shares state
        let h1 = handles[0].clone();
        assert_eq!(h1.get_level(), log::LevelFilter::Trace);
        h1.set_level(log::LevelFilter::Warn);
        assert_eq!(handles[0].get_level(), log::LevelFilter::Warn);
    }

    #[test]
    fn test_concurrent_writes_to_multi() {
        let (w1, rec1) = TestWriter::new(log::LevelFilter::Debug);
        let (w2, rec2) = TestWriter::new(log::LevelFilter::Info);

        let multi = Arc::new(
            MultiLog::new()
                .writer(w1)
                .writer(w2)
                .build()
                .unwrap(),
        );

        let mut jhs = Vec::new();
        for i in 0..8 {
            let m = multi.clone();
            jhs.push(std::thread::spawn(move || {
                for j in 0..50 {
                    m.log(
                        &log::Record::builder()
                            .level(log::Level::Info)
                            .target("test")
                            .args(format_args!("thread-{i}-msg-{j}"))
                            .build(),
                    );
                }
                m.flush();
            }));
        }
        for jh in jhs {
            jh.join().unwrap();
        }

        // 8 threads * 50 messages = 400, both writers accept Info
        assert_eq!(rec1.lock().unwrap().len(), 400);
        assert_eq!(rec2.lock().unwrap().len(), 400);
    }

    #[test]
    fn test_concurrent_handle_changes_in_multi() {
        let (w1, rec1) = TestWriter::new(log::LevelFilter::Debug);
        let (w2, rec2) = TestWriter::new(log::LevelFilter::Info);

        let (multi, handles) = MultiLog::new()
            .writer_with_handle(w1)
            .writer_with_handle(w2)
            .build_with_handles()
            .unwrap();

        let multi = Arc::new(multi);
        let handles = Arc::new(handles);

        // Writer threads: log messages at different levels
        let mut wjhs = Vec::new();
        for i in 0..4 {
            let m = multi.clone();
            wjhs.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    m.log(
                        &log::Record::builder()
                            .level(log::Level::Debug)
                            .target("test")
                            .args(format_args!("debug-{}", i))
                            .build(),
                    );
                    m.log(
                        &log::Record::builder()
                            .level(log::Level::Info)
                            .target("test")
                            .args(format_args!("info-{}", i))
                            .build(),
                    );
                    std::thread::yield_now();
                }
                m.flush();
            }));
        }

        // Level changer threads: toggle handles between Debug and Info
        let mut chs = Vec::new();
        for idx in 0..2 {
            let hs = handles.clone();
            chs.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    hs[idx].set_level(log::LevelFilter::Trace);
                    std::thread::yield_now();
                    hs[idx].set_level(log::LevelFilter::Warn);
                    std::thread::yield_now();
                }
            }));
        }

        for jh in wjhs {
            jh.join().unwrap();
        }
        for jh in chs {
            jh.join().unwrap();
        }

        // Just verify no panics and some records were captured
        assert!(!rec1.lock().unwrap().is_empty());
        assert!(!rec2.lock().unwrap().is_empty());
    }
}

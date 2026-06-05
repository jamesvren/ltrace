//! Shared log handle for dynamic level control.
//!
//! This module provides [`LogHandle`] which is used by [`LogWriter`](crate::log_layer::LogWriter),
//! [`ConsoleWriter`](crate::console_writer::ConsoleWriter), and individual writers inside
//! a [`MultiLog`](crate::multi_writer::MultiLog).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

pub(crate) fn level_to_u8(level: log::LevelFilter) -> u8 {
    match level {
        log::LevelFilter::Trace => 0,
        log::LevelFilter::Debug => 1,
        log::LevelFilter::Info => 2,
        log::LevelFilter::Warn => 3,
        log::LevelFilter::Error => 4,
        log::LevelFilter::Off => 5,
    }
}

pub(crate) fn u8_to_level(val: u8) -> log::LevelFilter {
    match val {
        0 => log::LevelFilter::Trace,
        1 => log::LevelFilter::Debug,
        2 => log::LevelFilter::Info,
        3 => log::LevelFilter::Warn,
        4 => log::LevelFilter::Error,
        _ => log::LevelFilter::Off,
    }
}

/// A handle to dynamically change the log level of a writer.
///
/// This handle is backed by an `Arc<AtomicU8>`, so it is `Clone + Send + Sync`.
/// You can clone it and move the clone to other threads for runtime level changes.
///
/// # Example
///
/// ```ignore
/// let handle = LogWriterBuilder::new("app.log")
///     .level(log::LevelFilter::Info)
///     .init()?;
///
/// // Later, in another thread:
/// handle.set_level(log::LevelFilter::Debug);
/// ```
///
/// # Multi-target
///
/// In a [`MultiLog`](crate::multi_writer::MultiLog), each writer gets its own `LogHandle`
/// for independent level control.
#[derive(Clone)]
pub struct LogHandle {
    pub(crate) level: Arc<AtomicU8>,
}

impl LogHandle {
    /// Set the log level.
    pub fn set_level(&self, level: log::LevelFilter) {
        self.level.store(level_to_u8(level), Ordering::Relaxed);
    }

    /// Get the current log level.
    pub fn get_level(&self) -> log::LevelFilter {
        u8_to_level(self.level.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_handle_is_clone_send_sync() {
        let level = Arc::new(AtomicU8::new(level_to_u8(log::LevelFilter::Info)));
        let handle = LogHandle { level: level.clone() };
        let handle2 = handle.clone();

        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LogHandle>();
        assert_sync::<LogHandle>();

        handle2.set_level(log::LevelFilter::Debug);
        assert_eq!(handle.get_level(), log::LevelFilter::Debug);
    }

    #[test]
    fn test_log_handle_concurrent_set_level() {
        let level = Arc::new(AtomicU8::new(level_to_u8(log::LevelFilter::Info)));
        let handle = LogHandle { level };
        let mut jhs = Vec::new();
        let levels = [
            log::LevelFilter::Trace,
            log::LevelFilter::Debug,
            log::LevelFilter::Info,
            log::LevelFilter::Warn,
            log::LevelFilter::Error,
            log::LevelFilter::Off,
        ];
        for i in 0..6 {
            let h = handle.clone();
            jhs.push(std::thread::spawn(move || {
                h.set_level(levels[i]);
            }));
        }
        for jh in jhs {
            jh.join().unwrap();
        }
        // The last writer wins, but the important thing is that we didn't panic or UB
        let final_level = handle.get_level();
        assert!(levels.contains(&final_level));
    }

    #[test]
    fn test_log_handle_concurrent_read_write() {
        let level = Arc::new(AtomicU8::new(level_to_u8(log::LevelFilter::Info)));
        let handle = LogHandle { level };
        let mut jhs = Vec::new();
        let levels = [
            log::LevelFilter::Trace,
            log::LevelFilter::Debug,
            log::LevelFilter::Info,
            log::LevelFilter::Warn,
            log::LevelFilter::Error,
            log::LevelFilter::Off,
        ];
        // Writer threads
        for i in 0..4 {
            let h = handle.clone();
            jhs.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    h.set_level(levels[i % levels.len()]);
                    std::thread::yield_now();
                }
            }));
        }
        // Reader threads
        for _ in 0..4 {
            let h = handle.clone();
            jhs.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let _ = h.get_level();
                    std::thread::yield_now();
                }
            }));
        }
        for jh in jhs {
            jh.join().unwrap();
        }
    }
}

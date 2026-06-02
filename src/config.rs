//! Configuration types for log rotation and compression.

use std::fmt;

use chrono::Timelike;

/// How often to rotate log files based on time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Rotate every minute.
    Minutely,
    /// Rotate every hour.
    Hourly,
    /// Rotate every day at midnight (local time).
    Daily,
    /// Never rotate by time (only by size if configured).
    Never,
}

impl Rotation {
    /// Returns the next rotation instant after `now` for this rotation frequency.
    pub fn next_rotation(&self, now: chrono::DateTime<chrono::FixedOffset>) -> chrono::DateTime<chrono::FixedOffset> {
        match self {
            Rotation::Minutely => {
                let dt = now + chrono::Duration::minutes(1);
                dt.with_second(0).unwrap_or(dt)
            }
            Rotation::Hourly => {
                let dt = now + chrono::Duration::hours(1);
                dt.with_minute(0).unwrap_or(dt).with_second(0).unwrap_or(dt)
            }
            Rotation::Daily => {
                let tomorrow = now.date_naive() + chrono::Days::new(1);
                chrono::DateTime::<chrono::FixedOffset>::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(tomorrow, chrono::NaiveTime::MIN),
                    now.offset().clone(),
                )
            }
            Rotation::Never => chrono::DateTime::<chrono::Utc>::MAX_UTC.into(),
        }
    }

    /// Returns the suffix format string for this rotation type.
    pub fn suffix_format(&self) -> &str {
        match self {
            Rotation::Minutely => "%Y-%m-%d-%H-%M",
            Rotation::Hourly => "%Y-%m-%d-%H",
            Rotation::Daily => "%Y-%m-%d",
            Rotation::Never => "",
        }
    }
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rotation::Minutely => write!(f, "minutely"),
            Rotation::Hourly => write!(f, "hourly"),
            Rotation::Daily => write!(f, "daily"),
            Rotation::Never => write!(f, "never"),
        }
    }
}

/// Compression mode for rotated log files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// Gzip compression.
    #[default]
    Gzip,
    /// No compression.
    None,
}

impl Compression {
    /// Returns the file extension for this compression type.
    pub const fn extension(&self) -> &'static str {
        match self {
            Compression::Gzip => ".gz",
            Compression::None => "",
        }
    }
}

/// Timezone used for rotation timestamps and filenames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timezone {
    /// UTC timezone.
    Utc,
    /// Local system timezone.
    Local,
}

impl Timezone {
    /// Returns the current time in this timezone as a `FixedOffset`.
    pub fn now(self) -> chrono::DateTime<chrono::FixedOffset> {
        match self {
            Timezone::Utc => chrono::Utc::now().into(),
            Timezone::Local => chrono::Local::now().into(),
        }
    }

    /// Returns the timezone offset used for formatting.
    pub fn offset(self) -> chrono::FixedOffset {
        self.now().offset().clone()
    }
}

impl Default for Timezone {
    fn default() -> Self {
        Timezone::Utc
    }
}

/// Configuration for log rotation.
#[derive(Debug, Clone)]
pub struct RotationConfig {
    /// Time-based rotation frequency.
    pub rotation: Rotation,
    /// Maximum size in bytes before triggering a rotation.
    /// `None` means no size-based rotation.
    pub max_file_size: Option<u64>,
    /// Maximum number of archived log files to keep.
    /// `None` means unlimited.
    pub max_files: Option<usize>,
    /// Compression mode for rotated files.
    pub compression: Compression,
    /// Timezone for rotation timestamps and filenames.
    pub timezone: Timezone,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            rotation: Rotation::Daily,
            max_file_size: None,
            max_files: None,
            compression: Compression::None,
            timezone: Timezone::default(),
        }
    }
}

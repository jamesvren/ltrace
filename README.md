# ltrace

> A high-performance rolling file writer for `tracing` and `log` crates with rotation, compression, and dynamic log level support.

[![crates.io](https://img.shields.io/crates/v/ltrace.svg)](https://crates.io/crates/ltrace)
[![docs.rs](https://docs.rs/ltrace/badge.svg)](https://docs.rs/ltrace)
[![License](https://img.shields.io/crates/l/ltrace.svg)](./LICENSE)

## Features

- **Log rotation by time** — MINUTELY, HOURLY, DAILY
- **Log rotation by size** — configurable max file size (independent of time rotation)
- **Time + size combined** — size limits can trigger additional rotations within a time window
- **Gzip compression** — rotated files can be compressed on a background thread (zero blocking)
- **Auto prune** — automatically remove old log files when max count is exceeded
- **Dynamic log level** — change the `log` crate log level at runtime via `LogHandle`
- **Timezone support** — UTC (default) or local timezone for rotation timestamps
- **Auto-create directories** — parent directories created automatically

## Installation

Add `ltrace` to your `Cargo.toml`:

```toml
[dependencies]
ltrace = "0.1"
```

### Features

| Feature      | Default | Description                              |
|------------- |---------|------------------------------------------|
| `log`        | ✅      | Enable `log` crate support                |
| `compression`| ✅      | Enable gzip compression (requires `flate2`) |

To disable default features:

```toml
[dependencies]
ltrace = { version = "0.1", default-features = false, features = ["log"] }
```

## Quick Start

### With `tracing`

Use `RollingWriter` with `tracing_appender::non_blocking()` for non-blocking writes:

```rust
use ltrace::{RollingWriter, Rotation, Compression};

let writer = RollingWriter::builder("/var/log/myapp/app.log")
    .rotation(Rotation::Daily)
    .max_file_size(10 * 1024 * 1024)  // 10 MB
    .max_files(10)
    .compression(Compression::Gzip)
    .build()?;

let (non_blocking, _guard) = tracing_appender::non_blocking(writer);
tracing_subscriber::fmt()
    .with_ansi(false)
    .with_writer(non_blocking)
    .finish()
    .try_init()?;
```

### With `log` crate

Chain `init()` directly on the builder for a clean one-liner:

```rust
use ltrace::log_layer::LogWriterBuilder;
use ltrace::{Rotation, Compression};

let handle = LogWriterBuilder::new("/var/log/myapp/app.log")
    .rotation(Rotation::Daily)
    .max_file_size(10 * 1024 * 1024)  // optional: rotate within the same day if size exceeded
    .max_files(5)
    .compression(Compression::Gzip)
    .level(log::LevelFilter::Info)
    .init()?;
```

## Configuration

### Rotation

```rust
use ltrace::Rotation;

// Time-based rotation
RollingWriter::builder("app.log").rotation(Rotation::Daily).build()?;
RollingWriter::builder("app.log").rotation(Rotation::Hourly).build()?;
RollingWriter::builder("app.log").rotation(Rotation::Minutely).build()?;

// Size-only rotation (no time-based rotation)
RollingWriter::builder("app.log")
    .rotation(Rotation::Never)
    .max_file_size(50 * 1024 * 1024)  // 50 MB
    .build()?;
```

### Compression

```rust
use ltrace::Compression;

RollingWriter::builder("app.log")
    .compression(Compression::Gzip)
    .build()?;
```

> Compression is performed on a background thread and does not block log writes.

### Timezone

```rust
use ltrace::Timezone;

// UTC (default)
RollingWriter::builder("app.log").timezone(Timezone::Utc).build()?;

// Local timezone
RollingWriter::builder("app.log").timezone(Timezone::Local).build()?;
```

### Max File Count

```rust
RollingWriter::builder("app.log")
    .max_files(10)  // Keep at most 10 archived log files
    .build()?;
```

## Dynamic Log Level (log crate only)

`LogHandle` is `Clone + Send + Sync`, so you can move it to other threads:

```rust
let handle = LogWriterBuilder::new("app.log")
    .level(log::LevelFilter::Info)
    .init()?;

// Spawn a thread to listen for signals
let h = handle.clone();
std::thread::spawn(move || {
    // e.g., listen for SIGHUP or an HTTP endpoint
    h.set_level(log::LevelFilter::Debug);
});

// Check current level
println!("Current level: {:?}", handle.get_level());
```

## Rotated File Naming

Rotated filenames use a nanosecond-precision timestamp (nanoseconds since UNIX epoch) to guarantee strict ordering and uniqueness:

| Mode        | Filename Pattern                                         |
|-------------|----------------------------------------------------------|
| Daily       | `app.log.2024-01-15T1780358400000000000`                 |
| Hourly      | `app.log.2024-01-15-14T1780358400000000000`              |
| Minutely    | `app.log.2024-01-15-14-30T1780358400000000000`           |
| Size-only   | `app.log.1780358400000000000`                            |

With gzip compression: `app.log.2024-01-15T1780358400000000000.gz`

## API Overview

| Type / Module | Description |
|---------------|-------------|
| [`RollingWriter`] | Core rolling file writer, implements `std::io::Write` |
| [`Rotation`] | Time-based rotation frequency enum |
| [`Compression`] | Compression mode (None / Gzip) |
| [`Timezone`] | UTC or Local timezone for timestamps |
| [`log_layer::LogWriterBuilder`] | Builder for `log` crate integration |
| [`log_layer::LogWriter`] | Implements `log::Log` for the global logger |
| [`log_layer::LogHandle`] | Cloneable handle for dynamic level changes |

[`RollingWriter`]: https://docs.rs/ltrace/latest/ltrace/struct.RollingWriter.html
[`Rotation`]: https://docs.rs/ltrace/latest/ltrace/enum.Rotation.html
[`Compression`]: https://docs.rs/ltrace/latest/ltrace/enum.Compression.html
[`Timezone`]: https://docs.rs/ltrace/latest/ltrace/enum.Timezone.html
[`log_layer::LogWriterBuilder`]: https://docs.rs/ltrace/latest/ltrace/log_layer/struct.LogWriterBuilder.html
[`log_layer::LogWriter`]: https://docs.rs/ltrace/latest/ltrace/log_layer/struct.LogWriter.html
[`log_layer::LogHandle`]: https://docs.rs/ltrace/latest/ltrace/log_layer/struct.LogHandle.html

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT License](./LICENSE-MIT)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

---

**Author**: James Ren <jamesvren@163.com>
**Repository**: https://github.com/jamesvren/ltrace

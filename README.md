# ltrace

> A high-performance rolling file writer and multi-target logger for `tracing` and `log` crates with rotation, compression, dynamic log levels, console output, and multi-target support.

[![crates.io](https://img.shields.io/crates/v/ltrace.svg)](https://crates.io/crates/ltrace)
[![docs.rs](https://docs.rs/ltrace/badge.svg)](https://docs.rs/ltrace)
[![License](https://img.shields.io/crates/l/ltrace.svg)](./LICENSE)

## Features

- **Log rotation by time** — MINUTELY, HOURLY, DAILY
- **Log rotation by size** — configurable max file size (independent of time rotation)
- **Time + size combined** — size limits can trigger additional rotations within a time window
- **Gzip compression** — rotated files can be compressed on a background thread (zero blocking)
- **Auto prune** — automatically remove old log files when max count is exceeded
- **Dynamic log level** — change the log level at runtime via `LogHandle` (`Clone + Send + Sync`)
- **Console output** — colored terminal logging with ANSI colors (feature `console`)
- **Multi-target logging** — combine console + file, or any number of writers together
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
| `log`        | ✅      | Enable `log` crate support               |
| `compression`| ✅      | Enable gzip compression (requires `flate2`) |
| `console`    | ❌      | Enable colored console output (requires `colored`) |

To enable console support:

```toml
[dependencies]
ltrace = { version = "0.1", features = ["log", "console"] }
```

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
let subscriber = tracing_subscriber::fmt()
    .with_writer(non_blocking)
    .finish()
    .try_init()?;
```

### With `log` crate

Simple initialization with defaults (Daily rotation, Info level):

```rust
use ltrace::LogWriter;

let handle = LogWriter::init("/var/log/myapp/app.log")?;

// Dynamically change level at runtime:
handle.set_level(log::LevelFilter::Debug);
```

Full builder configuration:

```rust
use ltrace::{LogWriter, Rotation, Compression};

let handle = LogWriter::builder("/var/log/myapp/app.log")
    .rotation(Rotation::Daily)
    .max_file_size(10 * 1024 * 1024)  // also rotate within the same day if size exceeded
    .max_files(5)
    .compression(Compression::Gzip)
    .level(log::LevelFilter::Info)
    .init()?;
```

### Console output (feature `console`)

Output colored logs to the terminal:

```rust
use ltrace::ConsoleWriter;

let handle = ConsoleWriter::builder()
    .level(log::LevelFilter::Debug)
    .init()?;
```

Colors by level:
- **ERROR**: bold red
- **WARN**: yellow
- **INFO**: green
- **DEBUG**: cyan
- **TRACE**: dimmed / bright black

Error and warning messages go to stderr; info, debug, and trace go to stdout.

### Multi-target output (console + file)

Combine multiple writers to log to both console and file simultaneously:

```rust
use ltrace::{MultiLog, ConsoleWriter, LogWriter, Rotation, Compression};

let multi = MultiLog::new()
    .writer(ConsoleWriter::builder()
        .level(log::LevelFilter::Info)
        .build()?)
    .writer(LogWriter::builder("/var/log/myapp/app.log")
        .rotation(Rotation::Daily)
        .compression(Compression::Gzip)
        .level(log::LevelFilter::Debug)
        .build()?)
    .init()?;
```

### Per-writer dynamic level control

Adjust each writer's log level independently at runtime:

```rust
use ltrace::{MultiLog, ConsoleWriter, LogWriter};

let (multi, handles) = MultiLog::new()
    .writer_with_handle(ConsoleWriter::builder().build()?)
    .writer_with_handle(LogWriter::builder("app.log").build()?)
    .build_with_handles()?;

// Adjust each writer's level independently:
handles[0].set_level(log::LevelFilter::Warn);   // console: only warn+
handles[1].set_level(log::LevelFilter::Debug);  // file: debug+
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

## Dynamic Log Level

`LogHandle` is `Clone + Send + Sync`, so you can move it to other threads:

```rust
let handle = LogWriter::init("app.log")?;

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

| Type | Description |
|------|-------------|
| [`RollingWriter`] | Core rolling file writer, implements `std::io::Write` |
| [`Rotation`] | Time-based rotation frequency enum |
| [`Compression`] | Compression mode (None / Gzip, requires `compression` feature) |
| [`Timezone`] | UTC or Local timezone for timestamps |
| [`LogWriter`] | Implements `log::Log` for rolling file logging |
| [`LogWriterBuilder`] | Builder for `LogWriter` |
| [`ConsoleWriter`] | Colored terminal logger (requires `console` feature) |
| [`ConsoleWriterBuilder`] | Builder for `ConsoleWriter` |
| [`MultiLog`] | Multi-target log dispatcher, combines multiple writers |
| [`LogHandle`] | Cloneable handle for dynamic level changes |
| [`MultiHandle`] | Handle for multi-log management with per-writer access |

[`RollingWriter`]: https://docs.rs/ltrace/latest/ltrace/struct.RollingWriter.html
[`Rotation`]: https://docs.rs/ltrace/latest/ltrace/enum.Rotation.html
[`Compression`]: https://docs.rs/ltrace/latest/ltrace/enum.Compression.html
[`Timezone`]: https://docs.rs/ltrace/latest/ltrace/enum.Timezone.html
[`LogWriter`]: https://docs.rs/ltrace/latest/ltrace/struct.LogWriter.html
[`LogWriterBuilder`]: https://docs.rs/ltrace/latest/ltrace/struct.LogWriterBuilder.html
[`ConsoleWriter`]: https://docs.rs/ltrace/latest/ltrace/struct.ConsoleWriter.html
[`ConsoleWriterBuilder`]: https://docs.rs/ltrace/latest/ltrace/struct.ConsoleWriterBuilder.html
[`MultiLog`]: https://docs.rs/ltrace/latest/ltrace/struct.MultiLog.html
[`LogHandle`]: https://docs.rs/ltrace/latest/ltrace/struct.LogHandle.html
[`MultiHandle`]: https://docs.rs/ltrace/latest/ltrace/struct.MultiHandle.html

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

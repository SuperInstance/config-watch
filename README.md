# Config Watch — Filesystem Polling for Dynamic Configuration Reload

**Config watch** is a polling-based file watcher that monitors configuration files for modifications, deletions, and creations. When a watched file's content changes (detected by mtime + hash comparison), it fires registered callbacks — enabling live configuration reload without restarting the application.

## Why It Matters

Hot-reloading configuration is a requirement for any service that can't afford downtime. Feature flags, rate limits, routing tables, and log levels all need to change while the process is running. This crate provides the detection layer: it watches files and tells you *when* they change — you decide *what* to do in response. The polling approach (rather than inotify/fsevents) ensures cross-platform compatibility and avoids edge cases with network filesystems, container bind-mounts, and NFS shares where kernel notifications are unreliable. Tools like Viper (Go), node-config (Node.js), and dotenv all use similar polling strategies.

## How It Works

### Polling Loop

Each `poll()` call compares every watched file's current state against its last-known state:

```
For each watched file:
  1. stat() the file to get mtime
  2. If mtime changed:
     a. Read file content
     b. Compute FNV-1a hash of content
     c. If hash differs from stored hash:
        → Emit Modified event
        → Update stored mtime + hash
  3. If file no longer exists (ENOENT):
     → Emit Deleted event
```

The dual check (mtime + content hash) avoids false positives from `touch` commands that update mtime without changing content.

### FNV-1a Hash

Content changes are detected via FNV-1a (Fowler-Noll-Vo), a fast non-cryptographic hash:

```
hash = 0xcbf29ce484222325  (offset basis)
for each byte b:
    hash = hash XOR b
    hash = hash × 0x100000001b3  (FNV prime)
```

FNV-1a is chosen over cryptographic hashes (SHA-256, BLAKE3) because it's `O(n)` with a tiny constant and no initialization overhead — config files are small, and we only need collision resistance for change detection, not security.

### Event Lifecycle

```
File exists    → Modified (when content changes)
File deleted   → Deleted
File created   → Detected on next poll (first time seen → implicitly Modified)
```

**Complexity**: Each `poll()` is `O(F × S)` where `F` = number of watched files and `S` = average file size for hashing. For typical config files (< 100 KB) and watch counts (< 50), each poll cycle is sub-millisecond.

### Thread-Safe Wrapper

`SharedConfigWatcher` wraps `ConfigWatcher` in `Arc<Mutex<>>` for use from multiple threads:

```rust
let shared = SharedConfigWatcher::new(watcher);
// Lock, poll, unlock — from any thread
```

## Quick Start

```rust
use config_watch::{ConfigWatcher, ChangeKind, ChangeCallback};
use std::time::Duration;
use std::path::PathBuf;

let mut watcher = ConfigWatcher::default_interval();

// Register callbacks
watcher.on_change(Box::new(|event| {
    println!("Config changed: {:?} ({})", event.path, event.kind);
}));

// Watch a config file
watcher.watch("config.toml").ok();

// Run for 60 seconds
let changes = watcher.run_for(Duration::from_secs(60));
println!("Detected {} changes", changes.len());
```

## API

| Type / Method | Description |
|---|---|
| `ConfigWatcher::new(interval)` | Create with a specific poll interval. |
| `ConfigWatcher::default_interval()` | Create with 1-second polling. |
| `watch(path)` | Add a file to the watch set. |
| `on_change(callback)` | Register a callback for `ConfigChangeEvent`s. |
| `poll()` | One detection cycle → `Vec<ConfigChangeEvent>`. |
| `run_for(duration)` | Blocking poll loop for a fixed duration. |
| `watched_paths()` | All registered paths. |
| `ConfigChangeEvent` | `{ path: PathBuf, kind: ChangeKind }`. |
| `ChangeKind` | `Created`, `Modified`, `Deleted`. |
| `SharedConfigWatcher` | Thread-safe `Arc<Mutex<ConfigWatcher>>` wrapper. |

## Architecture Notes

Config watch is part of the γ (generation/runtime) side of γ + η = C in SuperInstance. It provides the hot-reload capability that lets fleet instances pick up configuration changes — feature flag toggles, routing updates, threshold adjustments — without rolling restarts. See [SuperInstance Architecture](https://github.com/SuperInstance/SuperInstance/blob/main/ARCHITECTURE.md).

## References

1. Fowler, M. (2018). *Refactoring* (2nd ed.), "Configuration Parameters". Addison-Wesley. — Why externalized config matters.
2. inotify(7) Linux man page. <https://man7.org/linux/man-pages/man7/inotify.7.html> — Kernel-level alternative.
3. Noll, L. C. (1994). *FNV Hash*. <http://www.isthe.com/chongo/tech/comp/fnv/> — The FNV-1a hash algorithm.

## License

MIT

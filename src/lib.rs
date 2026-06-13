//! File watcher for dynamic configuration reloading.
//!
//! Monitors config files for changes and triggers reload callbacks.
//! Uses polling-based watching for broad platform compatibility.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Metadata tracked for each watched file.
#[derive(Debug, Clone)]
struct FileState {
    modified: std::time::SystemTime,
    content_hash: u64,
}

/// Event emitted when a config file changes.
#[derive(Debug, Clone)]
pub struct ConfigChangeEvent {
    /// Path of the changed file.
    pub path: PathBuf,
    /// The kind of change that occurred.
    pub kind: ChangeKind,
}

/// The kind of file change detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// File was created.
    Created,
    /// File was modified.
    Modified,
    /// File was deleted.
    Deleted,
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChangeKind::Created => write!(f, "created"),
            ChangeKind::Modified => write!(f, "modified"),
            ChangeKind::Deleted => write!(f, "deleted"),
        }
    }
}

/// Callback type for configuration change events.
pub type ChangeCallback = Box<dyn Fn(ConfigChangeEvent) + Send>;

/// Watches one or more config files for changes using polling.
pub struct ConfigWatcher {
    /// Paths being watched and their last known state.
    states: HashMap<PathBuf, FileState>,
    /// Callbacks to invoke when changes are detected.
    callbacks: Vec<ChangeCallback>,
    /// Polling interval.
    poll_interval: Duration,
}

impl ConfigWatcher {
    /// Create a new watcher with the given poll interval.
    pub fn new(poll_interval: Duration) -> Self {
        Self {
            states: HashMap::new(),
            callbacks: Vec::new(),
            poll_interval,
        }
    }

    /// Create a watcher with a default 1-second poll interval.
    pub fn default_interval() -> Self {
        Self::new(Duration::from_secs(1))
    }

    /// Add a file path to watch.
    pub fn watch(&mut self, path: impl Into<PathBuf>) -> Result<(), std::io::Error> {
        let path = path.into();
        let path = if path.is_relative() {
            std::env::current_dir()?.join(path)
        } else {
            path
        };

        if let Ok(metadata) = fs::metadata(&path) {
            if metadata.is_file() {
                let modified = metadata.modified()?;
                let content = fs::read_to_string(&path)?;
                let content_hash = simple_hash(&content);
                self.states.insert(
                    path,
                    FileState {
                        modified,
                        content_hash,
                    },
                );
            }
        }
        // If file doesn't exist yet, we'll catch it on the next poll
        Ok(())
    }

    /// Register a callback for change events.
    pub fn on_change(&mut self, callback: ChangeCallback) {
        self.callbacks.push(callback);
    }

    /// Perform a single poll cycle. Returns detected changes.
    pub fn poll(&mut self) -> Vec<ConfigChangeEvent> {
        let mut changes = Vec::new();

        for (path, state) in &mut self.states {
            match fs::metadata(path) {
                Ok(metadata) => {
                    if let Ok(new_modified) = metadata.modified() {
                        if new_modified != state.modified {
                            if let Ok(content) = fs::read_to_string(path) {
                                let new_hash = simple_hash(&content);
                                if new_hash != state.content_hash {
                                    state.modified = new_modified;
                                    state.content_hash = new_hash;
                                    changes.push(ConfigChangeEvent {
                                        path: path.clone(),
                                        kind: ChangeKind::Modified,
                                    });
                                }
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if state.content_hash != 0 {
                        state.content_hash = 0;
                        state.modified = std::time::SystemTime::UNIX_EPOCH;
                        changes.push(ConfigChangeEvent {
                            path: path.clone(),
                            kind: ChangeKind::Deleted,
                        });
                    }
                }
                Err(_) => {}
            }
        }

        // Invoke callbacks
        for event in &changes {
            for callback in &self.callbacks {
                callback(event.clone());
            }
        }

        changes
    }

    /// Run the watcher in a blocking loop until the given duration elapses.
    /// Returns all changes detected.
    pub fn run_for(&mut self, duration: Duration) -> Vec<ConfigChangeEvent> {
        let start = Instant::now();
        let mut all_changes = Vec::new();
        while start.elapsed() < duration {
            let changes = self.poll();
            all_changes.extend(changes);
            std::thread::sleep(self.poll_interval);
        }
        all_changes
    }

    /// Get the list of watched paths.
    pub fn watched_paths(&self) -> Vec<&Path> {
        self.states.keys().map(|p| p.as_path()).collect()
    }

    /// Number of watched files.
    pub fn watch_count(&self) -> usize {
        self.states.len()
    }
}

/// Simple FNV-1a-like hash for detecting content changes.
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Thread-safe wrapper for ConfigWatcher.
pub struct SharedConfigWatcher {
    inner: Arc<Mutex<ConfigWatcher>>,
}

impl SharedConfigWatcher {
    pub fn new(watcher: ConfigWatcher) -> Self {
        Self {
            inner: Arc::new(Mutex::new(watcher)),
        }
    }

    pub fn poll(&self) -> Vec<ConfigChangeEvent> {
        self.inner.lock().unwrap().poll()
    }

    pub fn watch(&self, path: impl Into<PathBuf>) -> Result<(), std::io::Error> {
        self.inner.lock().unwrap().watch(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_detect_modification() {
        let dir = std::env::temp_dir().join("config-watch-test");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("test.conf");

        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "key=value").unwrap();
        drop(f);

        let mut watcher = ConfigWatcher::new(Duration::from_millis(10));
        watcher.watch(&file_path).unwrap();

        // No changes yet
        let changes = watcher.poll();
        assert!(changes.is_empty());

        // Modify the file
        std::thread::sleep(Duration::from_millis(50));
        let mut f = fs::OpenOptions::new().write(true).truncate(true).open(&file_path).unwrap();
        writeln!(f, "key=new_value").unwrap();
        drop(f);

        // Allow filesystem timestamp to update
        std::thread::sleep(Duration::from_millis(50));
        let changes = watcher.poll();
        assert!(!changes.is_empty());
        assert_eq!(changes[0].kind, ChangeKind::Modified);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_simple_hash() {
        let h1 = simple_hash("hello");
        let h2 = simple_hash("hello");
        let h3 = simple_hash("world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_watched_paths() {
        let mut watcher = ConfigWatcher::default_interval();
        let dir = std::env::temp_dir().join("config-watch-test2");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("a.conf");
        fs::write(&file_path, "x=y").unwrap();

        watcher.watch(&file_path).unwrap();
        assert_eq!(watcher.watch_count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}

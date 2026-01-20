#![allow(dead_code)]

use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, SystemTime};

/// Event received from hook watcher
#[derive(Debug, Clone)]
pub enum WatcherEvent {
    /// Claude finished responding (Stop hook)
    ClaudeStopped {
        session_id: String,
        project_dir: PathBuf,
        source: String,
    },
    /// Session ended (SessionEnd hook)
    SessionEnded {
        session_id: String,
        project_dir: PathBuf,
        reason: String,
        source: String,
    },
    /// Claude needs work/input (Notification hook - permission_prompt or idle_prompt)
    NeedsWork {
        session_id: String,
        project_dir: PathBuf,
        input_type: String,
        source: String,
    },
    /// User provided input (UserPromptSubmit hook)
    InputProvided {
        session_id: String,
        project_dir: PathBuf,
        source: String,
    },
    /// Claude is working/using a tool (PreToolUse hook)
    Working {
        session_id: String,
        project_dir: PathBuf,
        source: String,
    },
    /// Error occurred
    Error(String),
}

/// Signal file format written by hook scripts
#[derive(Debug, Serialize, Deserialize)]
pub struct HookSignalFile {
    pub event: String,
    pub session_id: String,
    pub project_dir: PathBuf,
    pub timestamp: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub input_type: String,
    /// Source of the signal: "sdk" or "cli" (defaults to "cli" for backwards compatibility)
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "cli".to_string()
}

/// Watches the signal directory for hook notifications
pub struct HookWatcher {
    signal_dir: PathBuf,
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    /// Track processed signal filenames to avoid re-processing
    processed_signals: HashSet<String>,
    /// Last cleanup time
    last_cleanup: std::time::Instant,
}

/// How long to keep signal files before cleanup (24 hours)
/// Long TTL allows TUI instances that were offline to sync when they restart
const SIGNAL_TTL_SECS: u64 = 24 * 60 * 60;

impl HookWatcher {
    /// Create a new hook watcher
    pub fn new() -> Result<Self> {
        let signal_dir = get_signal_dir()?;
        std::fs::create_dir_all(&signal_dir)?;

        let (tx, rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default().with_poll_interval(Duration::from_millis(100)),
        )?;

        watcher.watch(&signal_dir, RecursiveMode::NonRecursive)?;

        Ok(Self {
            signal_dir,
            _watcher: watcher,
            receiver: rx,
            processed_signals: HashSet::new(),
            last_cleanup: std::time::Instant::now(),
        })
    }

    /// Check for new events (non-blocking)
    pub fn poll(&mut self) -> Option<WatcherEvent> {
        // Periodic cleanup of old signals (every 30 seconds)
        if self.last_cleanup.elapsed() > Duration::from_secs(30) {
            self.cleanup_old_signals();
            self.last_cleanup = std::time::Instant::now();
        }

        match self.receiver.try_recv() {
            Ok(Ok(event)) => self.process_event(event),
            Ok(Err(e)) => Some(WatcherEvent::Error(e.to_string())),
            Err(_) => None,
        }
    }

    /// Process a file system event
    fn process_event(&mut self, event: Event) -> Option<WatcherEvent> {
        // Only process create events
        if !matches!(event.kind, EventKind::Create(_)) {
            return None;
        }

        // Read and process signal files
        for path in event.paths {
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let filename = path.file_name()?.to_string_lossy().to_string();

                // Skip if already processed
                if self.processed_signals.contains(&filename) {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(signal) = serde_json::from_str::<HookSignalFile>(&content) {
                        // Mark as processed (don't delete - other instances may need it)
                        self.processed_signals.insert(filename);

                        return match signal.event.as_str() {
                            "stop" => Some(WatcherEvent::ClaudeStopped {
                                session_id: signal.session_id,
                                project_dir: signal.project_dir,
                                source: signal.source,
                            }),
                            "end" => Some(WatcherEvent::SessionEnded {
                                session_id: signal.session_id,
                                project_dir: signal.project_dir,
                                reason: signal.reason,
                                source: signal.source,
                            }),
                            "needs-input" => Some(WatcherEvent::NeedsWork {
                                session_id: signal.session_id,
                                project_dir: signal.project_dir,
                                input_type: signal.input_type,
                                source: signal.source,
                            }),
                            "input-provided" => Some(WatcherEvent::InputProvided {
                                session_id: signal.session_id,
                                project_dir: signal.project_dir,
                                source: signal.source,
                            }),
                            "working" => Some(WatcherEvent::Working {
                                session_id: signal.session_id,
                                project_dir: signal.project_dir,
                                source: signal.source,
                            }),
                            _ => None,
                        };
                    }
                }
            }
        }

        None
    }

    /// Get the signal directory path
    pub fn signal_dir(&self) -> &PathBuf {
        &self.signal_dir
    }

    /// Process all existing signal files in the directory
    /// Call this on startup to catch signals written while app was not running
    /// Signals are processed in chronological order (oldest first)
    pub fn process_all_pending(&mut self) -> Vec<WatcherEvent> {
        let mut events = Vec::new();

        let entries = match std::fs::read_dir(&self.signal_dir) {
            Ok(entries) => entries,
            Err(_) => return events,
        };

        // Collect and sort signal files by timestamp (extracted from filename)
        // Filename format: signal-{event}-{timestamp_millis}.json
        let mut signal_files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "json")
                    .unwrap_or(false)
            })
            .collect();

        // Sort by timestamp extracted from filename (last component before .json)
        signal_files.sort_by_key(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Extract timestamp from "signal-{event}-{timestamp}.json"
            name.strip_suffix(".json")
                .and_then(|s| s.rsplit('-').next())
                .and_then(|ts| ts.parse::<i64>().ok())
                .unwrap_or(0)
        });

        for entry in signal_files {
            let path = entry.path();
            let filename = entry.file_name().to_string_lossy().to_string();

            // Skip if already processed
            if self.processed_signals.contains(&filename) {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(signal) = serde_json::from_str::<HookSignalFile>(&content) {
                    // Mark as processed (don't delete - other instances may need it)
                    self.processed_signals.insert(filename);

                    let event = match signal.event.as_str() {
                        "stop" => Some(WatcherEvent::ClaudeStopped {
                            session_id: signal.session_id,
                            project_dir: signal.project_dir,
                            source: signal.source,
                        }),
                        "end" => Some(WatcherEvent::SessionEnded {
                            session_id: signal.session_id,
                            project_dir: signal.project_dir,
                            reason: signal.reason,
                            source: signal.source,
                        }),
                        "needs-input" => Some(WatcherEvent::NeedsWork {
                            session_id: signal.session_id,
                            project_dir: signal.project_dir,
                            input_type: signal.input_type,
                            source: signal.source,
                        }),
                        "input-provided" => Some(WatcherEvent::InputProvided {
                            session_id: signal.session_id,
                            project_dir: signal.project_dir,
                            source: signal.source,
                        }),
                        "working" => Some(WatcherEvent::Working {
                            session_id: signal.session_id,
                            project_dir: signal.project_dir,
                            source: signal.source,
                        }),
                        _ => None,
                    };

                    if let Some(e) = event {
                        events.push(e);
                    }
                } else {
                    // Invalid JSON - mark as processed so we don't retry
                    self.processed_signals.insert(filename);
                }
            }
        }

        events
    }

    /// Clean up signal files older than SIGNAL_TTL_SECS
    /// This allows multiple TUI instances to read signals before deletion
    pub fn cleanup_old_signals(&mut self) {
        let entries = match std::fs::read_dir(&self.signal_dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let now = SystemTime::now();
        let ttl = Duration::from_secs(SIGNAL_TTL_SECS);

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // Check file age
                if let Ok(metadata) = path.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age > ttl {
                                // Old signal - safe to delete
                                let _ = std::fs::remove_file(&path);
                                // Also remove from processed set to avoid memory growth
                                if let Some(filename) = path.file_name() {
                                    self.processed_signals.remove(&filename.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Get the signal directory path
pub fn get_signal_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    Ok(home.join(".kanblam").join("signals"))
}

/// Write a signal file (called by hook script via CLI)
/// Automatically detects SDK vs CLI source based on KANBLAM_SDK_SESSION env var
pub fn write_signal(event: &str, session_id: &str, project_dir: &PathBuf, input_type: Option<&str>) -> Result<()> {
    let signal_dir = get_signal_dir()?;
    std::fs::create_dir_all(&signal_dir)?;

    // Detect source: if KANBLAM_SDK_SESSION=1 is set, this is an SDK-driven session
    let source = if std::env::var("KANBLAM_SDK_SESSION").map(|v| v == "1").unwrap_or(false) {
        "sdk"
    } else {
        "cli"
    };

    let signal = HookSignalFile {
        event: event.to_string(),
        session_id: session_id.to_string(),
        project_dir: project_dir.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        reason: String::new(),
        input_type: input_type.unwrap_or("").to_string(),
        source: source.to_string(),
    };

    let filename = format!("signal-{}-{}.json", event, chrono::Utc::now().timestamp_millis());
    let path = signal_dir.join(filename);

    let content = serde_json::to_string_pretty(&signal)?;
    std::fs::write(path, content)?;

    Ok(())
}

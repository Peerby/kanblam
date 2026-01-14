mod installer;
mod watcher;

pub use installer::{hooks_installed, install_hooks};
pub use watcher::{write_signal, HookWatcher, WatcherEvent};

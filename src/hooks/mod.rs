mod installer;
mod watcher;

pub use installer::install_hooks;
pub use watcher::{write_signal, HookWatcher, WatcherEvent};

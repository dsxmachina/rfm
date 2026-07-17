use std::{collections::VecDeque, sync::Arc, time::Instant};

use log::Level;
use parking_lot::Mutex;
use tokio::sync::Notify;

/// Number of log lines retained for debugging (error.log, debug socket),
/// independent of the small display buffer shown in the log widget.
pub const HISTORY_CAPACITY: usize = 200;

#[derive(Clone)]
pub struct LogBuffer {
    buffer: Arc<Mutex<VecDeque<(Level, String)>>>,
    /// Retention ring for debugging: unlike `buffer`, entries are never
    /// evicted by the periodic display cleanup, only by capacity.
    history: Arc<Mutex<VecDeque<(Level, Instant, String)>>>,
    notify: Arc<Notify>,
    capacity: usize,
    level: Level,
}

impl LogBuffer {
    pub fn with_level(self, level: Level) -> Self {
        Self { level, ..self }
    }

    pub fn with_capacity(self, capacity: usize) -> Self {
        Self { capacity, ..self }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn get(&self) -> VecDeque<(Level, String)> {
        self.buffer.lock().clone()
    }

    pub fn get_errors(&self) -> Vec<String> {
        self.history
            .lock()
            .iter()
            .filter(|(level, _, _)| *level == Level::Error)
            .map(|(_, _, msg)| msg)
            .cloned()
            .collect()
    }

    /// Returns the newest `count` retained log lines, oldest first.
    pub fn history(&self, count: usize) -> Vec<(Level, Instant, String)> {
        let history = self.history.lock();
        let skip = history.len().saturating_sub(count);
        history.iter().skip(skip).cloned().collect()
    }

    /// Removes the oldest log line
    pub fn remove_oldest(&self) {
        let mut buffer = self.buffer.lock();
        buffer.pop_front();
    }

    pub async fn update(&self) {
        self.notify.notified().await
    }
}

impl log::Log for LogBuffer {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        // Verbose levels are only interesting for our own code; at trace
        // level dependencies like mio/inotify would flood the history
        if record.level() > Level::Info && !record.target().starts_with("rfm") {
            return;
        }
        let line = format!("{}", record.args());
        let mut history = self.history.lock();
        history.push_back((record.level(), Instant::now(), line.clone()));
        if history.len() > HISTORY_CAPACITY {
            history.pop_front();
        }
        drop(history);
        // The display buffer (log widget) only shows Info and above; debug
        // and trace detail is retained in the history for the debug socket
        if record.level() <= Level::Info {
            let mut inner = self.buffer.lock();
            inner.push_back((record.level(), line));
            if inner.len() > self.capacity {
                inner.pop_front();
            }
            drop(inner);
            self.notify.notify_one();
        }
    }

    fn flush(&self) {}
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            buffer: Default::default(),
            history: Default::default(),
            notify: Default::default(),
            capacity: 10,
            level: Level::Info,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;

    fn log_line(buffer: &LogBuffer, level: Level, msg: &str) {
        log_line_from(buffer, level, msg, "rfm::panel::manager");
    }

    fn log_line_from(buffer: &LogBuffer, level: Level, msg: &str, target: &str) {
        buffer.log(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{}", msg))
                .build(),
        );
    }

    #[test]
    fn verbose_lines_from_foreign_crates_are_dropped() {
        // At trace level, dependencies (mio, inotify, ...) would flood the
        // history and drown rfm's own signal — only rfm targets may pass
        let buffer = LogBuffer::default().with_level(Level::Trace);
        log_line_from(&buffer, Level::Trace, "poller detail", "mio::poll");
        log_line_from(&buffer, Level::Debug, "inotify detail", "inotify");
        log_line_from(&buffer, Level::Info, "foreign info", "mio::poll");
        log_line_from(&buffer, Level::Trace, "own detail", "rfm::panel::manager");

        let history: Vec<String> = buffer
            .history(10)
            .into_iter()
            .map(|(_, _, msg)| msg)
            .collect();
        assert_eq!(
            history,
            vec!["foreign info".to_string(), "own detail".to_string()]
        );
    }

    #[test]
    fn history_survives_display_eviction() {
        let buffer = LogBuffer::default().with_capacity(10);
        log_line(&buffer, Level::Info, "first");
        log_line(&buffer, Level::Error, "second");
        log_line(&buffer, Level::Warn, "third");

        // The periodic task evicts from the display buffer...
        buffer.remove_oldest();
        buffer.remove_oldest();
        buffer.remove_oldest();
        assert!(buffer.get().is_empty(), "display buffer should be drained");

        // ...but the history keeps everything
        let history = buffer.history(10);
        let messages: Vec<&str> = history.iter().map(|(_, _, msg)| msg.as_str()).collect();
        assert_eq!(messages, vec!["first", "second", "third"]);
    }

    #[test]
    fn history_returns_newest_entries_up_to_count() {
        let buffer = LogBuffer::default();
        for i in 0..5 {
            log_line(&buffer, Level::Info, &format!("line-{i}"));
        }
        let history = buffer.history(2);
        let messages: Vec<&str> = history.iter().map(|(_, _, msg)| msg.as_str()).collect();
        assert_eq!(messages, vec!["line-3", "line-4"]);
    }

    #[test]
    fn history_is_capped() {
        let buffer = LogBuffer::default();
        for i in 0..(HISTORY_CAPACITY + 20) {
            log_line(&buffer, Level::Info, &format!("line-{i}"));
        }
        let history = buffer.history(usize::MAX);
        assert_eq!(history.len(), HISTORY_CAPACITY);
        // Oldest entries were dropped
        assert_eq!(history[0].2, "line-20");
    }

    #[test]
    fn verbose_lines_go_to_history_only() {
        // In trace mode (debug socket active) the widget must stay calm:
        // debug/trace lines are retained for the socket but never displayed.
        let buffer = LogBuffer::default().with_level(Level::Trace);
        log_line(&buffer, Level::Trace, "trace-detail");
        log_line(&buffer, Level::Debug, "debug-detail");
        log_line(&buffer, Level::Info, "user-visible");

        let displayed: Vec<String> = buffer.get().into_iter().map(|(_, msg)| msg).collect();
        assert_eq!(displayed, vec!["user-visible".to_string()]);

        let history: Vec<String> = buffer
            .history(10)
            .into_iter()
            .map(|(_, _, msg)| msg)
            .collect();
        assert_eq!(
            history,
            vec![
                "trace-detail".to_string(),
                "debug-detail".to_string(),
                "user-visible".to_string()
            ]
        );
    }

    #[test]
    fn get_errors_reads_history_not_display_buffer() {
        let buffer = LogBuffer::default();
        log_line(&buffer, Level::Error, "important failure");
        buffer.remove_oldest();
        assert!(buffer.get().is_empty());
        assert_eq!(buffer.get_errors(), vec!["important failure".to_string()]);
    }
}

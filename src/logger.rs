use std::{collections::VecDeque, sync::Arc};

use log::Level;
use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct LogBuffer {
    buffer: Arc<Mutex<VecDeque<(Level, String)>>>,
    notify: Arc<Notify>,
    capacity: usize,
    level: Level,
}

impl LogBuffer {
    pub fn with_level(self, level: Level) -> Self {
        Self {
            buffer: self.buffer,
            notify: self.notify,
            capacity: self.capacity,
            level,
        }
    }

    pub fn with_capacity(self, capacity: usize) -> Self {
        Self {
            buffer: self.buffer,
            notify: self.notify,
            capacity,
            level: self.level,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn get(&self) -> VecDeque<(Level, String)> {
        self.buffer.lock().clone()
    }

    pub fn get_errors(&self) -> Vec<String> {
        self.buffer
            .lock()
            .iter()
            .filter(|(level, _)| *level == Level::Error)
            .map(|(_, msg)| msg)
            .cloned()
            .collect()
    }

    /// Removes the oldest log line
    pub fn remove_oldest(&self) {
        let mut buffer = self.buffer.lock();
        buffer.pop_front();
    }

    pub async fn update(&self) {
        self.notify.notified().await
    }

    /// Writes the current log buffer to stdout - useful for debugging after cleanup
    pub fn write_to_stdout(&self) {
        for (level, line) in self.buffer.lock().iter() {
            println!("[{level}]: {line}")
        }
    }
}

impl log::Log for LogBuffer {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        let line = format!("{}", record.args());
        let mut inner = self.buffer.lock();
        inner.push_back((record.level(), line));
        if inner.len() > self.capacity {
            inner.pop_front();
        }
        drop(inner);
        self.notify.notify_one();
    }

    fn flush(&self) {}
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            buffer: Default::default(),
            notify: Default::default(),
            capacity: 10,
            level: Level::Info,
        }
    }
}

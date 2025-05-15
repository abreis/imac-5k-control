//! An in-memory log storage, with a fixed size for records.
#![allow(dead_code)]

use core::fmt::Display;

use alloc::{
    collections::vec_deque::{self, VecDeque},
    string::String,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    mutex::{MappedMutexGuard, Mutex, MutexGuard},
};
use embassy_time::Instant;

static GLOBAL_LOGGER: Mutex<CriticalSectionRawMutex, LogStorage> =
    Mutex::new(LogStorage::with_capacity(DISCARD_ERROR.len()));

struct LogStorage {
    records: VecDeque<Record>,
    // In characters.
    utilization: usize,
    capacity: usize,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub instant: Instant,
    pub level: Level,
    pub text: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Display for Level {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Level::Trace => write!(f, "{}", "TRCE"),
            Level::Debug => write!(f, "{}", "DEBG"),
            Level::Info => write!(f, "{}", "INFO"),
            Level::Warn => write!(f, "{}", "WARN"),
            Level::Error => write!(f, "{}", "ERRO"),
        }
    }
}

const DISCARD_ERROR: &str = "log discarded: too large for storage";

impl LogStorage {
    const fn with_capacity(capacity: usize) -> Self {
        LogStorage {
            records: VecDeque::new(),
            utilization: 0,
            capacity,
        }
    }

    fn add_record(&mut self, level: Level, text: impl Into<String>) {
        let text: String = text.into();

        // Can't fit this record in storage. Log a warning.
        if text.len() > self.capacity {
            self.add_record(Level::Warn, DISCARD_ERROR);
            return;
        }

        // At this point we know we have enough capacity (even if all existing
        // records need to be removed), so we can safely use unwraps.

        // Pop existing records until we have enough space for the new record.
        while (self.capacity - self.utilization) < text.len() {
            let removed = self.records.pop_back().unwrap();
            self.utilization -= removed.text.len();
        }

        // Store the new record.
        self.utilization += text.len();
        self.records.push_front(Record {
            instant: Instant::now(),
            level,
            text,
        });
    }

    fn clear(&mut self) {
        self.utilization = 0;
        self.records.clear();
    }

    fn iter(&self) -> vec_deque::Iter<Record> {
        self.records.iter()
    }
}

pub async fn init(capacity: usize) {
    // Ensure we have enough space to store the error about not having enough space.
    if capacity < DISCARD_ERROR.len() {
        panic!("minimum log storage capacity is {}", DISCARD_ERROR.len());
    }

    let storage = LogStorage::with_capacity(capacity);

    *GLOBAL_LOGGER.lock().await = storage;
}

pub async fn trace(text: impl Into<String>) {
    GLOBAL_LOGGER.lock().await.add_record(Level::Trace, text);
}
pub async fn debug(text: impl Into<String>) {
    GLOBAL_LOGGER.lock().await.add_record(Level::Debug, text);
}
pub async fn info(text: impl Into<String>) {
    GLOBAL_LOGGER.lock().await.add_record(Level::Info, text);
}
pub async fn warn(text: impl Into<String>) {
    GLOBAL_LOGGER.lock().await.add_record(Level::Warn, text);
}
pub async fn error(text: impl Into<String>) {
    GLOBAL_LOGGER.lock().await.add_record(Level::Error, text);
}
pub async fn clear() {
    GLOBAL_LOGGER.lock().await.clear();
}
pub async fn records() -> MappedMutexGuard<'static, CriticalSectionRawMutex, VecDeque<Record>> {
    let guard = GLOBAL_LOGGER.lock().await;
    MutexGuard::map(guard, |storage| &mut storage.records)
}

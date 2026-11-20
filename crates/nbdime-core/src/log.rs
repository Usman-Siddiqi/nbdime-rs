//! Log records that nbdime emits through `nbdime.log` while merging.
//!
//! Upstream prints these on stderr as `[W strategies:468] message`, so each record carries the
//! module and line number of the upstream call. The engine collects the records in the order they
//! are emitted; the caller decides what to print and takes them with [`take`].

use std::cell::RefCell;

/// Python's `logging` levels that nbdime uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 10,
    Info = 20,
    Warning = 30,
    Error = 40,
    Critical = 50,
}

impl Level {
    /// The first letter of the level name, as `%(levelname)1.1s` prints it.
    pub fn initial(self) -> char {
        match self {
            Level::Debug => 'D',
            Level::Info => 'I',
            Level::Warning => 'W',
            Level::Error => 'E',
            Level::Critical => 'C',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub level: Level,
    /// The upstream module (file name without extension) that logs the message.
    pub module: &'static str,
    /// The line of the logging call upstream.
    pub line: u32,
    pub message: String,
}

thread_local! {
    static RECORDS: RefCell<Vec<Record>> = const { RefCell::new(Vec::new()) };
}

/// Records a message on the current thread.
pub fn emit(level: Level, module: &'static str, line: u32, message: String) {
    RECORDS.with(|records| {
        records.borrow_mut().push(Record {
            level,
            module,
            line,
            message,
        });
    });
}

/// Removes and returns the records emitted on the current thread.
pub fn take() -> Vec<Record> {
    RECORDS.with(|records| std::mem::take(&mut *records.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_taken_in_order() {
        take();
        emit(Level::Warning, "strategies", 468, "first".into());
        emit(Level::Error, "decisions", 219, "second".into());
        let records = take();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message, "first");
        assert_eq!(records[1].level.initial(), 'E');
        assert!(take().is_empty());
    }

    #[test]
    fn levels_order_like_python() {
        assert!(Level::Debug < Level::Info && Level::Warning < Level::Error);
        assert_eq!(Level::Critical as u32, 50);
    }
}

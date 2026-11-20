//! The `logging` setup of `nbdime.log.init_logging`: records go to stderr as
//! `[W module:line] message`, filtered by the level `--log-level` sets.

use nbdime_core::log::{Level, Record};

pub struct Logger {
    threshold: Level,
}

impl Default for Logger {
    fn default() -> Self {
        Logger {
            threshold: Level::Info,
        }
    }
}

impl Logger {
    /// Sets the level by the name `--log-level` accepts.
    pub fn set_level(&mut self, name: &str) {
        self.threshold = match name {
            "DEBUG" => Level::Debug,
            "WARN" | "WARNING" => Level::Warning,
            "ERROR" => Level::Error,
            "CRITICAL" => Level::Critical,
            _ => Level::Info,
        };
    }

    pub fn enabled(&self, level: Level) -> bool {
        level >= self.threshold
    }

    /// The line `logging` writes for a message.
    pub fn format(level: Level, module: &str, line: u32, message: &str) -> String {
        format!("[{} {module}:{line}] {message}\n", level.initial())
    }

    pub fn log(&self, level: Level, module: &str, line: u32, message: &str) {
        if self.enabled(level) {
            crate::osstr::eprint(&Self::format(level, module, line, message));
        }
    }

    /// Writes the records the engine collected, in the order they were emitted.
    pub fn log_records(&self, records: &[Record]) {
        for record in records {
            self.log(record.level, record.module, record.line, &record.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_matches_logging() {
        assert_eq!(
            Logger::format(Level::Warning, "nbmergeapp", 56, "Conflicts."),
            "[W nbmergeapp:56] Conflicts.\n"
        );
    }

    #[test]
    fn levels_filter() {
        let mut logger = Logger::default();
        assert!(logger.enabled(Level::Info) && !logger.enabled(Level::Debug));
        logger.set_level("ERROR");
        assert!(!logger.enabled(Level::Warning) && logger.enabled(Level::Error));
        logger.set_level("WARN");
        assert!(logger.enabled(Level::Warning));
        logger.set_level("DEBUG");
        assert!(logger.enabled(Level::Debug));
    }
}

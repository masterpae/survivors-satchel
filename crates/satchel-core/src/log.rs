//! A tiny log sink: the app points it at its log file, tests leave it unset.

use std::sync::OnceLock;

type Sink = Box<dyn Fn(&str) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

pub fn set_sink(sink: impl Fn(&str) + Send + Sync + 'static) {
    let _ = SINK.set(Box::new(sink));
}

pub fn write(message: &str) {
    if let Some(sink) = SINK.get() {
        sink(message);
    }
}

/// `log!("sent version {n}")`: formats and writes one log line.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}

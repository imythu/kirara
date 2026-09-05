use std::io::{self, Write};
use std::sync::OnceLock;

use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;

use crate::error::AppError;

const DEPENDENCY_LOG_DIRECTIVES: &[&str] = &[
    "hyper=info",
    "hyper_util=info",
    "h2=info",
    "html5ever=info",
    "reqwest=info",
    "rustls=info",
    "selectors=info",
    "tungstenite=info",
];
const LOG_CHANNEL_CAPACITY: usize = 1024;

tokio::task_local! {
    pub static TASK_LOG_CONTEXT: String;
}

fn reload_handle() -> &'static OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> {
    static HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
        OnceLock::new();
    &HANDLE
}

fn log_sender() -> &'static broadcast::Sender<String> {
    static SENDER: OnceLock<broadcast::Sender<String>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, _) = broadcast::channel(LOG_CHANNEL_CAPACITY);
        sender
    })
}

pub fn build_log_filter(log_level: Option<&str>) -> Result<EnvFilter, AppError> {
    match log_level {
        Some(level) => {
            let filter = normalize_log_filter(level);
            EnvFilter::try_new(filter).map_err(|error| AppError::InvalidConfig {
                message: format!("global.log_level is invalid: {}", error),
            })
        }
        None => {
            let level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
            Ok(EnvFilter::try_new(normalize_log_filter(&level))
                .unwrap_or_else(|_| EnvFilter::new(normalize_log_filter("info"))))
        }
    }
}

fn normalize_log_filter(level: &str) -> String {
    let mut directives = Vec::with_capacity(1 + DEPENDENCY_LOG_DIRECTIVES.len());
    directives.push(level.to_string());
    for directive in DEPENDENCY_LOG_DIRECTIVES {
        let target = directive
            .split_once('=')
            .map(|(target, _)| target)
            .unwrap_or(directive);
        let explicitly_configured = level.split(',').any(|configured| {
            configured
                .trim()
                .split_once('=')
                .is_some_and(|(configured_target, _)| configured_target.trim() == target)
        });
        if !explicitly_configured {
            directives.push((*directive).to_string());
        }
    }
    directives.join(",")
}

pub fn init_logging(filter: EnvFilter) {
    let (filter_layer, handle) = reload::Layer::new(filter);
    let _ = reload_handle().set(handle);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(LogWriterFactory);

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();
}

pub fn update_log_filter(log_level: Option<&str>) -> Result<(), AppError> {
    let filter = build_log_filter(log_level)?;
    let handle = reload_handle().get().ok_or_else(|| AppError::Server {
        message: "logging reload handle not initialized".to_string(),
    })?;
    handle.reload(filter).map_err(|error| AppError::Server {
        message: format!("failed to reload log filter: {}", error),
    })
}

pub fn subscribe_logs() -> broadcast::Receiver<String> {
    log_sender().subscribe()
}

pub fn current_task_context() -> String {
    TASK_LOG_CONTEXT
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "main".to_string())
}

#[derive(Clone, Copy)]
struct LogWriterFactory;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriterFactory {
    type Writer = BroadcastWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BroadcastWriter {
            sender: log_sender().clone(),
            pending: Vec::new(),
        }
    }
}

struct BroadcastWriter {
    sender: broadcast::Sender<String>,
    pending: Vec<u8>,
}

impl BroadcastWriter {
    fn flush_lines(&mut self, force_tail: bool) {
        while let Some(pos) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=pos).collect::<Vec<_>>();
            self.emit_line(&line);
        }

        if force_tail && !self.pending.is_empty() {
            let tail = std::mem::take(&mut self.pending);
            self.emit_line(&tail);
        }
    }

    fn emit_line(&self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes)
            .trim_end_matches(&['\r', '\n'][..])
            .to_string();
        if text.is_empty() {
            return;
        }

        let redacted = redact_sensitive_values(&strip_ansi_sequences(&text));
        let _ = writeln!(io::stdout(), "{redacted}");
        let _ = self.sender.send(redacted);
    }
}

impl Write for BroadcastWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.flush_lines(false);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()?;
        self.flush_lines(true);
        Ok(())
    }
}

impl Drop for BroadcastWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            continue;
        }

        output.push(ch);
    }

    output
}

fn redact_sensitive_values(input: &str) -> String {
    redact_query_value(input, "token")
}

fn redact_query_value(input: &str, key: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let pattern = format!("{key}=");
    let mut rest = input;

    while let Some(index) = rest.find(&pattern) {
        let (before, after_before) = rest.split_at(index);
        output.push_str(before);
        output.push_str(&pattern);
        output.push_str("[REDACTED]");

        let value_start = pattern.len();
        let after_value_start = &after_before[value_start..];
        let value_end = after_value_start
            .find(|ch| matches!(ch, '&' | ' ' | '\t' | '\r' | '\n'))
            .unwrap_or(after_value_start.len());
        rest = &after_value_start[value_end..];
    }

    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_debug_noise_is_capped_for_simple_and_custom_filters() {
        let simple = normalize_log_filter("debug");
        assert!(simple.contains("selectors=info"));
        assert!(simple.contains("html5ever=info"));

        let custom = normalize_log_filter("debug,kirara=trace");
        assert!(custom.contains("kirara=trace"));
        assert!(custom.contains("selectors=info"));
        assert!(custom.contains("html5ever=info"));
    }

    #[test]
    fn explicit_dependency_filter_is_preserved() {
        let filter = normalize_log_filter("debug,html5ever=trace");
        assert!(filter.contains("html5ever=trace"));
        assert!(!filter.contains("html5ever=info"));
        assert!(filter.contains("selectors=info"));
    }
}

//! A capturing `tracing` layer, and a driver that runs terminal-mcp's real
//! service under it.
//!
//! The telemetry criteria are about what the dispatch and execution paths
//! emit, so a test has to read the spans and events back rather than assert a
//! constant against itself. Each test file gets its own copy of this module,
//! so not every item is reached from every file.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use mcp_core::{ServerCore, Session};
use serde_json::Value;
use tracing::Level;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// One span, as the subscriber saw it. A span whose fields are recorded after
/// creation appears a second time, carrying only what was recorded then.
#[derive(Clone, Debug)]
pub struct RecordedSpan {
    /// The span's name.
    pub name: &'static str,
    /// Field name to its rendered value.
    pub fields: BTreeMap<String, String>,
}

/// One event, as the subscriber saw it.
#[derive(Clone, Debug)]
pub struct RecordedEvent {
    /// The level the event was emitted at.
    pub level: Level,
    /// Field name to its rendered value. The message is the `message` field.
    pub fields: BTreeMap<String, String>,
}

/// Everything one captured run produced.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    /// Spans, in the order they opened.
    pub spans: Vec<RecordedSpan>,
    /// Events, in the order they were emitted.
    pub events: Vec<RecordedEvent>,
}

impl Recorded {
    /// A short rendering for an assertion message.
    pub fn span_summary(&self) -> Vec<String> {
        self.spans
            .iter()
            .map(|span| format!("{}{:?}", span.name, span.fields))
            .collect()
    }

    /// A short rendering for an assertion message.
    pub fn event_summary(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| format!("{}{:?}", event.level, event.fields))
            .collect()
    }
}

/// Run `body` with a capturing subscriber installed on this thread, and return
/// what it emitted.
pub fn capture<F, Fut>(body: F) -> Recorded
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        runtime.block_on(body());
    });
    capture.take()
}

/// A shared core over terminal-mcp's real, zero-config service. Fresh every
/// call: dynamic scripts stored by one test must never leak into another.
pub fn demo_core() -> Arc<ServerCore> {
    let service = terminal_mcp::build_service().expect("build_service with built-in defaults");
    ServerCore::new(terminal_mcp::server_config(), Arc::new(service))
}

/// Drive `messages` through one session over the real service, capturing what
/// the dispatch and execution paths emitted.
pub fn capture_dispatch(messages: &[Value]) -> Recorded {
    let messages = messages.to_vec();
    capture(|| async move {
        let mut session = Session::new(demo_core());
        for message in messages {
            session.handle_message(message).await;
        }
    })
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Recorded>>);

impl Capture {
    fn take(self) -> Recorded {
        self.0
            .lock()
            .expect("the capture lock is only held to push one record")
            .clone()
    }
}

impl<S> Layer<S> for Capture
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        attrs.record(&mut Collector(&mut fields));
        self.0
            .lock()
            .expect("the capture lock is only held to push one record")
            .spans
            .push(RecordedSpan {
                name: attrs.metadata().name(),
                fields,
            });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let name = ctx.span(id).map_or("<closed>", |span| span.name());
        let mut fields = BTreeMap::new();
        values.record(&mut Collector(&mut fields));
        self.0
            .lock()
            .expect("the capture lock is only held to push one record")
            .spans
            .push(RecordedSpan { name, fields });
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        event.record(&mut Collector(&mut fields));
        self.0
            .lock()
            .expect("the capture lock is only held to push one record")
            .events
            .push(RecordedEvent {
                level: *event.metadata().level(),
                fields,
            });
    }
}

struct Collector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Collector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

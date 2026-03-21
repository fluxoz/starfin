// Port of dash.js/src/core/EventBus.js
//
// A priority-based event bus with support for one-shot handlers and
// stream/media-type filtering.

use std::collections::HashMap;

use crate::core::events::Event;

/// Priority constants matching dash.js `EVENT_PRIORITY_LOW` / `EVENT_PRIORITY_HIGH`.
pub const EVENT_PRIORITY_LOW: i32 = 0;
pub const EVENT_PRIORITY_HIGH: i32 = 5000;

/// Payload carried alongside an event.
#[derive(Clone, Debug, Default)]
pub struct EventData {
    /// Arbitrary key-value pairs (mirrors the JS `payload` object).
    pub values: HashMap<String, serde_json::Value>,
    /// Optional stream-id filter.
    pub stream_id: Option<String>,
    /// Optional media-type filter (e.g. "audio", "video").
    pub media_type: Option<String>,
}

/// Unique handle returned when registering a handler — used for removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId(u64);

type Callback = Box<dyn Fn(&EventData)>;

struct Handler {
    id: HandlerId,
    callback: Callback,
    priority: i32,
    once: bool,
    stream_id: Option<String>,
    media_type: Option<String>,
}

/// Options for registering an event handler.
#[derive(Clone, Debug, Default)]
pub struct HandlerOptions {
    pub priority: Option<i32>,
    pub stream_id: Option<String>,
    pub media_type: Option<String>,
}

/// A synchronous, priority-sorted event bus.
///
/// Mirrors the dash.js `EventBus` singleton.
pub struct EventBus {
    handlers: HashMap<Event, Vec<Handler>>,
    next_id: u64,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            next_id: 0,
        }
    }

    /// Register a persistent handler.
    pub fn on(
        &mut self,
        event: Event,
        callback: impl Fn(&EventData) + 'static,
        options: Option<HandlerOptions>,
    ) -> HandlerId {
        self.register(event, Box::new(callback), false, options)
    }

    /// Register a handler that fires at most once.
    pub fn once(
        &mut self,
        event: Event,
        callback: impl Fn(&EventData) + 'static,
        options: Option<HandlerOptions>,
    ) -> HandlerId {
        self.register(event, Box::new(callback), true, options)
    }

    /// Remove a handler by its [`HandlerId`].
    pub fn off(&mut self, event: &Event, id: HandlerId) {
        if let Some(vec) = self.handlers.get_mut(event) {
            vec.retain(|h| h.id != id);
        }
    }

    /// Trigger an event, invoking matching handlers in priority order (high → low).
    pub fn trigger(&mut self, event: Event, data: EventData) {
        let handlers = match self.handlers.get_mut(&event) {
            Some(h) => h,
            None => return,
        };

        let mut to_remove: Vec<HandlerId> = Vec::new();

        for handler in handlers.iter() {
            // Stream-id filter
            if let (Some(filter_sid), Some(handler_sid)) =
                (&data.stream_id, &handler.stream_id)
            {
                if filter_sid != handler_sid {
                    continue;
                }
            }
            // Media-type filter
            if let (Some(filter_mt), Some(handler_mt)) =
                (&data.media_type, &handler.media_type)
            {
                if filter_mt != handler_mt {
                    continue;
                }
            }

            (handler.callback)(&data);

            if handler.once {
                to_remove.push(handler.id);
            }
        }

        for id in to_remove {
            handlers.retain(|h| h.id != id);
        }
    }

    /// Remove **all** handlers.
    pub fn reset(&mut self) {
        self.handlers.clear();
    }

    // ── Internal ─────────────────────────────────────────────────────────

    fn register(
        &mut self,
        event: Event,
        callback: Callback,
        once: bool,
        options: Option<HandlerOptions>,
    ) -> HandlerId {
        let opts = options.unwrap_or_default();
        let priority = opts.priority.unwrap_or(EVENT_PRIORITY_LOW);
        let id = HandlerId(self.next_id);
        self.next_id += 1;

        let handler = Handler {
            id,
            callback,
            priority,
            once,
            stream_id: opts.stream_id,
            media_type: opts.media_type,
        };

        let vec = self.handlers.entry(event).or_default();

        // Insert in priority order (highest first).
        let pos = vec
            .iter()
            .position(|h| priority > h.priority)
            .unwrap_or(vec.len());
        vec.insert(pos, handler);

        id
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("handler_count", &self.handlers.values().map(|v| v.len()).sum::<usize>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn on_and_trigger() {
        let mut bus = EventBus::new();
        let called = Rc::new(Cell::new(false));
        let called_clone = called.clone();

        bus.on(Event::PlaybackStarted, move |_| called_clone.set(true), None);
        bus.trigger(Event::PlaybackStarted, EventData::default());

        assert!(called.get());
    }

    #[test]
    fn once_fires_only_once() {
        let mut bus = EventBus::new();
        let count = Rc::new(Cell::new(0u32));
        let count_clone = count.clone();

        bus.once(Event::ManifestLoaded, move |_| {
            count_clone.set(count_clone.get() + 1);
        }, None);

        bus.trigger(Event::ManifestLoaded, EventData::default());
        bus.trigger(Event::ManifestLoaded, EventData::default());

        assert_eq!(count.get(), 1);
    }

    #[test]
    fn off_removes_handler() {
        let mut bus = EventBus::new();
        let called = Rc::new(Cell::new(false));
        let called_clone = called.clone();

        let id = bus.on(Event::Error, move |_| called_clone.set(true), None);
        bus.off(&Event::Error, id);
        bus.trigger(Event::Error, EventData::default());

        assert!(!called.get());
    }

    #[test]
    fn priority_ordering() {
        let mut bus = EventBus::new();
        let order = Rc::new(std::cell::RefCell::new(Vec::new()));

        let o1 = order.clone();
        bus.on(Event::BufferEmpty, move |_| o1.borrow_mut().push("low"), None);

        let o2 = order.clone();
        bus.on(
            Event::BufferEmpty,
            move |_| o2.borrow_mut().push("high"),
            Some(HandlerOptions {
                priority: Some(EVENT_PRIORITY_HIGH),
                ..Default::default()
            }),
        );

        bus.trigger(Event::BufferEmpty, EventData::default());

        let result = order.borrow();
        assert_eq!(&*result, &["high", "low"]);
    }

    #[test]
    fn reset_clears_all() {
        let mut bus = EventBus::new();
        let called = Rc::new(Cell::new(false));
        let c = called.clone();
        bus.on(Event::Error, move |_| c.set(true), None);
        bus.reset();
        bus.trigger(Event::Error, EventData::default());
        assert!(!called.get());
    }
}

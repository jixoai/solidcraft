//! Browser host for the reusable Rust engine worker. Only the loader is JavaScript: it imports
//! this build's generated bindings and instantiates the already compiled WebAssembly.Module.
//! Requests and exact results travel as transferable ArrayBuffers, with one transaction in flight.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use js_sys::{Array, ArrayBuffer, Object, Reflect, Uint8Array};
use serde_json::{Value, json};
use solidcraft_engine::worker::{self as engine, Source};
use solidcraft_ui_egui::{ControlRequest, SolveApp};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

fn now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}
fn error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

#[derive(Default)]
struct Stats {
    ready: bool,
    workers: u32,
    requests: u64,
    local_requests: u64,
    pending: usize,
    compute_ms: f64,
    apply_ms: f64,
    request_bytes: usize,
    reply_bytes: usize,
}

enum Message {
    Ready,
    Reply(ArrayBuffer),
    Failed(String),
}
struct Incoming {
    generation: u32,
    message: Message,
}

struct Worker {
    worker: web_sys::Worker,
    url: String,
    _message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

impl Worker {
    fn start(generation: u32, inbox: Rc<RefCell<VecDeque<Incoming>>>, ctx: egui::Context) -> Result<Self, String> {
        let window = web_sys::window().ok_or("worker needs a window")?;
        let document = window.document().ok_or("worker needs a document")?;
        let module = document
            .query_selector("link[rel='modulepreload'][href*='solidcraft-web']")
            .map_err(error)?
            .and_then(|link| link.get_attribute("href"))
            .ok_or("cannot locate this build's wasm bindings")?;
        let base = window.location().href().map_err(error)?;
        let module = web_sys::Url::new_with_base(&module, &base).map_err(error)?.href();
        // Required module-loader glue. All command routing, validation and execution is Rust.
        let loader = document
            .get_element_by_id("solidcraft_worker_bootstrap")
            .and_then(|element| element.text_content())
            .ok_or("cannot locate the engine worker bootstrap")?;
        let parts = Array::of1(&JsValue::from_str(&loader));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("text/javascript");
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options).map_err(error)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(error)?;
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        options.set_name("SolidCraft engine");
        let worker = match web_sys::Worker::new_with_options(&url, &options) {
            Ok(worker) => worker,
            Err(e) => {
                let _ = web_sys::Url::revoke_object_url(&url);
                return Err(error(e));
            }
        };
        let messages = inbox.clone();
        let wake = ctx.clone();
        let message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let data = event.data();
            let message = if data.as_string().as_deref() == Some("ready") {
                Message::Ready
            } else if let Ok(buffer) = data.clone().dyn_into::<ArrayBuffer>() {
                Message::Reply(buffer)
            } else {
                Message::Failed(error(data))
            };
            messages.borrow_mut().push_back(Incoming { generation, message });
            wake.request_repaint();
        });
        let on_error = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |event: web_sys::ErrorEvent| {
            event.prevent_default();
            inbox.borrow_mut().push_back(Incoming { generation, message: Message::Failed(format!("engine worker stopped: {}", event.message())) });
            ctx.request_repaint();
        });
        worker.set_onmessage(Some(message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let init = Object::new();
        Reflect::set(&init, &"url".into(), &module.into()).map_err(error)?;
        Reflect::set(&init, &"module".into(), &wasm_bindgen::module()).map_err(error)?;
        let handle = Self { worker, url, _message: message, _error: on_error };
        handle.worker.post_message(&init).map_err(error)?;
        Ok(handle)
    }

    fn send(&self, bytes: &[u8]) -> Result<(), String> {
        let buffer = Uint8Array::from(bytes).buffer();
        self.worker.post_message_with_transfer(&buffer, &Array::of1(&buffer)).map_err(error)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
        let _ = web_sys::Url::revoke_object_url(&self.url);
    }
}

enum Pending {
    Worker { request: ControlRequest, source: Box<Source>, cancel: Arc<AtomicBool>, tries: u8 },
    Local { request: ControlRequest, reply: Receiver<Value> },
}

pub struct Control {
    requests: Receiver<ControlRequest>,
    local: Sender<ControlRequest>,
    queue: VecDeque<(ControlRequest, u8)>,
    pending: Option<Pending>,
    worker: Option<Worker>,
    worker_error: Option<String>,
    inbox: Rc<RefCell<VecDeque<Incoming>>>,
    stats: Rc<RefCell<Stats>>,
    current: Option<Source>,
    started: f64,
}

impl Control {
    pub fn new(requests: Receiver<ControlRequest>, local: Sender<ControlRequest>, ctx: &egui::Context) -> Self {
        let mut control = Self {
            requests,
            local,
            queue: VecDeque::new(),
            pending: None,
            worker: None,
            worker_error: None,
            inbox: Rc::default(),
            stats: Rc::default(),
            current: None,
            started: now(),
        };
        control.start(ctx);
        control
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.stats.borrow_mut().workers += 1;
        let generation = self.stats.borrow().workers;
        self.started = now();
        match Worker::start(generation, self.inbox.clone(), ctx.clone()) {
            Ok(worker) => {
                self.worker = Some(worker);
                self.worker_error = None;
            }
            Err(error) => self.worker_error = Some(error),
        }
    }

    fn reset(&mut self) {
        self.worker = None;
        self.current = None;
        self.stats.borrow_mut().ready = false;
    }

    pub fn install_diagnostics(&self) -> Result<(), JsValue> {
        let stats = self.stats.clone();
        let runtime = Closure::<dyn Fn() -> JsValue>::new(move || {
            let stats = stats.borrow();
            let value = json!({"worker_ready":stats.ready,"workers_started":stats.workers,"worker_requests":stats.requests,
                "local_requests":stats.local_requests,"pending":stats.pending,"last_compute_ms":stats.compute_ms,
                "last_apply_ms":stats.apply_ms,"last_request_bytes":stats.request_bytes,"last_reply_bytes":stats.reply_bytes});
            js_sys::JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL)
        });
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let api = Reflect::get(&window, &"solidcraft".into())?;
        Reflect::set(&api, &"runtime".into(), &runtime.into_js_value())?;
        Ok(())
    }

    pub fn tick(&mut self, app: &mut SolveApp, ctx: &egui::Context) {
        for _ in 0..256 {
            let Ok(request) = self.requests.try_recv() else { break };
            if self.queue.len() >= 256 {
                fail(request, "too many queued SolidCraft requests");
            } else {
                self.queue.push_back((request, 0));
            }
        }
        self.receive(app);
        self.finish_local();
        if let Some(Pending::Worker { source, cancel, .. }) = &self.pending {
            let cancelled = cancel.load(Ordering::Relaxed);
            if cancelled || !source.is_current(&app.session) {
                let same_design = source.same_design(&app.session);
                if let Some(Pending::Worker { request, tries, .. }) = self.pending.take() {
                    self.reset();
                    app.worker_control_finished();
                    if !cancelled && same_design && tries < 2 {
                        self.queue.push_front((request, tries + 1));
                    } else {
                        fail(
                            request,
                            if cancelled {
                                "agent command stopped; nothing was applied"
                            } else {
                                "the design changed while the agent command computed; nothing was applied"
                            },
                        );
                    }
                }
            }
        }
        if self.pending.is_none() {
            self.next(app, ctx);
        }
        let mut stats = self.stats.borrow_mut();
        stats.pending = self.queue.len() + usize::from(self.pending.is_some());
        if stats.pending > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn finish_local(&mut self) {
        let Some(Pending::Local { reply, .. }) = &self.pending else { return };
        let result = match reply.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Disconnected) => json!({"ok":false,"error":"SolidCraft request dropped"}),
            Err(TryRecvError::Empty) => return,
        };
        if let Some(Pending::Local { request, .. }) = self.pending.take() {
            let _ = request.reply.send(result);
        }
    }

    fn receive(&mut self, app: &mut SolveApp) {
        let messages: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for event in messages {
            if event.generation != self.stats.borrow().workers {
                continue;
            }
            match event.message {
                Message::Ready => self.stats.borrow_mut().ready = true,
                Message::Failed(error) => {
                    let running = matches!(&self.pending, Some(Pending::Worker { .. }));
                    self.reset();
                    // A failed transaction never poisons subsequent requests. Its source is
                    // still owned by the UI, so the next request can start a fresh worker.
                    // An initialization failure is different: retain it instead of repeatedly
                    // starting a worker that this browser or its CSP cannot initialize.
                    self.worker_error = (!running).then(|| error.clone());
                    if running && let Some(Pending::Worker { request, .. }) = self.pending.take() {
                        fail(request, &error);
                    }
                    app.worker_control_finished();
                }
                Message::Reply(buffer) => {
                    if !app.worker_control_ready() {
                        self.inbox.borrow_mut().push_back(Incoming { generation: event.generation, message: Message::Reply(buffer) });
                        continue;
                    }
                    if !matches!(&self.pending, Some(Pending::Worker { .. })) {
                        continue;
                    }
                    let Some(Pending::Worker { request, source, cancel, tries }) = self.pending.take() else { continue };
                    app.worker_control_finished();
                    if cancel.load(Ordering::Relaxed) {
                        self.reset();
                        fail(request, "agent command stopped; nothing was applied");
                        continue;
                    }
                    if !source.is_current(&app.session) {
                        let same = source.same_design(&app.session);
                        self.reset();
                        if same && tries < 2 {
                            self.queue.push_front((request, tries + 1));
                        } else {
                            fail(request, "the design changed while the agent command computed; nothing was applied");
                        }
                        continue;
                    }
                    let started = now();
                    let length = buffer.byte_length() as usize;
                    self.stats.borrow_mut().reply_bytes = length;
                    let result = if length > engine::MAX_MESSAGE_BYTES {
                        Err("worker reply exceeds 128 MiB".into())
                    } else {
                        engine::Reply::decode(&Uint8Array::new(&buffer).to_vec()).and_then(|reply| source.accept(&mut app.session, reply))
                    };
                    let result = result.and_then(|completion| {
                        self.stats.borrow_mut().compute_ms = completion.compute_ms;
                        app.complete_worker_control(completion)
                    });
                    self.stats.borrow_mut().apply_ms = now() - started;
                    self.current = result.as_ref().ok().map(|_| Source::new(&app.session));
                    let reply = match result {
                        Ok(result) => json!({"ok":true,"result":result}),
                        Err(error) => {
                            self.current = None;
                            json!({"ok":false,"error":error})
                        }
                    };
                    let _ = request.reply.send(reply);
                }
            }
        }
    }

    fn next(&mut self, app: &mut SolveApp, ctx: &egui::Context) {
        let Some((request, tries)) = self.queue.pop_front() else { return };
        if request.is_cancelled() {
            fail(request, "SolidCraft request cancelled");
            return;
        }
        if app.preferences.agent_off_thread && engine::supports_session(&app.session) && engine::supports(&request.method, &request.params) {
            match app.prepare_worker_control(&request.method, &request.params) {
                Ok(true) => {}
                Ok(false) => {
                    self.queue.push_front((request, tries));
                    return;
                }
                Err(error) => {
                    fail(request, &error);
                    return;
                }
            }
            if self.worker.is_none() && self.worker_error.is_none() {
                self.start(ctx);
            }
            if let Some(error) = &self.worker_error {
                fail(
                    request,
                    &format!(
                        "engine worker unavailable: {error}; turn off 'Keep the window responsive while agents run commands' to use the synchronous path"
                    ),
                );
                return;
            }
            if !self.stats.borrow().ready {
                if now() - self.started > 30_000.0 {
                    self.reset();
                    self.worker_error = Some("initialization timed out".into());
                }
                self.queue.push_front((request, tries));
                return;
            }
            let synchronize = !self.current.as_ref().is_some_and(|source| source.is_current(&app.session));
            let source = Source::new(&app.session);
            let bytes = match source.request(&request.method, request.params.clone(), synchronize, solidcraft_engine::doc::display_quality::current())
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    fail(request, &error);
                    return;
                }
            };
            let result = self.worker.as_ref().ok_or_else(|| "engine worker unavailable".to_string()).and_then(|worker| worker.send(&bytes));
            match result {
                Ok(()) => {
                    let cancel = request.cancel.clone().unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                    request.started.store(true, Ordering::Relaxed);
                    app.worker_control_started(&request.method, &request.params, cancel.clone());
                    let mut stats = self.stats.borrow_mut();
                    stats.requests += 1;
                    stats.request_bytes = bytes.len();
                    self.pending = Some(Pending::Worker { request, source: Box::new(source), cancel, tries });
                }
                Err(error) => {
                    self.reset();
                    fail(request, &error);
                }
            }
        } else {
            // At most one synchronous request per frame; even a flood of cheap calls yields
            // between requests. A later request never overtakes an earlier worker transaction.
            let (mut local, reply) = ControlRequest::new(request.method.clone(), request.params.clone());
            local.cancel = request.cancel.clone();
            if self.local.send(local).is_err() {
                fail(request, "SolidCraft is not running");
                return;
            }
            request.started.store(true, Ordering::Relaxed);
            self.stats.borrow_mut().local_requests += 1;
            self.pending = Some(Pending::Local { request, reply });
        }
    }
}

fn fail(request: ControlRequest, error: &str) {
    let _ = request.reply.send(json!({"ok":false,"error":error}));
}

/// Entry point when this same wasm module starts in a Worker instead of a Window.
pub fn start() {
    let Ok(scope) = js_sys::global().dyn_into::<web_sys::DedicatedWorkerGlobalScope>() else { return };
    let target = scope.clone();
    let mut engine = engine::Runtime::default();
    let message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
        let result = event.data().dyn_into::<ArrayBuffer>().map_err(|_| "invalid engine worker request".to_string()).and_then(|buffer| {
            if buffer.byte_length() as usize > engine::MAX_MESSAGE_BYTES {
                return Err("worker request exceeds 128 MiB".into());
            }
            engine.run(&Uint8Array::new(&buffer).to_vec())
        });
        match result {
            Ok(bytes) => {
                let buffer = Uint8Array::from(bytes.as_slice()).buffer();
                let _ = target.post_message_with_transfer(&buffer, &Array::of1(&buffer));
            }
            Err(error) => {
                let _ = target.post_message(&error.into());
            }
        }
    });
    scope.set_onmessage(Some(message.as_ref().unchecked_ref()));
    message.forget();
    let _ = scope.post_message(&"ready".into());
}

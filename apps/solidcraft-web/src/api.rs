//! Promise-based access to the running app's existing control channel.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde_json::{Value, json};
use solidcraft_ui_egui::ControlRequest;
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

const MAX_PENDING: usize = 256;
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

struct Pending {
    reply: Receiver<Value>,
    resolve: js_sys::Function,
    reject: js_sys::Function,
    bytes: usize,
    abort: Option<Abort>,
}

struct Abort {
    signal: web_sys::AbortSignal,
    cancel: Arc<AtomicBool>,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

impl Abort {
    fn new(signal: web_sys::AbortSignal, ctx: egui::Context) -> Result<Self, JsValue> {
        let cancel = Arc::new(AtomicBool::new(signal.aborted()));
        let flag = cancel.clone();
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            flag.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
        signal.add_event_listener_with_callback("abort", callback.as_ref().unchecked_ref())?;
        Ok(Self { signal, cancel, callback })
    }
}

impl Drop for Abort {
    fn drop(&mut self) {
        let _ = self.signal.remove_event_listener_with_callback("abort", self.callback.as_ref().unchecked_ref());
    }
}

/// Count the queued JSON without allocating another copy of a potentially large request.
struct JsonBytes(usize);
impl std::io::Write for JsonBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_JSON_BYTES.saturating_sub(self.0) {
            return Err(std::io::Error::other("params exceed 16 MiB"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct Api {
    sender: Sender<ControlRequest>,
    pending: Rc<RefCell<Vec<Pending>>>,
    ctx: egui::Context,
}

pub(crate) fn params(value: &JsValue) -> Result<Value, String> {
    if value.is_undefined() || value.is_null() {
        return Ok(json!({}));
    }
    let text = js_sys::JSON::stringify(value).ok().and_then(|v| v.as_string()).ok_or("params must be JSON serializable")?;
    if text.len() > MAX_JSON_BYTES {
        return Err("params exceed 16 MiB".into());
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid params: {e}"))
}

fn error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

impl Api {
    pub fn install(sender: Sender<ControlRequest>, ctx: egui::Context) -> Result<Self, JsValue> {
        let api = Self { sender, pending: Rc::default(), ctx };
        let object = js_sys::Object::new();
        let request = api.clone();
        let execute = api.clone();
        let commands = api.clone();
        let inspect = api.clone();
        for (name, function) in [
            (
                "request",
                Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(move |method: String, value: JsValue| {
                    request.request(&method, params(&value))
                })
                .into_js_value(),
            ),
            (
                "execute",
                Closure::<dyn Fn(String, JsValue) -> js_sys::Promise>::new(move |command: String, value: JsValue| {
                    execute.request("engine.execute", params(&value).map(|p| json!({"command": command, "params": p})))
                })
                .into_js_value(),
            ),
            ("commands", Closure::<dyn Fn() -> js_sys::Promise>::new(move || commands.request("engine.commands", Ok(json!({})))).into_js_value()),
            (
                "inspect",
                Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |value: JsValue| inspect.request("document.inspect", params(&value)))
                    .into_js_value(),
            ),
        ] {
            js_sys::Reflect::set(&object, &name.into(), &function)?;
        }
        let window = web_sys::window().ok_or_else(|| error("no window"))?;
        js_sys::Reflect::set(&window, &"solidcraft".into(), &object)?;
        // An agent in the browser finds the same surface through WebMCP, where the browser has it.
        crate::webmcp::install(&api);
        Ok(api)
    }

    pub(crate) fn request(&self, method: &str, params: Result<Value, String>) -> js_sys::Promise {
        self.request_with_signal(method, params, None)
    }

    pub(crate) fn request_with_signal(&self, method: &str, params: Result<Value, String>, signal: Option<web_sys::AbortSignal>) -> js_sys::Promise {
        if signal.as_ref().is_some_and(web_sys::AbortSignal::aborted) {
            return js_sys::Promise::reject(&error("SolidCraft request cancelled"));
        }
        let params = match params {
            Ok(params) => params,
            Err(message) => return js_sys::Promise::reject(&error(&message)),
        };
        if self.pending.borrow().len() >= MAX_PENDING {
            return js_sys::Promise::reject(&error("too many pending SolidCraft requests (maximum 256)"));
        }
        let mut bytes = JsonBytes(0);
        if let Err(message) = serde_json::to_writer(&mut bytes, &params) {
            return js_sys::Promise::reject(&error(&message.to_string()));
        }
        let bytes = bytes.0;
        if bytes > MAX_PENDING_BYTES.saturating_sub(self.pending.borrow().iter().map(|pending| pending.bytes).sum()) {
            return js_sys::Promise::reject(&error("pending SolidCraft parameters exceed 32 MiB"));
        }
        js_sys::Promise::new(&mut |resolve, reject| {
            let abort = match signal.clone().map(|signal| Abort::new(signal, self.ctx.clone())).transpose() {
                Ok(abort) => abort,
                Err(message) => {
                    let _ = reject.call1(&JsValue::UNDEFINED, &message);
                    return;
                }
            };
            let (mut request, reply) = ControlRequest::new(method, params.clone());
            request.cancel = abort.as_ref().map(|abort| abort.cancel.clone());
            if self.sender.send(request).is_err() {
                let _ = reject.call1(&JsValue::UNDEFINED, &error("SolidCraft is not running"));
                return;
            }
            self.pending.borrow_mut().push(Pending { reply, resolve, reject, bytes, abort });
            self.ctx.request_repaint();
        })
    }

    pub fn poll(&self) {
        let mut completed = Vec::new();
        self.pending.borrow_mut().retain_mut(|pending| {
            if pending.abort.as_ref().is_some_and(|abort| abort.cancel.load(Ordering::Relaxed)) {
                completed.push((pending.resolve.clone(), pending.reject.clone(), Err(error("SolidCraft request cancelled"))));
                return false;
            }
            let result = match pending.reply.try_recv() {
                Ok(reply) => {
                    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                        js_sys::JSON::parse(&reply.get("result").unwrap_or(&Value::Null).to_string())
                    } else {
                        Err(error(reply.get("error").and_then(Value::as_str).unwrap_or("SolidCraft request failed")))
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => Err(error("SolidCraft request dropped")),
            };
            completed.push((pending.resolve.clone(), pending.reject.clone(), result));
            false
        });
        // Release the queue before calling JavaScript, which can submit another request.
        for (resolve, reject, result) in completed {
            match result {
                Ok(value) => {
                    let _ = resolve.call1(&JsValue::UNDEFINED, &value);
                }
                Err(error) => {
                    let _ = reject.call1(&JsValue::UNDEFINED, &error);
                }
            }
        }
        if !self.pending.borrow().is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

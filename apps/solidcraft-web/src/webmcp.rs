//! WebMCP tools: the command surface of `window.solidcraft`, declared to the browser so an
//! agent running there can find and call it ([WebMCP](https://webmachinelearning.github.io/webmcp/)).
//!
//! Chrome exposes the tools as `document.modelContext` (149 and later, behind the `WebMCP`
//! feature in [origin trials](https://developer.chrome.com/docs/ai/webmcp) or the
//! `enable-webmcp-testing` flag). A browser without WebMCP leaves `document.modelContext`
//! undefined and nothing here runs, so this stays a progressive enhancement.
//!
//! Every tool is a pass-through to a [control method](../../../docs/control-protocol.md):
//! no second registry, and the same command ids, parameters and undo history as the toolbar
//! and [`crate::api`].
//!
//! A tool answers `{ok: true, result}` or `{ok: false, error}`. A thrown error would reach the
//! agent as the browser's generic `UnknownError`, hiding what went wrong, so failures travel
//! as values instead (docs/browser-api.md).

use serde_json::{Value, json};

const MAX_BATCH_STEPS: usize = 10_000;

/// One tool: what an agent is told, and the control call the tool makes.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// The tool's input as JSON Schema, parsed when the tool is registered.
    schema: &'static str,
    /// `true` for tools that only read: the browser marks the others as consequential, since
    /// any command can run (a file command writes browser files).
    read_only: bool,
    /// The agent's arguments as the control method's parameters.
    request: fn(&Value) -> Result<(&'static str, Value), String>,
}

/// The tools this page offers. They mirror the MCP tools an agent already knows
/// (docs/mcp.md) and call the control methods behind them.
const TOOLS: &[Tool] = &[
    Tool {
        name: "solidcraft_list_commands",
        title: "List SolidCraft commands",
        description: "Every command SolidCraft can run: id, label, toolbar tab and panel, shortcut, \
            one-line parameter docs and whether it is enabled now. Ids are dotted and lower-case \
            (solid.extrude, sketch.create, solid.box). The whole catalog is large (about 350 KB): ask \
            solidcraft_describe_command about one command instead when you know its id. Read-only.",
        schema: r#"{"type": "object", "properties": {}, "additionalProperties": false}"#,
        read_only: true,
        request: list_commands,
    },
    Tool {
        name: "solidcraft_describe_command",
        title: "Describe a SolidCraft command",
        description: "One command's parameters: its one-line parameter docs and, for commands with a \
            typed contract, the JSON Schemas (2020-12) of its parameters and of a successful result. \
            Read-only.",
        schema: r#"{"type": "object", "properties": {"command": {"type": "string", "description": "Command id, e.g. solid.extrude"}}, "required": ["command"], "additionalProperties": false}"#,
        read_only: true,
        request: describe_command,
    },
    Tool {
        name: "solidcraft_execute",
        title: "Run a SolidCraft command",
        description: "Run one engine command with JSON parameters, exactly as the toolbar, palette and \
            scripts do; it never opens a dialog. Examples: {command: \"sketch.create\", params: {plane: \"XY\"}}, \
            {command: \"solid.box\", params: {length: 30, width: 20, height: 10}}, {command: \"view.fit\"}. \
            Lengths are mm and expressions may use parameters and units (\"width / 2\", \"30 deg\"). A command \
            that fails leaves the design unchanged. Changes the design (undoable) and, for file commands, \
            browser files.",
        schema: r#"{"type": "object", "properties": {"command": {"type": "string", "description": "Command id, e.g. solid.box"}, "params": {"type": "object", "description": "The command's parameters; omit for none"}}, "required": ["command"], "additionalProperties": false}"#,
        read_only: false,
        request: execute,
    },
    Tool {
        name: "solidcraft_batch",
        title: "Run several SolidCraft commands",
        description: "Run commands in order, stopping at the first error. Each step is {command, params}. \
            With rollback (the default) the design goes back to where the batch started, so the batch is \
            all-or-nothing. File writes cannot be rolled back. A failed step answers ok: false, with \
            result.failed and result.rolled_back explaining what happened. This is the usual way to \
            build a part in one call: sketch, draw, dimension, finish, extrude, holes. Changes the \
            design (undoable) and, for file commands, browser files.",
        schema: r#"{"type": "object", "properties": {"commands": {"type": "array", "minItems": 1, "maxItems": 10000, "description": "Steps: [{\"command\": id, \"params\": {...}}, ...]", "items": {"type": "object", "properties": {"command": {"type": "string", "minLength": 1}, "params": {"type": "object"}}, "required": ["command"], "additionalProperties": false}}, "rollback": {"type": "boolean", "description": "Undo the steps already run when one fails (default true)"}}, "required": ["commands"], "additionalProperties": false}"#,
        read_only: false,
        request: batch,
    },
    Tool {
        name: "solidcraft_inspect",
        title: "Inspect the SolidCraft design",
        description: "The active design: user parameters, the timeline of features, bodies (volume, area, \
            centre of mass, bounding box, face/edge/vertex counts) and sketches. measure: true measures every \
            body exactly. Read-only.",
        schema: r#"{"type": "object", "properties": {"measure": {"type": "boolean", "description": "Measure every body (volume, area, bounding box)"}}, "additionalProperties": false}"#,
        read_only: true,
        request: inspect,
    },
];

fn fields(input: &Value, allowed: &[&str]) -> Result<(), String> {
    let object = input.as_object().ok_or("arguments must be an object")?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        let shown: String = key.chars().take(64).collect();
        return Err(format!("unknown argument `{shown}`"));
    }
    Ok(())
}

fn list_commands(input: &Value) -> Result<(&'static str, Value), String> {
    fields(input, &[])?;
    Ok(("engine.commands", json!({})))
}

fn command_of(input: &Value) -> Result<String, String> {
    let command = input.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty());
    command.map(str::to_string).ok_or_else(|| "`command` must be an engine command id, e.g. solid.box".into())
}

fn describe_command(input: &Value) -> Result<(&'static str, Value), String> {
    fields(input, &["command"])?;
    let command = command_of(input)?;
    Ok(("engine.execute", json!({"command": "engine.describe", "params": {"command": command}})))
}

fn execute(input: &Value) -> Result<(&'static str, Value), String> {
    fields(input, &["command", "params"])?;
    let command = command_of(input)?;
    let params = match input.get("params") {
        None => json!({}),
        Some(params @ Value::Object(_)) => params.clone(),
        Some(_) => return Err("`params` must be an object of the command's parameters".into()),
    };
    Ok(("engine.execute", json!({"command": command, "params": params})))
}

fn batch(input: &Value) -> Result<(&'static str, Value), String> {
    fields(input, &["commands", "rollback"])?;
    let steps = match input.get("commands") {
        Some(Value::Array(steps)) if !steps.is_empty() && steps.len() <= MAX_BATCH_STEPS => steps,
        _ => return Err(format!("`commands` must contain 1 to {MAX_BATCH_STEPS} {{command, params}} steps")),
    };
    for (i, step) in steps.iter().enumerate() {
        execute(step).map_err(|error| format!("commands[{i}]: {error}"))?;
    }
    let mut params = json!({"commands": steps});
    match input.get("rollback") {
        None => {}
        Some(Value::Bool(rollback)) => params["rollback"] = Value::Bool(*rollback),
        Some(_) => return Err("`rollback` must be true or false".into()),
    }
    Ok(("engine.batch", params))
}

fn inspect(input: &Value) -> Result<(&'static str, Value), String> {
    fields(input, &["measure"])?;
    match input.get("measure") {
        None => Ok(("document.inspect", json!({}))),
        Some(Value::Bool(measure)) => Ok(("document.inspect", json!({ "measure": measure }))),
        Some(_) => Err("`measure` must be true or false".into()),
    }
}

/// Offer the tools to the browser, unless it has no WebMCP. Registration is best effort: a
/// browser that refuses a tool must not stop the app from starting.
#[cfg(target_arch = "wasm32")]
pub mod browser {
    use super::{TOOLS, Tool};
    use crate::api::Api;
    use js_sys::{Function, Object, Promise, Reflect};
    use serde_json::Value;
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

    /// The tool objects, their callbacks and the envelope callbacks live for the page (they are
    /// made with `into_js_value`, as [`Api::install`]'s methods are): a page registers its
    /// tools once and has nothing to unregister them from.
    pub fn install(api: &Api) {
        let Some(context) = model_context() else { return };
        let envelope = Envelope::new();
        let log_failure = log_failure();
        for tool in TOOLS {
            let api = api.clone();
            let envelope = envelope.clone();
            let execute = Closure::<dyn Fn(JsValue, JsValue) -> Promise>::new(move |arguments: JsValue, options: JsValue| {
                match request(tool, &api, &arguments, &options) {
                    Ok(promise) => envelope.wrap(promise, tool.name == "solidcraft_batch"),
                    Err(message) => Promise::resolve(&failed(&message)),
                }
            });
            match tool_object(tool, execute.into_js_value()).and_then(|object| call_method(&context, "registerTool", &[object.into()])) {
                Ok(registered) => {
                    let _ = call_method(&Promise::resolve(&registered), "catch", &[log_failure.clone().into()]);
                }
                Err(error) => warn(&error),
            }
        }
    }

    /// The browser's tool registry for this document, if it has one.
    fn model_context() -> Option<JsValue> {
        let document = web_sys::window()?.document()?;
        property(&document, "modelContext").ok().filter(|context| !context.is_undefined() && !context.is_null())
    }

    fn property(object: &JsValue, name: &str) -> Result<JsValue, JsValue> {
        Reflect::get(object, &JsValue::from_str(name))
    }

    /// The tool as the browser's `registerTool` takes it.
    fn tool_object(tool: &Tool, execute: JsValue) -> Result<Object, JsValue> {
        let object = Object::new();
        let annotations = Object::new();
        set(&annotations, "readOnlyHint", &tool.read_only.into());
        set(&annotations, "consequentialHint", &(!tool.read_only).into());
        set(&annotations, "untrustedContentHint", &matches!(tool.name, "solidcraft_execute" | "solidcraft_batch" | "solidcraft_inspect").into());
        set(&object, "name", &tool.name.into());
        set(&object, "title", &tool.title.into());
        set(&object, "description", &tool.description.into());
        set(&object, "inputSchema", &js_sys::JSON::parse(tool.schema)?);
        set(&object, "annotations", &annotations.into());
        set(&object, "execute", &execute);
        Ok(object)
    }

    fn set(object: &Object, name: &str, value: &JsValue) {
        // Only a frozen object or a read-only key fails, and the tool objects are neither.
        let _ = Reflect::set(object, &JsValue::from_str(name), value);
    }

    /// The agent's arguments as JSON, rejecting anything that is not an object.
    fn arguments_of(value: &JsValue) -> Result<Value, String> {
        let parsed = crate::api::params(value)?;
        if !parsed.is_object() {
            return Err("arguments must be an object".into());
        }
        Ok(parsed)
    }

    /// The control request behind one tool call, as a promise of the reply's result.
    fn request(tool: &Tool, api: &Api, arguments: &JsValue, options: &JsValue) -> Result<Promise, String> {
        let arguments = arguments_of(arguments)?;
        let (method, params) = (tool.request)(&arguments)?;
        let signal = property(options, "signal").ok().and_then(|value| value.dyn_into::<web_sys::AbortSignal>().ok());
        Ok(api.request_with_signal(method, Ok(params), signal))
    }

    /// `{ok: false, error}`: what a tool answers when the call did not happen or failed.
    fn failed(message: &str) -> JsValue {
        let object = Object::new();
        set(&object, "ok", &JsValue::FALSE);
        set(&object, "error", &JsValue::from_str(message));
        object.into()
    }

    /// Wraps a control reply as the tool's answer. One per page: the callbacks are shared by
    /// every tool, so a long session does not collect a closure per call.
    #[derive(Clone)]
    struct Envelope {
        ok: Function,
        batch: Function,
        error: Function,
    }

    impl Envelope {
        fn new() -> Self {
            let ok = Closure::<dyn Fn(JsValue) -> JsValue>::new(|result: JsValue| {
                let object = Object::new();
                set(&object, "ok", &JsValue::TRUE);
                set(&object, "result", &result);
                object.into()
            });
            let batch = Closure::<dyn Fn(JsValue) -> JsValue>::new(|result: JsValue| {
                let failure = property(&result, "failed").ok().filter(|v| !v.is_null() && !v.is_undefined());
                let object = Object::new();
                set(&object, "ok", &failure.is_none().into());
                set(&object, "result", &result);
                if let Some(failure) = failure {
                    let error = property(&failure, "error").ok().and_then(|v| v.as_string()).unwrap_or_else(|| "a batch step failed".into());
                    set(&object, "error", &error.into());
                }
                object.into()
            });
            let error = Closure::<dyn Fn(JsValue) -> JsValue>::new(|error: JsValue| failed(&message(&error)));
            Self {
                ok: ok.into_js_value().unchecked_into(),
                batch: batch.into_js_value().unchecked_into(),
                error: error.into_js_value().unchecked_into(),
            }
        }

        /// `promise.then(ok, error)`, called as JavaScript calls it so the reply's value becomes
        /// the tool's answer rather than a promise to it.
        fn wrap(&self, promise: Promise, batch: bool) -> Promise {
            let ok = if batch { &self.batch } else { &self.ok };
            let wrapped = call_method(&promise, "then", &[ok.clone().into(), self.error.clone().into()]);
            wrapped.and_then(|value| value.dyn_into::<Promise>()).unwrap_or_else(|error| Promise::resolve(&failed(&message(&error))))
        }
    }

    /// `object[name](...arguments)`: a method called the way JavaScript calls it.
    fn call_method(object: &JsValue, name: &str, arguments: &[JsValue]) -> Result<JsValue, JsValue> {
        let method = property(object, name)?.dyn_into::<Function>().map_err(|_| JsValue::from_str(&format!("{name} is not a function")))?;
        method.apply(object, &arguments.iter().collect::<js_sys::Array>())
    }

    /// A JavaScript error's message, or the value itself when it is not one.
    fn message(error: &JsValue) -> String {
        property(error, "message")
            .ok()
            .and_then(|message| message.as_string())
            .unwrap_or_else(|| error.as_string().unwrap_or_else(|| format!("{error:?}")))
    }

    /// A refused `registerTool` (a duplicate or an unknown tool name) is a programming error; say
    /// so in the console rather than failing the page.
    fn log_failure() -> Function {
        Closure::<dyn Fn(JsValue)>::new(|error: JsValue| warn(&error)).into_js_value().unchecked_into()
    }

    fn warn(error: &JsValue) {
        web_sys::console::warn_1(&format!("SolidCraft WebMCP: {}", message(error)).into());
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::install;

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> &'static Tool {
        TOOLS.iter().find(|tool| tool.name == name).unwrap_or(&TOOLS[0])
    }

    /// What the browser accepts, and that every answer comes from a control method rather than
    /// a second surface of its own.
    #[test]
    fn every_tool_is_a_control_call_with_a_usable_name_and_schema() {
        for tool in TOOLS {
            assert!(serde_json::from_str::<Value>(tool.schema).is_ok_and(|schema| schema["type"] == "object"), "{}", tool.name);
            assert!(tool.name.starts_with("solidcraft_"), "{}", tool.name);
            // WebMCP: 1-128 ASCII alphanumerics, `_`, `-` or `.`.
            assert!(tool.name.len() <= 128, "{}", tool.name);
            assert!(tool.name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)), "{}", tool.name);
            assert!(!tool.title.is_empty() && !tool.description.is_empty(), "{}", tool.name);
        }
        let methods: Vec<&str> = TOOLS.iter().filter_map(|tool| (tool.request)(&json!({})).ok().map(|(method, _)| method)).collect();
        assert_eq!(methods, ["engine.commands", "document.inspect"]);
    }

    #[test]
    fn list_commands_reads_the_catalog() {
        assert_eq!(list_commands(&json!({})), Ok(("engine.commands", json!({}))));
    }

    #[test]
    fn execute_passes_the_command_and_its_parameters() {
        let request = execute(&json!({"command": "solid.box", "params": {"length": 30}}));
        assert_eq!(request, Ok(("engine.execute", json!({"command": "solid.box", "params": {"length": 30}}))));
        assert_eq!(execute(&json!({"command": " view.fit "})), Ok(("engine.execute", json!({"command": "view.fit", "params": {}}))));
        assert!(execute(&json!({})).is_err());
        assert!(execute(&json!({"command": "solid.box", "params": [1]})).is_err());
    }

    #[test]
    fn describe_runs_the_catalog_command_that_documents_one() {
        let request = describe_command(&json!({"command": "solid.extrude"}));
        assert_eq!(request, Ok(("engine.execute", json!({"command": "engine.describe", "params": {"command": "solid.extrude"}}))));
        assert!(describe_command(&json!({})).is_err());
    }

    #[test]
    fn inspect_reads_the_design_and_measures_only_when_asked() {
        assert_eq!(inspect(&json!({})), Ok(("document.inspect", json!({}))));
        assert_eq!(inspect(&json!({"measure": true})), Ok(("document.inspect", json!({"measure": true}))));
        assert!(inspect(&json!({"measure": "yes"})).is_err());
    }

    #[test]
    fn batch_keeps_rollback_and_refuses_a_step_without_a_command() {
        let request = batch(&json!({"commands": [{"command": "view.fit"}], "rollback": false}));
        assert_eq!(request, Ok(("engine.batch", json!({"commands": [{"command": "view.fit"}], "rollback": false}))));
        assert!(batch(&json!({"commands": []})).is_err());
        assert!(batch(&json!({"commands": [{"params": {}}]})).is_err());
        assert!(batch(&json!({"commands": [{"command": "view.fit"}], "rollback": "no"})).is_err());
    }

    #[test]
    fn malformed_batch_steps_are_refused_before_any_command_runs() {
        for step in [json!({"command": " "}), json!({"command": "view.fit", "params": []}), json!({"command": "view.fit", "param": {}})] {
            let request =
                json!({"commands": [{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}, step], "rollback": false});
            let error = batch(&request).expect_err("invalid steps must refuse the entire request");
            assert!(error.contains("commands[1]"), "{error}");
        }
        let steps = vec![json!({"command": "view.fit"}); 10_001];
        assert!(batch(&json!({"commands": steps})).is_err());
    }

    #[test]
    fn arguments_follow_the_tool_schema_instead_of_ignoring_mistakes() {
        assert!(list_commands(&json!({"filter": "hole"})).is_err());
        assert!(describe_command(&json!({"command": "solid.box", "extra": true})).is_err());
        assert!(execute(&json!({"command": "solid.box", "param": {"length": 10}})).is_err());
        assert!(inspect(&json!({"measures": true})).is_err());
        assert!(batch(&json!({"commands": [{"command": "view.fit"}], "roll_back": false})).is_err());
        for input in [Value::Null, json!([]), json!("input"), json!(true), json!(1)] {
            for tool in TOOLS {
                assert!((tool.request)(&input).is_err(), "{} accepted {input}", tool.name);
            }
        }
    }

    /// The name the test and the docs use is the tool the page registers.
    #[test]
    fn the_tools_are_the_documented_set() {
        let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            ["solidcraft_list_commands", "solidcraft_describe_command", "solidcraft_execute", "solidcraft_batch", "solidcraft_inspect"]
        );
        assert!(tool("solidcraft_inspect").read_only);
        assert!(!tool("solidcraft_execute").read_only);
        assert!(!tool("solidcraft_batch").read_only);
    }
}

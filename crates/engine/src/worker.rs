//! A bounded, transient protocol for a browser's persistent engine worker.
//! The UI owns the session. Work is staged on a copy and accepted only against its original
//! document and authoring context. No command is replayed on the receiving UI thread.

use crate::{Session, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_doc::{Document, Mat, ModelTransfer, Visibility, display_quality::DisplayQuality};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::Arc;

pub const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_STEPS: usize = 10_000;

/// Linked designs and host policy callbacks need the window's storage/services. A worker
/// must not report missing links merely because it has a separate virtual filesystem.
pub fn supports_session(session: &Session) -> bool {
    session.command_check().is_none() && session.doc.links.is_empty()
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    encode_bounded(value, MAX_MESSAGE_BYTES)
}

fn encode_bounded<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Bounded { bytes: Vec::new(), limit, exceeded: false };
    let result = value.serialize(&mut rmp_serde::Serializer::new(&mut bytes).with_struct_map());
    if bytes.exceeded {
        return Err(format!("worker message exceeds its {limit}-byte limit"));
    }
    result.map_err(|e| format!("worker encoding: {e}"))?;
    Ok(bytes.bytes)
}

struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("worker message exceeds its byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("worker message exceeds 128 MiB".into());
    }
    rmp_serde::from_slice(bytes).map_err(|e| format!("invalid worker message: {e}"))
}

/// Only commands with no host/file/clipboard effects can run on the engine worker. Other
/// requests retain the existing control handlers. The same predicate is checked by the worker.
pub fn supports(method: &str, params: &Value) -> bool {
    if ["param_check", "allow_paths", "design_folders", "timings", "profile", "coalesce_parameters"].iter().any(|key| params.get(*key).is_some()) {
        return false;
    }
    match method {
        "document.inspect" => true,
        "engine.execute" | "command" => step(params).is_some_and(|(id, params)| command_supported(id, params)),
        "engine.batch" => params.get("commands").and_then(Value::as_array).is_some_and(|steps| {
            !steps.is_empty()
                && steps.len() <= MAX_STEPS
                && steps.iter().all(|s| {
                    step(s).is_some_and(|(id, params)| {
                        // History inside a mixed batch can close a host feature editor mid-batch.
                        !crate::find_command(id).is_some_and(|spec| matches!(spec.id, "edit.undo" | "edit.redo")) && command_supported(id, params)
                    })
                })
        }),
        _ => false,
    }
}

fn step(value: &Value) -> Option<(&str, &Value)> {
    let command = value.get("command").or_else(|| value.get("id")).and_then(Value::as_str)?;
    Some((command, value.get("params").filter(|p| !p.is_null()).unwrap_or(&Value::Null)))
}

fn command_supported(command: &str, params: &Value) -> bool {
    let Some(spec) = crate::find_command(command) else { return false };
    let id = spec.id;
    if matches!(id, "edit.undo" | "edit.redo" | "view.fit" | "view.home" | "inspect.measure" | "document.inspect") {
        return true;
    }
    if params.get("live").and_then(Value::as_bool) == Some(true)
        || params
            .as_object()
            .is_some_and(|p| p.keys().any(|k| ["path", "paths", "file", "files", "dir", "folder", "url", "out"].contains(&k.as_str())))
        || ["copy", "cut", "paste", "import", "export", "save", "library", "play", "edit_in_place", "finish_edit", "update_links"]
            .iter()
            .any(|part| id.contains(part))
    {
        return false;
    }
    matches!(
        id.split('.').next(),
        Some("solid" | "sketch" | "construct" | "parameters" | "timeline" | "sheet" | "plastic" | "surface" | "config" | "joint" | "component")
    )
}

/// A pinned source is cheap: documents, evaluated bodies and meshes are shared by Arc.
pub struct Source(Session);

impl Source {
    pub fn new(session: &Session) -> Self {
        Self(replica(session))
    }

    pub fn is_current(&self, session: &Session) -> bool {
        let source = &self.0;
        session.uid == source.uid
            && session.revision == source.revision
            && session.same_document_context(source)
            && session.selection == source.selection
            && Arc::ptr_eq(&session.saved, &source.saved)
            && session.saved_visibility == source.saved_visibility
            && session.auto_project == source.auto_project
            && session.param_check == source.param_check
            && session.validation() == source.validation()
    }

    pub fn same_design(&self, session: &Session) -> bool {
        self.0.uid == session.uid
    }

    pub fn request(&self, method: &str, params: Value, synchronize: bool, quality: DisplayQuality) -> Result<Vec<u8>, String> {
        if !supports_session(&self.0) {
            return Err("linked designs and host command checks require the application's control handler".into());
        }
        if !supports(method, &params) {
            return Err("request needs the application's control handler".into());
        }
        let source = synchronize.then(|| State::capture(&self.0, &[], None, &[]));
        encode(&Request { source, method: method.into(), params, quality: quality.id().into(), revision: self.0.revision })
    }

    /// Validate and adopt a completed transaction, retaining the live file, saved state, camera,
    /// checkpoints and host services. The document is untouched if decoding or freshness fails.
    pub fn accept(self, session: &mut Session, reply: Reply) -> Result<Completion, String> {
        if !self.is_current(session) {
            return Err("the design changed while the agent command computed; nothing was applied".into());
        }
        let mut completions = reply.effects;
        if let Some(state) = reply.state {
            let (computed, traces) = state.restore(Some(&self.0), false)?;
            for trace in &traces {
                crate::crash::accepted_command(session, &trace.before, &trace.after, &trace.id, &trace.params, trace.changed);
                completions.push((trace.id.clone(), trace.reply.clone()));
            }
            let previous = session.revision;
            session.doc = computed.doc;
            session.model = computed.model;
            session.undo = computed.undo;
            session.redo = computed.redo;
            session.active_sketch = computed.active_sketch;
            session.sketch_entry = computed.sketch_entry;
            session.selection = computed.selection;
            session.active_component = computed.active_component;
            session.active_occurrence_path = computed.active_occurrence_path;
            session.auto_project = computed.auto_project;
            session.pending_moves = computed.pending_moves;
            session.exploded = computed.exploded;
            session.visibility = computed.visibility;
            session.live_links = None;
            session.joints_settled = None;
            session.world_cache = Default::default();
            session.dirty_cache = Default::default();
            session.revision = computed.revision;
            session.invalidate_view(previous);
            for message in computed.log {
                session.echo(message);
            }
        }
        Ok(Completion { result: reply.result, commands: completions, compute_ms: reply.compute_ms })
    }
}

fn replica(session: &Session) -> Session {
    let mut copy = session.scratch();
    copy.undo = session.undo.clone();
    copy.redo = session.redo.clone();
    copy.sketch_entry = session.sketch_entry.clone();
    copy.path = session.path.clone();
    copy.crash_journal = false;
    copy
}

#[derive(Serialize, Deserialize)]
struct Request {
    source: Option<State>,
    method: String,
    params: Value,
    quality: String,
    revision: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    result: Result<Value, String>,
    state: Option<State>,
    compute_ms: f64,
    effects: Vec<(String, Value)>,
}

impl Reply {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        decode(bytes)
    }
}

pub struct Completion {
    pub result: Result<Value, String>,
    pub commands: Vec<(String, Value)>,
    pub compute_ms: f64,
}

/// A single reusable engine. The page serializes calls and resets it whenever the source no
/// longer matches, so repeated calls retain evaluated geometry and skip document transfer.
#[derive(Default)]
pub struct Runtime {
    session: Session,
    initialized: bool,
}

impl Runtime {
    pub fn run(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.run_bounded(bytes, MAX_MESSAGE_BYTES)
    }

    pub(crate) fn run_bounded(&mut self, bytes: &[u8], reply_limit: usize) -> Result<Vec<u8>, String> {
        let request: Request = decode(bytes)?;
        if !supports(&request.method, &request.params) {
            return Err("request needs the application's control handler".into());
        }
        let quality = DisplayQuality::parse(&request.quality).ok_or("invalid worker display quality")?;
        if let Some(state) = request.source {
            self.session = state.restore(None, true)?.0;
            self.initialized = true;
        }
        if !self.initialized {
            return Err("worker needs a source session".into());
        }
        self.session.revision = request.revision;
        let clock = crate::timing::Clock::start();
        let before = replica(&self.session);
        let mut staged = replica(&self.session);
        let mut traces = Vec::new();
        let result = match request.method.as_str() {
            "document.inspect" => staged.execute("document.inspect", &request.params).map_err(|e| e.to_string()),
            "engine.batch" => batch(&mut staged, &request.params, &mut traces),
            _ => {
                let (id, params) = step(&request.params).ok_or("missing command")?;
                run(&mut staged, id, params, &mut traces)
            }
        };
        let changed = !Arc::ptr_eq(&before.doc, &staged.doc)
            || before.revision != staged.revision
            || before.active_sketch != staged.active_sketch
            || before.selection != staged.selection
            || before.undo.len() != staged.undo.len()
            || before.redo.len() != staged.redo.len()
            || before.auto_project != staged.auto_project
            || before.active_component != staged.active_component
            || before.active_occurrence_path != staged.active_occurrence_path
            || before.pending_moves != staged.pending_moves
            || before.visibility != staged.visibility
            || before.exploded != staged.exploded;
        let state = if changed {
            let model = ModelTransfer::capture(&staged.model, &staged.doc, quality).map_err(|e| e.to_string())?;
            Some(State::capture(&staged, &documents(&before), Some(model), &traces))
        } else {
            None
        };
        let effects = if changed { Vec::new() } else { traces.into_iter().map(|trace| (trace.id, trace.reply)).collect() };
        let bytes = encode_bounded(&Reply { result, state, effects, compute_ms: clock.ms() }, reply_limit)?;
        self.session = staged;
        Ok(bytes)
    }
}

struct Trace {
    id: String,
    params: Value,
    before: Arc<Document>,
    after: Arc<Document>,
    changed: bool,
    reply: Value,
}

fn run(session: &mut Session, id: &str, params: &Value, traces: &mut Vec<Trace>) -> Result<Value, String> {
    let params = if params.is_null() { json!({}) } else { params.clone() };
    let before = session.doc.clone();
    let revision = session.revision;
    let reply = session.execute(id, &params).map_err(|e| e.to_string())?;
    // Reading requests need no model transfer; view.fit/home still need their host effect.
    if revision != session.revision || matches!(id, "view.fit" | "view.home") {
        traces.push(Trace { id: id.into(), params, before, after: session.doc.clone(), changed: revision != session.revision, reply: reply.clone() });
    }
    Ok(reply)
}

fn batch(session: &mut Session, params: &Value, traces: &mut Vec<Trace>) -> Result<Value, String> {
    let steps = params.get("commands").and_then(Value::as_array).filter(|s| s.len() <= MAX_STEPS).ok_or("invalid batch commands")?;
    if let Some(refusal) = crate::batch_preflight::check(steps, session.param_check, &[]) {
        return Ok(refusal.reply(steps.len()));
    }
    let rollback = params.get("rollback").and_then(Value::as_bool).unwrap_or(true);
    let checkpoint = rollback.then(|| session.checkpoint());
    let mut results = Vec::new();
    for (i, command) in steps.iter().enumerate() {
        let result = step(command).ok_or_else(|| "missing command".to_string()).and_then(|(id, params)| run(session, id, params, traces));
        match result {
            Ok(reply) => results.push(reply),
            Err(error) => {
                let restored = checkpoint.is_some_and(|checkpoint| session.restore_checkpoint(checkpoint).is_ok());
                if restored {
                    traces.clear();
                }
                return Ok(
                    json!({"completed":i,"results":results,"failed":{"index":i,"command":step(command).map(|s| s.0),"error":error},"undone":if restored { i } else { 0 },"rolled_back":restored}),
                );
            }
        }
    }
    if let Some(checkpoint) = checkpoint {
        session.drop_checkpoint(checkpoint);
    }
    Ok(json!({"completed":results.len(),"results":results,"undone":0}))
}

/// Documents are referenced by index: undo/redo and the sketch-entry checkpoint share them.
/// Replies refer to the pinned source's documents and send only newly created versions.
struct DocumentTable {
    docs: Vec<Arc<Document>>,
    indices: HashMap<usize, usize>,
    known: usize,
}

impl DocumentTable {
    fn new(known: &[Arc<Document>]) -> Self {
        let indices = known.iter().enumerate().map(|(i, doc)| (Arc::as_ptr(doc).addr(), i)).collect();
        Self { docs: known.to_vec(), indices, known: known.len() }
    }
    fn index(&mut self, doc: &Arc<Document>) -> usize {
        let key = Arc::as_ptr(doc).addr();
        if let Some(i) = self.indices.get(&key) {
            return *i;
        }
        let index = self.docs.len();
        self.docs.push(doc.clone());
        self.indices.insert(key, index);
        index
    }
}

fn documents(session: &Session) -> Vec<Arc<Document>> {
    let mut table = DocumentTable::new(&[]);
    table.index(&session.doc);
    table.index(&session.saved);
    for snapshot in session.undo.iter().chain(&session.redo) {
        table.index(&snapshot.doc);
    }
    if let Some(entry) = &session.sketch_entry {
        table.index(&entry.doc);
        if let Some(doc) = &entry.undo_top {
            table.index(doc);
        }
    }
    table.docs
}

#[derive(Serialize, Deserialize)]
struct History {
    label: String,
    doc: usize,
    active_sketch: Option<u64>,
    pending_moves: BTreeMap<u64, Mat>,
    visibility: Visibility,
}

impl History {
    fn of(snapshot: &Snapshot, table: &mut DocumentTable) -> Self {
        Self {
            label: snapshot.label.clone(),
            doc: table.index(&snapshot.doc),
            active_sketch: snapshot.active_sketch,
            pending_moves: snapshot.pending_moves.clone(),
            visibility: snapshot.visibility.clone(),
        }
    }
    fn restore(self, docs: &[Arc<Document>]) -> Result<Snapshot, String> {
        Ok(Snapshot {
            label: self.label,
            doc: document(docs, self.doc)?,
            active_sketch: self.active_sketch,
            pending_moves: self.pending_moves,
            visibility: self.visibility,
        })
    }
}

fn document(docs: &[Arc<Document>], index: usize) -> Result<Arc<Document>, String> {
    docs.get(index).cloned().ok_or_else(|| "invalid worker document reference".into())
}

#[derive(Serialize, Deserialize)]
struct SketchEntry {
    sketch: u64,
    doc: usize,
    undo_len: usize,
    undo_top: Option<usize>,
    restore_marker: Option<Option<usize>>,
    pending_moves: BTreeMap<u64, Mat>,
    visibility: Visibility,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    id: String,
    params: Value,
    before: usize,
    after: usize,
    changed: bool,
    reply: Value,
}

#[derive(Serialize, Deserialize)]
struct State {
    documents: Vec<Arc<Document>>,
    doc: usize,
    saved: usize,
    saved_visibility: Visibility,
    document_id: String,
    undo: Vec<History>,
    redo: Vec<History>,
    entry: Option<SketchEntry>,
    active_sketch: Option<u64>,
    selection: Vec<crate::Sel>,
    active_component: u64,
    active_occurrence_path: Option<Vec<u64>>,
    pending_moves: BTreeMap<u64, Mat>,
    exploded: BTreeMap<u64, Mat>,
    visibility: Visibility,
    revision: u64,
    auto_project: bool,
    param_check: String,
    validation: String,
    path: Option<String>,
    log: Vec<String>,
    model: Option<ModelTransfer>,
    journal: Vec<Journal>,
}

impl State {
    fn capture(session: &Session, known: &[Arc<Document>], model: Option<ModelTransfer>, traces: &[Trace]) -> Self {
        let mut table = DocumentTable::new(known);
        let doc = table.index(&session.doc);
        let saved = table.index(&session.saved);
        let undo = session.undo.iter().map(|s| History::of(s, &mut table)).collect();
        let redo = session.redo.iter().map(|s| History::of(s, &mut table)).collect();
        let entry = session.sketch_entry.as_ref().map(|entry| SketchEntry {
            sketch: entry.sketch,
            doc: table.index(&entry.doc),
            undo_len: entry.undo_len,
            undo_top: entry.undo_top.as_ref().map(|doc| table.index(doc)),
            restore_marker: entry.restore_marker,
            pending_moves: entry.pending_moves.clone(),
            visibility: entry.visibility.clone(),
        });
        let journal = traces
            .iter()
            .map(|trace| Journal {
                id: trace.id.clone(),
                params: trace.params.clone(),
                before: table.index(&trace.before),
                after: table.index(&trace.after),
                changed: trace.changed,
                reply: trace.reply.clone(),
            })
            .collect();
        Self {
            documents: table.docs.into_iter().skip(table.known).collect(),
            doc,
            saved,
            saved_visibility: session.saved_visibility.clone(),
            document_id: session.document_id.clone(),
            undo,
            redo,
            entry,
            active_sketch: session.active_sketch,
            selection: session.selection.clone(),
            active_component: session.active_component,
            active_occurrence_path: session.active_occurrence_path.clone(),
            pending_moves: session.pending_moves.clone(),
            exploded: session.exploded.clone(),
            visibility: session.visibility.clone(),
            revision: session.revision,
            auto_project: session.auto_project,
            param_check: session.param_check.name().into(),
            validation: session.validation().id().into(),
            path: session.path.clone(),
            log: session.log.clone(),
            model,
            journal,
        }
    }

    fn restore(self, source: Option<&Session>, evaluate: bool) -> Result<(Session, Vec<Trace>), String> {
        if self.undo.len() > crate::MAX_UNDO || self.redo.len() > crate::MAX_UNDO || self.journal.len() > MAX_STEPS {
            return Err("worker history exceeds its limit".into());
        }
        let mut docs = source.map(documents).unwrap_or_default();
        docs.extend(self.documents);
        let doc = document(&docs, self.doc)?;
        let mut session = source.map(replica).unwrap_or_default();
        session.doc = doc;
        session.saved = document(&docs, self.saved)?;
        session.saved_visibility = self.saved_visibility;
        session.document_id = self.document_id;
        session.undo = self.undo.into_iter().map(|s| s.restore(&docs)).collect::<Result<_, _>>()?;
        session.redo = self.redo.into_iter().map(|s| s.restore(&docs)).collect::<Result<_, _>>()?;
        session.sketch_entry = self
            .entry
            .map(|entry| {
                Ok::<_, String>(crate::SketchEntry {
                    sketch: entry.sketch,
                    doc: document(&docs, entry.doc)?,
                    undo_len: entry.undo_len,
                    undo_top: entry.undo_top.map(|i| document(&docs, i)).transpose()?,
                    restore_marker: entry.restore_marker,
                    pending_moves: entry.pending_moves,
                    visibility: entry.visibility,
                })
            })
            .transpose()?;
        session.active_sketch = self.active_sketch;
        session.selection = self.selection;
        session.active_component = self.active_component;
        session.active_occurrence_path = self.active_occurrence_path;
        session.pending_moves = self.pending_moves;
        session.exploded = self.exploded;
        session.visibility = self.visibility;
        session.revision = self.revision;
        session.auto_project = self.auto_project;
        session.param_check = crate::schema::ParamCheck::parse(&self.param_check).ok_or("invalid worker parameter policy")?;
        session.path = self.path;
        session.log = self.log;
        session.crash_journal = false;
        let validation = crate::kernel::Validation::parse(&self.validation).ok_or("invalid worker validation policy")?;
        session.model = match self.model {
            Some(model) => model.restore(&session.doc).map_err(|e| e.to_string())?,
            None if evaluate => {
                let mut model = solidcraft_doc::Model::default();
                model.set_validation(validation);
                model.evaluate(&session.doc);
                model
            }
            None => return Err("worker reply omitted its evaluated model".into()),
        };
        let traces = self
            .journal
            .into_iter()
            .map(|trace| {
                Ok(Trace {
                    id: trace.id,
                    params: trace.params,
                    before: document(&docs, trace.before)?,
                    after: document(&docs, trace.after)?,
                    changed: trace.changed,
                    reply: trace.reply,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok((session, traces))
    }
}

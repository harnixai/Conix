//! `script` gates: a script part run in the library's own QuickJS (docs/design/flows.md §2.3, in
//! Conix).
//!
//! Not through the application's engine: a gate is read-only and deterministic, and a script run
//! there would go through that engine's tool pipeline (permissions, hooks, middleware) and judge
//! differently by the session's settings. One runtime a call, thrown away after; what the script
//! gets is `flow`, read-only, its paths held to the run's workspace by [`Reader`], every file it
//! reads recorded as evidence whatever it says itself.
//!
//! The native functions take and give JSON text, wrapped by a few lines of JavaScript ([`PRELUDE`]):
//! no QuickJS value crosses into Rust.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{Ctx, Function, Module, Object, Value as JsValue};
use serde_json::Value;

use super::files::{parse, OutOfBounds, Reader};
use crate::kernel::host::GateResult;
use crate::kernel::parts::ReadAs;

/// The memory one script may use.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// The largest result a script may give.
const MAX_RESULT: usize = 1024 * 1024;
/// The most a script's `flow.log` keeps.
const MAX_LOG: usize = 64 * 1024;
/// How long past its deadline a script that does not stop is waited for.
const ABANDON_AFTER: Duration = Duration::from_secs(2);

/// What the script gets as `flow`. The natives are `globalThis.__flow`, removed once wrapped.
const PRELUDE: &str = r#"(() => {
  const n = globalThis.__flow;
  delete globalThis.__flow;
  const j = (s) => (s == null ? null : JSON.parse(s));
  const read = (as) => (p) => j(n.read(String(p), as));
  return Object.freeze({
    read: Object.freeze({
      text: (p) => { const s = n.text(String(p)); return s == null ? null : s; },
      json: read("json"),
      yaml: read("yaml"),
      jsonl: read("jsonl"),
      frontmatter: read("frontmatter"),
    }),
    glob: (p) => JSON.parse(n.glob(String(p))),
    stat: (p) => j(n.stat(String(p))),
    schema: Object.freeze({
      validate: (value, schema) => JSON.parse(n.validate(JSON.stringify(value === undefined ? null : value), JSON.stringify(schema))),
    }),
    log: (...a) => n.log(a.map((x) => (typeof x === "string" ? x : JSON.stringify(x))).join(" ")),
    ...(n.write ? {
      write: Object.freeze({
        text: (p, s) => n.write(String(p), String(s), "text"),
        json: (p, v) => n.write(String(p), JSON.stringify(v, null, 2) + "\n", "text"),
        yaml: (p, v) => n.write(String(p), JSON.stringify(v), "yaml"),
        jsonl: (p, rows) => n.write(String(p), rows.map((r) => JSON.stringify(r)).join("\n") + (rows.length ? "\n" : ""), "text"),
      }),
      append: Object.freeze({
        text: (p, s) => n.append(String(p), String(s)),
        jsonl: (p, row) => n.append(String(p), JSON.stringify(row) + "\n"),
      }),
      remove: (p) => n.remove(String(p)),
      print: (...a) => n.print(a.map((x) => (typeof x === "string" ? x : JSON.stringify(x, null, 2))).join(" ")),
    } : {}),
  });
})()"#;

/// What a run of a script left behind besides its result.
#[derive(Default)]
pub(crate) struct Trace {
    pub log: String,
    /// What a command printed (`flow.print`).
    pub out: String,
    /// The first path the script tried to read out of bounds: the gate is an `error` even if the
    /// script caught it.
    pub out_of_bounds: Option<String>,
}

/// Run `source` (an ES module whose default export is `function (input, flow)`) on `input`.
pub(crate) async fn run(source: String, input: Value, reader: Arc<Reader>, timeout: Duration) -> (GateResult, Trace) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let t = trace.clone();
    let deadline = Instant::now() + timeout;
    let r = reader.clone();
    let joined = tokio::time::timeout(
        timeout + ABANDON_AFTER,
        tokio::task::spawn_blocking(move || blocking(&source, &input, r, t, deadline, timeout)),
    )
    .await;
    let mut out = match joined {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => GateResult::errored(format!("the script did not finish: {e}")),
        Err(_) => GateResult::errored(format!("the script ran longer than {}s and did not stop", timeout.as_secs())),
    };
    let trace = std::mem::take(&mut *trace.lock().unwrap());
    if let Some(p) = &trace.out_of_bounds {
        if out.verdict != crate::kernel::run::GateVerdict::Error {
            out = GateResult::errored(format!("the script read out of bounds: {p}"));
        }
    }
    // The evidence is what was read, not what the script says.
    out.evidence = reader.evidence();
    (out, trace)
}

fn blocking(
    source: &str,
    input: &Value,
    reader: Arc<Reader>,
    trace: Arc<Mutex<Trace>>,
    deadline: Instant,
    timeout: Duration,
) -> GateResult {
    let runtime = match rquickjs::Runtime::new() {
        Ok(r) => r,
        Err(e) => return GateResult::errored(format!("no script runtime: {e}")),
    };
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = match rquickjs::Context::full(&runtime) {
        Ok(c) => c,
        Err(e) => return GateResult::errored(format!("no script context: {e}")),
    };
    context.with(|ctx| {
        let fail = |e: rquickjs::Error| {
            if Instant::now() >= deadline {
                GateResult::errored(format!("the script ran longer than {}s", timeout.as_secs()))
            } else {
                GateResult::errored(format!("the script failed: {}", message(&ctx, e)))
            }
        };
        let flow = match install(&ctx, reader, trace) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
        let main: Function = match evaluate(&ctx, source) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
        let input = match ctx.json_parse(input.to_string()) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let got: JsValue = match main.call((input, flow)) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let got = match got.as_promise() {
            Some(p) => match p.finish::<JsValue>() {
                Ok(v) => v,
                Err(rquickjs::Error::WouldBlock) => {
                    return GateResult::errored("the script waits for something that never comes (it may only read)")
                }
                Err(e) => return fail(e),
            },
            None => got,
        };
        let text = match ctx.json_stringify(got) {
            Ok(Some(s)) => match s.to_string() {
                Ok(s) => s,
                Err(e) => return fail(e),
            },
            Ok(None) => return GateResult::errored("the script returned nothing: it returns a gate result"),
            Err(e) => return fail(e),
        };
        if text.len() > MAX_RESULT {
            return GateResult::errored(format!("the script's result is larger than {} KiB", MAX_RESULT / 1024));
        }
        match serde_json::from_str::<GateResult>(&text) {
            Ok(r) => r,
            Err(e) => GateResult::errored(format!("the script's result is not a gate result: {e}")),
        }
    })
}

/// Declare and evaluate the module, and hand back its default export.
fn evaluate<'js>(ctx: &Ctx<'js>, source: &str) -> rquickjs::Result<Function<'js>> {
    let (module, done) = Module::declare(ctx.clone(), "gate.js", source)?.eval()?;
    done.finish::<()>()?;
    module.get::<_, Function>("default")
}

/// What is wrong with a script's text as a module (its syntax); empty when it may be saved.
pub(crate) fn problems(source: &str) -> Vec<String> {
    let Ok(runtime) = rquickjs::Runtime::new() else { return vec![] };
    let Ok(context) = rquickjs::Context::full(&runtime) else { return vec![] };
    context.with(|ctx| match Module::declare(ctx.clone(), "gate.js", source) {
        Ok(_) => {
            if source.contains("export default") {
                vec![]
            } else {
                vec!["a gate script exports its function as default (`export default function (input, flow) { … }`)".into()]
            }
        }
        Err(e) => vec![format!("not a module: {}", message(&ctx, e))],
    })
}

fn message(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    if let rquickjs::Error::Exception = e {
        let caught = ctx.catch();
        if let Some(ex) = caught.as_exception() {
            let m = ex.message().unwrap_or_else(|| "uncaught exception".into());
            return match ex.stack().filter(|s| !s.trim().is_empty()) {
                Some(stack) => format!("{m}\n{}", stack.trim_end()),
                None => m,
            };
        }
        if let Ok(Some(s)) = ctx.json_stringify(caught) {
            if let Ok(s) = s.to_string() {
                return format!("threw {s}");
            }
        }
    }
    e.to_string()
}

fn install<'js>(ctx: &Ctx<'js>, reader: Arc<Reader>, trace: Arc<Mutex<Trace>>) -> rquickjs::Result<JsValue<'js>> {
    install_with(ctx, reader, trace, None)
}

fn install_with<'js>(ctx: &Ctx<'js>, reader: Arc<Reader>, trace: Arc<Mutex<Trace>>, writer: Option<Arc<Writer>>) -> rquickjs::Result<JsValue<'js>> {
    let n = Object::new(ctx.clone())?;
    if let Some(w) = writer {
        let thrown = |ctx: &Ctx<'_>, e: String| rquickjs::Exception::throw_message(ctx, &e);
        {
            let w = w.clone();
            n.set("write", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String, text: String, as_: String| -> rquickjs::Result<()> {
                let text = if as_ == "yaml" {
                    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    serde_yaml::to_string(&v).map_err(|e| thrown(&ctx, e.to_string()))?
                } else {
                    text
                };
                w.write(&p, &text, false).map_err(|e| thrown(&ctx, e))
            })?)?;
        }
        {
            let w = w.clone();
            n.set("append", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String, text: String| -> rquickjs::Result<()> {
                w.write(&p, &text, true).map_err(|e| thrown(&ctx, e))
            })?)?;
        }
        {
            let w = w.clone();
            n.set("remove", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String| -> rquickjs::Result<bool> {
                w.remove(&p).map_err(|e| thrown(&ctx, e))
            })?)?;
        }
        {
            let trace = trace.clone();
            n.set("print", Function::new(ctx.clone(), move |line: String| {
                let mut t = trace.lock().unwrap();
                if t.out.len() < MAX_RESULT {
                    t.out.push_str(&line);
                    t.out.push('\n');
                }
            })?)?;
        }
    }
    // A path out of bounds throws, and is remembered: the gate is an error even if caught.
    let oob = {
        let trace = trace.clone();
        move |ctx: &Ctx<'_>, e: OutOfBounds| -> rquickjs::Error {
            let mut t = trace.lock().unwrap();
            if t.out_of_bounds.is_none() {
                t.out_of_bounds = Some(e.0.clone());
            }
            rquickjs::Exception::throw_message(ctx, &e.0)
        }
    };
    {
        let (r, oob) = (reader.clone(), oob.clone());
        n.set("text", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String| -> rquickjs::Result<Option<String>> {
            r.text(&p).map(|o| o.map(|(_, t)| t)).map_err(|e| oob(&ctx, e))
        })?)?;
    }
    {
        let (r, oob) = (reader.clone(), oob.clone());
        n.set("read", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String, as_: String| -> rquickjs::Result<Option<String>> {
            let read_as = match as_.as_str() {
                "json" => ReadAs::Json,
                "yaml" => ReadAs::Yaml,
                "jsonl" => ReadAs::Jsonl,
                _ => ReadAs::Frontmatter,
            };
            let Some((rel, text)) = r.text(&p).map_err(|e| oob(&ctx, e))? else { return Ok(None) };
            match parse(&text, read_as) {
                Ok(v) => Ok(Some(v.to_string())),
                Err(why) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{rel}: {why}"))),
            }
        })?)?;
    }
    {
        let (r, oob) = (reader.clone(), oob.clone());
        n.set("glob", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String| -> rquickjs::Result<String> {
            r.glob(&p).map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())).map_err(|e| oob(&ctx, e))
        })?)?;
    }
    {
        let (r, oob) = (reader.clone(), oob.clone());
        n.set("stat", Function::new(ctx.clone(), move |ctx: Ctx<'_>, p: String| -> rquickjs::Result<Option<String>> {
            r.stat(&p)
                .map(|o| o.map(|e| serde_json::json!({"size": e.size, "sha256": e.sha256}).to_string()))
                .map_err(|e| oob(&ctx, e))
        })?)?;
    }
    n.set("validate", Function::new(ctx.clone(), |ctx: Ctx<'_>, value: String, schema: String| -> rquickjs::Result<String> {
        let value: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
        let schema: Value = serde_json::from_str(&schema)
            .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("the schema is not JSON: {e}")))?;
        let v = jsonschema::validator_for(&schema)
            .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("the schema does not compile: {e}")))?;
        let errors: Vec<String> = v
            .iter_errors(&value)
            .map(|e| {
                let at = e.instance_path().to_string();
                if at.is_empty() { e.to_string() } else { format!("{at}: {e}") }
            })
            .collect();
        Ok(serde_json::json!({"ok": errors.is_empty(), "errors": errors}).to_string())
    })?)?;
    {
        let trace = trace.clone();
        n.set("log", Function::new(ctx.clone(), move |line: String| {
            let mut t = trace.lock().unwrap();
            if t.log.len() < MAX_LOG {
                t.log.push_str(&line);
                t.log.push('\n');
            }
        })?)?;
    }
    ctx.globals().set("__flow", n)?;
    ctx.eval::<JsValue, _>(PRELUDE)
}

// ── a bundle's commands (in Conix: docs/design/layout.md §2) ─────────────────

/// Where a command may write: paths relative to the run's workspace, each inside one of `roots`
/// (the run's own directory and its bundle's data directory), never inside `denied` (the run's
/// process directory). Links are followed before judging, as far as the path exists.
pub(crate) struct Writer {
    pub workspace: std::path::PathBuf,
    pub roots: Vec<std::path::PathBuf>,
    pub denied: Vec<std::path::PathBuf>,
    /// What was written, relative to the workspace, in order: told back to the session.
    pub written: Mutex<Vec<String>>,
}

impl Writer {
    fn target(&self, rel: &str) -> Result<std::path::PathBuf, String> {
        use std::path::Component;
        let p = std::path::Path::new(rel);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!("{rel}: `..` is not written through"));
        }
        let full = if p.is_absolute() { p.to_path_buf() } else { self.workspace.join(p) };
        // The deepest part that exists, resolved: a link there cannot lead the write out.
        let mut existing = full.clone();
        let mut rest = vec![];
        while !existing.exists() {
            let Some(name) = existing.file_name().map(|n| n.to_os_string()) else { break };
            rest.push(name);
            if !existing.pop() {
                break;
            }
        }
        let mut real = existing.canonicalize().map_err(|e| format!("{rel}: {e}"))?;
        for n in rest.iter().rev() {
            real.push(n);
        }
        let canon = |r: &std::path::PathBuf| r.canonicalize().unwrap_or_else(|_| r.clone());
        if self.denied.iter().map(canon).any(|d| real.starts_with(&d)) {
            return Err(format!("{rel}: the run's process directory is the flow's, not the command's"));
        }
        if !self.roots.iter().map(canon).any(|r| real.starts_with(&r)) {
            return Err(format!(
                "{rel}: a command writes only in the run's directory and its flow's data directory ({})",
                self.roots.iter().map(|r| r.strip_prefix(&self.workspace).unwrap_or(r).display().to_string()).collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(real)
    }

    fn write(&self, rel: &str, text: &str, append: bool) -> Result<(), String> {
        use std::io::Write;
        let path = self.target(rel)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{rel}: {e}"))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&path)
            .map_err(|e| format!("{rel}: {e}"))?;
        f.write_all(text.as_bytes()).map_err(|e| format!("{rel}: {e}"))?;
        self.written.lock().unwrap().push(rel.to_string());
        Ok(())
    }

    fn remove(&self, rel: &str) -> Result<bool, String> {
        let path = self.target(rel)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.written.lock().unwrap().push(rel.to_string());
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("{rel}: {e}")),
        }
    }
}

/// What a command answered: what it printed and returned, or why it failed; the files it wrote.
pub(crate) struct CommandOutcome {
    pub ok: bool,
    pub text: String,
    pub written: Vec<String>,
    /// What it returned, as a value (`null` when it returned nothing or failed): a command step's
    /// output.
    pub value: Value,
}

/// Run a bundle's command: `source` an ES module whose default export is `function (input, flow)`,
/// `flow` as a gate script's plus `write`, `append`, `remove` and `print`. What it returns (a string,
/// or a value shown as JSON) follows what it printed; a throw is a failure.
pub(crate) async fn command(source: String, input: Value, reader: Arc<Reader>, writer: Arc<Writer>, timeout: Duration) -> CommandOutcome {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (t, w) = (trace.clone(), writer.clone());
    let deadline = Instant::now() + timeout;
    let joined = tokio::time::timeout(
        timeout + ABANDON_AFTER,
        tokio::task::spawn_blocking(move || run_command(&source, &input, reader, w, t, deadline, timeout)),
    )
    .await;
    let result = match joined {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(format!("the command did not finish: {e}")),
        Err(_) => Err(format!("the command ran longer than {}s and did not stop", timeout.as_secs())),
    };
    let trace = std::mem::take(&mut *trace.lock().unwrap());
    let written = std::mem::take(&mut *writer.written.lock().unwrap());
    let mut text = trace.out;
    let mut value = Value::Null;
    let ok = match result {
        Ok(v) => {
            value = v.clone();
            match v {
                Value::Null => {}
                Value::String(s) => text.push_str(&s),
                other => text.push_str(&serde_json::to_string_pretty(&other).unwrap_or_default()),
            }
            if let Some(p) = &trace.out_of_bounds {
                text.push_str(&format!("\n(read out of bounds: {p})"));
            }
            true
        }
        Err(e) => {
            text.push_str(&e);
            false
        }
    };
    CommandOutcome { ok, text: text.trim_end().to_string(), written, value }
}

/// Run a read-only script for the value it returns (`foreach: {script}`): `flow` as a gate
/// script's, the value whatever JSON it gives. A throw, a time-out or a read out of bounds is an
/// `Err`.
pub(crate) async fn value(source: String, input: Value, reader: Arc<Reader>, timeout: Duration) -> Result<Value, String> {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let t = trace.clone();
    let deadline = Instant::now() + timeout;
    let joined = tokio::time::timeout(
        timeout + ABANDON_AFTER,
        tokio::task::spawn_blocking(move || {
            let runtime = rquickjs::Runtime::new().map_err(|e| format!("no script runtime: {e}"))?;
            runtime.set_memory_limit(MEMORY_LIMIT);
            runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
            let context = rquickjs::Context::full(&runtime).map_err(|e| format!("no script context: {e}"))?;
            context.with(|ctx| {
                let fail = |e: rquickjs::Error| {
                    if Instant::now() >= deadline {
                        format!("the script ran longer than {}s", timeout.as_secs())
                    } else {
                        format!("the script failed: {}", message(&ctx, e))
                    }
                };
                let flow = install(&ctx, reader, t).map_err(fail)?;
                let main: Function = evaluate(&ctx, &source).map_err(fail)?;
                let input = ctx.json_parse(input.to_string()).map_err(fail)?;
                let got: JsValue = main.call((input, flow)).map_err(fail)?;
                let got = match got.as_promise() {
                    Some(p) => p.finish::<JsValue>().map_err(fail)?,
                    None => got,
                };
                match ctx.json_stringify(got).map_err(fail)?.map(|s| s.to_string()).transpose().map_err(fail)? {
                    Some(t) if t.len() > MAX_RESULT => Err(format!("the script's result is larger than {} KiB", MAX_RESULT / 1024)),
                    Some(t) => serde_json::from_str(&t).map_err(|e| format!("the script's result is not JSON: {e}")),
                    None => Err("the script returned nothing".to_string()),
                }
            })
        }),
    )
    .await;
    let out = match joined {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(format!("the script did not finish: {e}")),
        Err(_) => Err(format!("the script ran longer than {}s and did not stop", timeout.as_secs())),
    };
    if let Some(p) = &trace.lock().unwrap().out_of_bounds {
        return Err(format!("the script read out of bounds: {p}"));
    }
    out
}

fn run_command(
    source: &str,
    input: &Value,
    reader: Arc<Reader>,
    writer: Arc<Writer>,
    trace: Arc<Mutex<Trace>>,
    deadline: Instant,
    timeout: Duration,
) -> Result<Value, String> {
    let runtime = rquickjs::Runtime::new().map_err(|e| format!("no script runtime: {e}"))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = rquickjs::Context::full(&runtime).map_err(|e| format!("no script context: {e}"))?;
    context.with(|ctx| {
        let fail = |e: rquickjs::Error| {
            if Instant::now() >= deadline {
                format!("the command ran longer than {}s", timeout.as_secs())
            } else {
                message(&ctx, e)
            }
        };
        let flow = install_with(&ctx, reader, trace, Some(writer)).map_err(fail)?;
        let main: Function = evaluate(&ctx, source).map_err(fail)?;
        let input = ctx.json_parse(input.to_string()).map_err(fail)?;
        let got: JsValue = main.call((input, flow)).map_err(fail)?;
        let got = match got.as_promise() {
            Some(p) => p.finish::<JsValue>().map_err(fail)?,
            None => got,
        };
        if got.is_undefined() || got.is_null() {
            return Ok(Value::Null);
        }
        let text = ctx.json_stringify(got).map_err(fail)?.map(|s| s.to_string()).transpose().map_err(fail)?;
        match text {
            Some(t) if t.len() > MAX_RESULT => Err(format!("the command's answer is larger than {} KiB", MAX_RESULT / 1024)),
            Some(t) => Ok(serde_json::from_str(&t).unwrap_or(Value::String(t))),
            None => Ok(Value::Null),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::run::GateVerdict;
    use serde_json::json;

    async fn judge(source: &str, files: &[(&str, &str)], timeout: u64) -> (GateResult, Trace) {
        let d = tempfile::tempdir().unwrap();
        for (p, t) in files {
            std::fs::write(d.path().join(p), t).unwrap();
        }
        let reader = Arc::new(Reader::new(d.path()).unwrap());
        run(source.into(), json!({"params": {"kinds": ["risk"]}}), reader, Duration::from_secs(timeout)).await
    }

    #[tokio::test]
    async fn a_script_reads_files_and_concludes() {
        let src = r#"
export default function (input, flow) {
  const rows = flow.read.jsonl(`${input.params.kinds[0]}s.jsonl`) ?? [];
  const open = rows.filter((r) => r.status === "open");
  flow.log("rows", rows.length);
  return {
    verdict: open.length ? "fail" : "pass",
    summary: `${open.length} open`,
    findings: open.map((r) => ({ rule: "closed", path: "risks.jsonl", message: `${r.id} is open` })),
    metrics: { open: open.length },
    evidence: [{ path: "made-up", sha256: "x", size: 1 }],
  };
}"#;
        let (r, t) = judge(src, &[("risks.jsonl", "{\"id\":\"R1\",\"status\":\"open\"}\n{\"id\":\"R2\",\"status\":\"closed\"}\n")], 5).await;
        assert_eq!(r.verdict, GateVerdict::Fail, "{r:?}");
        assert_eq!(r.findings[0].message, "R1 is open");
        assert_eq!(r.metrics["open"], 1);
        // What it read, not what it said it read.
        assert_eq!(r.evidence.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["risks.jsonl"]);
        assert_eq!(t.log, "rows 2\n");
    }

    #[tokio::test]
    async fn an_async_script_and_the_old_shape_are_both_taken() {
        let (r, _) = judge("export default async function () { return { pass: true, detail: 'ok' }; }", &[], 5).await;
        assert_eq!((r.verdict, r.summary.as_str()), (GateVerdict::Pass, "ok"));
    }

    #[tokio::test]
    async fn a_script_that_throws_runs_too_long_or_returns_junk_is_an_error() {
        let (r, _) = judge("export default function () { throw new Error('boom'); }", &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Error);
        assert!(r.summary.contains("boom"), "{r:?}");
        let (r, _) = judge("export default function () { for (;;) {} }", &[], 1).await;
        assert_eq!(r.verdict, GateVerdict::Error);
        assert!(r.summary.contains("longer than 1s"), "{r:?}");
        let (r, _) = judge("export default function () { return { verdict: 'maybe' }; }", &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Error);
        let (r, _) = judge("export default function () {}", &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Error);
    }

    #[tokio::test]
    async fn reading_out_of_bounds_is_an_error_even_when_caught() {
        let src = "export default function (i, flow) { try { flow.read.text('../secret'); } catch (e) {} return { verdict: 'pass' }; }";
        let (r, _) = judge(src, &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Error, "{r:?}");
        assert!(r.summary.contains("out of bounds"), "{r:?}");
    }

    #[tokio::test]
    async fn a_script_has_no_way_out() {
        // No imports (no loader), no host functions beyond `flow`.
        let (r, _) = judge("import fs from 'fs'; export default function () { return { verdict: 'pass' }; }", &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Error);
        let src = "export default function () { return { verdict: typeof require === 'undefined' && typeof fetch === 'undefined' && typeof __flow === 'undefined' ? 'pass' : 'fail' }; }";
        let (r, _) = judge(src, &[], 5).await;
        assert_eq!(r.verdict, GateVerdict::Pass, "{r:?}");
    }

    #[test]
    fn a_script_is_checked_as_a_module_before_it_is_saved() {
        assert!(problems("export default function () { return {verdict: 'pass'}; }").is_empty());
        assert!(!problems("export default function ( {").is_empty());
        assert!(!problems("function check() {}").is_empty());
    }
}

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;

use serde_json::{json, Value as Json};

use crate::interp::{FnDef, Value};
use crate::parse::Lang;

// Embedded so the binary is self-contained; worker/ holds the source of truth.
const PY_WORKER: &str = include_str!("../worker/worker.py");
const JS_WORKER: &str = include_str!("../worker/worker.js");

/// Shared list of worker-object ids whose last seam handle has dropped.
/// Drained (batched into one `release` op) at statement boundaries.
pub type Graveyard = Rc<std::cell::RefCell<Vec<u64>>>;

/// Rides inside every Value::Ref; clones share it. When the final clone
/// drops, the id goes to the graveyard so the worker can free the object.
pub struct RefGuard {
    id: u64,
    graveyard: Graveyard,
}

impl Drop for RefGuard {
    fn drop(&mut self) {
        self.graveyard.borrow_mut().push(self.id);
    }
}

/// Seam functions handed to workers, keyed by the id sent over the wire.
pub struct CbRegistry {
    fns: HashMap<u64, Rc<FnDef>>,
    next: u64,
}

impl CbRegistry {
    pub fn new() -> CbRegistry {
        CbRegistry { fns: HashMap::new(), next: 0 }
    }

    fn register(&mut self, f: Rc<FnDef>) -> u64 {
        self.next += 1;
        self.fns.insert(self.next, f);
        self.next
    }

    pub fn get(&self, id: u64) -> Option<Rc<FnDef>> {
        self.fns.get(&id).cloned()
    }
}

/// What a worker says next: the answer to our request, a callback to run,
/// or "the iterator is exhausted" (only ever in reply to a `next` op).
pub enum WorkerMsg {
    Response(Result<Value, String>),
    Callback { fn_id: u64, args: Vec<Value> },
    IterDone,
}

pub struct Worker {
    lang: Lang,
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    graveyard: Graveyard,
    pub cmd: String,
}

impl Worker {
    pub fn spawn(lang: Lang) -> Result<Worker, String> {
        let (cmd, script) = match lang {
            Lang::Py => {
                let python = if std::path::Path::new(".venv/bin/python3").exists() {
                    ".venv/bin/python3".to_string()
                } else {
                    "python3".to_string()
                };
                (python, PY_WORKER)
            }
            Lang::Js => ("node".to_string(), JS_WORKER),
        };
        let args: &[&str] = match lang {
            Lang::Py => &["-u", "-c", script],
            Lang::Js => &["-e", script],
        };
        let mut child = Command::new(&cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to start {cmd}: {e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Worker {
            lang,
            _child: child,
            stdin,
            stdout,
            next_id: 0,
            graveyard: Rc::new(std::cell::RefCell::new(Vec::new())),
            cmd,
        })
    }

    pub fn send_import(&mut self, name: &str) -> Result<(), String> {
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "import", "name": name}))
    }

    pub fn send_getattr(&mut self, reg: &mut CbRegistry, obj: &Value, name: &str) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "getattr", "obj": obj, "name": name}))
    }

    pub fn send_index(&mut self, reg: &mut CbRegistry, obj: &Value, key: &Value) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let key = self.to_wire(reg, key)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "index", "obj": obj, "key": key}))
    }

    pub fn send_call(
        &mut self,
        reg: &mut CbRegistry,
        obj: &Value,
        args: &[Value],
        kwargs: &[(String, Value)],
    ) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let args: Vec<Json> = args.iter().map(|v| self.to_wire(reg, v)).collect::<Result<_, _>>()?;
        let mut kw = serde_json::Map::new();
        for (k, v) in kwargs {
            kw.insert(k.clone(), self.to_wire(reg, v)?);
        }
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "call", "obj": obj, "args": args, "kwargs": kw}))
    }

    pub fn send_binop(
        &mut self,
        reg: &mut CbRegistry,
        op: &str,
        a: &Value,
        b: Option<&Value>,
    ) -> Result<(), String> {
        let a = self.to_wire(reg, a)?;
        let mut msg = json!({"op": "binop", "operator": op, "a": a});
        if let Some(b) = b {
            msg["b"] = self.to_wire(reg, b)?;
        }
        let id = self.next_req();
        msg["id"] = json!(id);
        self.send_json(&msg)
    }

    pub fn send_str(&mut self, reg: &mut CbRegistry, obj: &Value) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "str", "obj": obj}))
    }

    pub fn send_setattr(
        &mut self,
        reg: &mut CbRegistry,
        obj: &Value,
        name: &str,
        value: &Value,
    ) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let value = self.to_wire(reg, value)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "setattr", "obj": obj, "name": name, "value": value}))
    }

    pub fn send_setitem(
        &mut self,
        reg: &mut CbRegistry,
        obj: &Value,
        key: &Value,
        value: &Value,
    ) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let key = self.to_wire(reg, key)?;
        let value = self.to_wire(reg, value)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "setitem", "obj": obj, "key": key, "value": value}))
    }

    pub fn send_new(
        &mut self,
        reg: &mut CbRegistry,
        obj: &Value,
        args: &[Value],
        kwargs: &[(String, Value)],
    ) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let args: Vec<Json> = args.iter().map(|v| self.to_wire(reg, v)).collect::<Result<_, _>>()?;
        let mut kw = serde_json::Map::new();
        for (k, v) in kwargs {
            kw.insert(k.clone(), self.to_wire(reg, v)?);
        }
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "new", "obj": obj, "args": args, "kwargs": kw}))
    }

    pub fn pending_release(&self) -> bool {
        !self.graveyard.borrow().is_empty()
    }

    /// Send one batched release for every id whose last handle has dropped.
    pub fn send_release_batch(&mut self) -> Result<(), String> {
        let ids = std::mem::take(&mut *self.graveyard.borrow_mut());
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "release", "refs": ids}))
    }

    pub fn send_stats(&mut self) -> Result<(), String> {
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "stats"}))
    }

    pub fn send_iter(&mut self, reg: &mut CbRegistry, obj: &Value) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "iter", "obj": obj}))
    }

    pub fn send_next(&mut self, reg: &mut CbRegistry, it: &Value) -> Result<(), String> {
        let it = self.to_wire(reg, it)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "next", "obj": it}))
    }

    pub fn send_cb_ok(&mut self, reg: &mut CbRegistry, v: &Value) -> Result<(), String> {
        match self.to_wire(reg, v) {
            Ok(w) => self.send_json(&json!({"cbr": true, "ok": true, "value": w})),
            Err(e) => self.send_cb_err(&e),
        }
    }

    pub fn send_cb_err(&mut self, e: &str) -> Result<(), String> {
        self.send_json(&json!({"cbr": true, "ok": false, "error": e}))
    }

    pub fn read_msg(&mut self) -> Result<WorkerMsg, String> {
        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("{} worker read failed: {e}", self.lang))?;
        if n == 0 {
            return Err(format!("{} worker died", self.lang));
        }
        let r: Json = serde_json::from_str(&resp).map_err(|e| format!("bad worker response: {e}"))?;
        if r["cb"].as_bool() == Some(true) {
            let fn_id = r["fn"].as_u64().unwrap_or(0);
            let args = r["args"]
                .as_array()
                .map(|a| a.iter().map(|w| self.wire_to_value(w)).collect())
                .unwrap_or_default();
            return Ok(WorkerMsg::Callback { fn_id, args });
        }
        if r["ok"].as_bool() == Some(true) {
            if r["stop"].as_bool() == Some(true) {
                return Ok(WorkerMsg::IterDone);
            }
            Ok(WorkerMsg::Response(Ok(self.wire_to_value(&r["value"]))))
        } else {
            let err = r["error"].as_str().unwrap_or("unknown worker error");
            let full = match r["trace"].as_str() {
                Some(t) if !t.is_empty() => format!("{err}\n{}", t.trim_end()),
                _ => err.to_string(),
            };
            Ok(WorkerMsg::Response(Err(full)))
        }
    }

    fn next_req(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn send_json(&mut self, msg: &Json) -> Result<(), String> {
        let line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}")
            .map_err(|_| format!("{} worker died (write failed)", self.lang))?;
        self.stdin.flush().map_err(|e| e.to_string())
    }

    fn to_wire(&self, reg: &mut CbRegistry, v: &Value) -> Result<Json, String> {
        match v {
            Value::Data(j) => Ok(j.clone()),
            Value::Ref { lang, id, .. } if *lang == self.lang => Ok(json!({"$": "ref", "id": id})),
            Value::Ref { lang, .. } => Err(format!(
                "can't pass a {lang} ref to the {} worker — pull it into data first (workers don't share heaps)",
                self.lang
            )),
            Value::Fn(f) => {
                let id = reg.register(f.clone());
                Ok(json!({"$": "fn", "id": id}))
            }
        }
    }

    fn wire_to_value(&self, w: &Json) -> Value {
        match w["$"].as_str() {
            Some("ref") => {
                let id = w["id"].as_u64().unwrap_or(0);
                Value::Ref {
                    lang: self.lang,
                    id,
                    repr: w["repr"].as_str().unwrap_or("").to_string(),
                    _guard: Rc::new(RefGuard { id, graveyard: self.graveyard.clone() }),
                }
            }
            _ => Value::Data(w["v"].clone()),
        }
    }
}

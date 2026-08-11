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

/// What a worker says next: the answer to our request, or a callback to run.
pub enum WorkerMsg {
    Response(Result<Value, String>),
    Callback { fn_id: u64, args: Vec<Value> },
}

pub struct Worker {
    lang: Lang,
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
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
        Ok(Worker { lang, _child: child, stdin, stdout, next_id: 0, cmd })
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

    pub fn send_str(&mut self, reg: &mut CbRegistry, obj: &Value) -> Result<(), String> {
        let obj = self.to_wire(reg, obj)?;
        let id = self.next_req();
        self.send_json(&json!({"id": id, "op": "str", "obj": obj}))
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
            Some("ref") => Value::Ref {
                lang: self.lang,
                id: w["id"].as_u64().unwrap_or(0),
                repr: w["repr"].as_str().unwrap_or("").to_string(),
            },
            _ => Value::Data(w["v"].clone()),
        }
    }
}

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value as Json};

use crate::interp::Value;
use crate::parse::Lang;

// Embedded so the binary is self-contained; worker/ holds the source of truth.
const PY_WORKER: &str = include_str!("../worker/worker.py");
const JS_WORKER: &str = include_str!("../worker/worker.js");

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

    pub fn import(&mut self, name: &str) -> Result<Value, String> {
        self.request(json!({"op": "import", "name": name}))
    }

    pub fn getattr(&mut self, obj: &Value, name: &str) -> Result<Value, String> {
        let obj = self.to_wire(obj)?;
        self.request(json!({"op": "getattr", "obj": obj, "name": name}))
    }

    pub fn index(&mut self, obj: &Value, key: &Value) -> Result<Value, String> {
        let obj = self.to_wire(obj)?;
        let key = self.to_wire(key)?;
        self.request(json!({"op": "index", "obj": obj, "key": key}))
    }

    pub fn call(&mut self, obj: &Value, args: &[Value], kwargs: &[(String, Value)]) -> Result<Value, String> {
        let obj = self.to_wire(obj)?;
        let args: Vec<Json> = args.iter().map(|v| self.to_wire(v)).collect::<Result<_, _>>()?;
        let mut kw = serde_json::Map::new();
        for (k, v) in kwargs {
            kw.insert(k.clone(), self.to_wire(v)?);
        }
        self.request(json!({"op": "call", "obj": obj, "args": args, "kwargs": kw}))
    }

    pub fn str_of(&mut self, obj: &Value) -> Result<Value, String> {
        let obj = self.to_wire(obj)?;
        self.request(json!({"op": "str", "obj": obj}))
    }

    fn request(&mut self, mut req: Json) -> Result<Value, String> {
        self.next_id += 1;
        req["id"] = json!(self.next_id);
        let line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}")
            .map_err(|_| format!("{} worker died (write failed)", self.lang))?;
        self.stdin.flush().map_err(|e| e.to_string())?;

        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("{} worker read failed: {e}", self.lang))?;
        if n == 0 {
            return Err(format!("{} worker died", self.lang));
        }
        let r: Json = serde_json::from_str(&resp).map_err(|e| format!("bad worker response: {e}"))?;
        if r["ok"].as_bool() == Some(true) {
            Ok(self.wire_to_value(&r["value"]))
        } else {
            let err = r["error"].as_str().unwrap_or("unknown worker error");
            match r["trace"].as_str() {
                Some(t) if !t.is_empty() => Err(format!("{err}\n{}", t.trim_end())),
                _ => Err(err.to_string()),
            }
        }
    }

    fn to_wire(&self, v: &Value) -> Result<Json, String> {
        match v {
            Value::Data(j) => Ok(j.clone()),
            Value::Ref { lang, id, .. } if *lang == self.lang => Ok(json!({"$": "ref", "id": id})),
            Value::Ref { lang, .. } => Err(format!(
                "can't pass a {lang} ref to the {} worker — pull it into data first (workers don't share heaps)",
                self.lang
            )),
            Value::Fn(_) => {
                Err("can't pass a seam function to a worker (callbacks are a future weekend)".to_string())
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

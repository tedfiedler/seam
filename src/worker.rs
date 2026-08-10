use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value as Json};

use crate::lang::Value;

// Embedded so the binary is self-contained; worker/worker.py is the source of truth.
const PY_WORKER: &str = include_str!("../worker/worker.py");

pub struct Worker {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    pub python: String,
}

impl Worker {
    pub fn spawn_python() -> Result<Worker, String> {
        let python = if std::path::Path::new(".venv/bin/python3").exists() {
            ".venv/bin/python3".to_string()
        } else {
            "python3".to_string()
        };
        let mut child = Command::new(&python)
            .args(["-u", "-c", PY_WORKER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to start {python}: {e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Worker { _child: child, stdin, stdout, next_id: 0, python })
    }

    pub fn import(&mut self, name: &str) -> Result<Value, String> {
        self.request(json!({"op": "import", "name": name}))
    }

    pub fn getattr(&mut self, obj: &Value, name: &str) -> Result<Value, String> {
        self.request(json!({"op": "getattr", "obj": value_to_wire(obj), "name": name}))
    }

    pub fn index(&mut self, obj: &Value, key: &Value) -> Result<Value, String> {
        self.request(json!({"op": "index", "obj": value_to_wire(obj), "key": value_to_wire(key)}))
    }

    pub fn call(&mut self, obj: &Value, args: &[Value], kwargs: &[(String, Value)]) -> Result<Value, String> {
        let args: Vec<Json> = args.iter().map(value_to_wire).collect();
        let kwargs: serde_json::Map<String, Json> = kwargs
            .iter()
            .map(|(k, v)| (k.clone(), value_to_wire(v)))
            .collect();
        self.request(json!({"op": "call", "obj": value_to_wire(obj), "args": args, "kwargs": kwargs}))
    }

    pub fn str_of(&mut self, obj: &Value) -> Result<Value, String> {
        self.request(json!({"op": "str", "obj": value_to_wire(obj)}))
    }

    fn request(&mut self, mut req: Json) -> Result<Value, String> {
        self.next_id += 1;
        req["id"] = json!(self.next_id);
        let line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|_| "python worker died (write failed)".to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;

        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("python worker read failed: {e}"))?;
        if n == 0 {
            return Err("python worker died".to_string());
        }
        let r: Json = serde_json::from_str(&resp).map_err(|e| format!("bad worker response: {e}"))?;
        if r["ok"].as_bool() == Some(true) {
            Ok(wire_to_value(&r["value"]))
        } else {
            let err = r["error"].as_str().unwrap_or("unknown python error");
            match r["trace"].as_str() {
                Some(t) => Err(format!("{err}\n{}", t.trim_end())),
                None => Err(err.to_string()),
            }
        }
    }
}

fn value_to_wire(v: &Value) -> Json {
    match v {
        Value::Data(j) => j.clone(),
        Value::PyRef { id, .. } => json!({"$": "ref", "id": id}),
    }
}

fn wire_to_value(w: &Json) -> Value {
    match w["$"].as_str() {
        Some("ref") => Value::PyRef {
            id: w["id"].as_u64().unwrap_or(0),
            repr: w["repr"].as_str().unwrap_or("").to_string(),
        },
        _ => Value::Data(w["v"].clone()),
    }
}

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Value as Json};

use crate::parse::{BinOp, Expr, Lang, Parser, Stmt};
use crate::worker::Worker;

#[derive(Clone)]
pub enum Value {
    Data(Json),
    Ref { lang: Lang, id: u64, repr: String },
    Fn(Rc<FnDef>),
}

pub struct FnDef {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
    closure: Rc<RefCell<Env>>,
}

pub struct Env {
    vars: HashMap<String, Value>,
    parent: Option<Rc<RefCell<Env>>>,
}

fn child(parent: &Rc<RefCell<Env>>) -> Rc<RefCell<Env>> {
    Rc::new(RefCell::new(Env { vars: HashMap::new(), parent: Some(parent.clone()) }))
}

impl Env {
    fn get(env: &Rc<RefCell<Env>>, name: &str) -> Option<Value> {
        let e = env.borrow();
        if let Some(v) = e.vars.get(name) {
            return Some(v.clone());
        }
        match &e.parent {
            Some(p) => Env::get(p, name),
            None => None,
        }
    }

    /// Assign to an existing variable somewhere up the scope chain.
    fn set(env: &Rc<RefCell<Env>>, name: &str, v: Value) -> bool {
        {
            let mut e = env.borrow_mut();
            if e.vars.contains_key(name) {
                e.vars.insert(name.to_string(), v);
                return true;
            }
        }
        let parent = env.borrow().parent.clone();
        match parent {
            Some(p) => Env::set(&p, name, v),
            None => false,
        }
    }

    fn define(env: &Rc<RefCell<Env>>, name: &str, v: Value) {
        env.borrow_mut().vars.insert(name.to_string(), v);
    }
}

/// Non-local exits: errors, plus return/break/continue unwinding.
pub enum Escape {
    Error(String),
    Return(Value),
    Break,
    Continue,
}

impl From<String> for Escape {
    fn from(s: String) -> Escape {
        Escape::Error(s)
    }
}

type EvalResult = Result<Value, Escape>;

pub struct Interp {
    globals: Rc<RefCell<Env>>,
    py: Option<Worker>,
    js: Option<Worker>,
    echo: bool,
}

impl Interp {
    pub fn new() -> Interp {
        Interp {
            globals: Rc::new(RefCell::new(Env { vars: HashMap::new(), parent: None })),
            py: None,
            js: None,
            echo: false,
        }
    }

    fn worker(&mut self, lang: Lang) -> Result<&mut Worker, String> {
        let slot = match lang {
            Lang::Py => &mut self.py,
            Lang::Js => &mut self.js,
        };
        if slot.is_none() {
            let w = Worker::spawn(lang)?;
            eprintln!("· spawned {lang} worker ({})", w.cmd);
            *slot = Some(w);
        }
        Ok(slot.as_mut().unwrap())
    }

    /// Run a chunk of source. With `echo`, bare expression results are printed.
    pub fn run(&mut self, src: &str, echo: bool) -> Result<(), String> {
        self.echo = echo;
        let toks = crate::lex::lex(src)?;
        let stmts = Parser::new(toks).parse_program()?;
        let globals = self.globals.clone();
        for s in &stmts {
            match self.exec(s, &globals) {
                Ok(()) => {}
                Err(Escape::Error(e)) => return Err(e),
                Err(Escape::Return(_)) => return Err("return outside a function".to_string()),
                Err(Escape::Break) | Err(Escape::Continue) => {
                    return Err("break/continue outside a loop".to_string())
                }
            }
        }
        Ok(())
    }

    fn exec(&mut self, s: &Stmt, env: &Rc<RefCell<Env>>) -> Result<(), Escape> {
        match s {
            Stmt::Use { lang, module, alias } => {
                let v = self.worker(*lang)?.import(module)?;
                Env::define(env, alias, v);
            }
            Stmt::Let(name, e) => {
                let v = self.eval(e, env)?;
                Env::define(env, name, v);
            }
            Stmt::Assign(name, e) => {
                let v = self.eval(e, env)?;
                if !Env::set(env, name, v) {
                    return Err(Escape::Error(format!(
                        "undefined variable '{name}' — use let to define it"
                    )));
                }
            }
            Stmt::Expr(e) => {
                let v = self.eval(e, env)?;
                if self.echo && !matches!(v, Value::Data(Json::Null)) {
                    println!("{}", display(&v));
                }
            }
            Stmt::If { cond, then, els } => {
                let c = self.eval(cond, env)?;
                let branch = if truthy(&c) { then } else { els };
                self.exec_block(branch, env)?;
            }
            Stmt::While { cond, body } => loop {
                let c = self.eval(cond, env)?;
                if !truthy(&c) {
                    break;
                }
                match self.exec_block(body, env) {
                    Ok(()) => {}
                    Err(Escape::Break) => break,
                    Err(Escape::Continue) => continue,
                    Err(other) => return Err(other),
                }
            },
            Stmt::For { var, iter, body } => {
                let items: Vec<Value> = match self.eval(iter, env)? {
                    Value::Data(Json::Array(a)) => a.into_iter().map(Value::Data).collect(),
                    Value::Data(Json::Object(m)) => {
                        m.keys().map(|k| Value::Data(json!(k))).collect()
                    }
                    Value::Data(Json::String(s)) => {
                        s.chars().map(|c| Value::Data(json!(c.to_string()))).collect()
                    }
                    Value::Ref { .. } => {
                        return Err(Escape::Error(
                            "can't iterate a worker ref yet — pull data across first (e.g. .tolist() or .to_dict())"
                                .to_string(),
                        ))
                    }
                    _ => {
                        return Err(Escape::Error(
                            "for wants an array, object (keys), or string".to_string(),
                        ))
                    }
                };
                let loop_env = child(env);
                for item in items {
                    Env::define(&loop_env, var, item);
                    match self.exec_block(body, &loop_env) {
                        Ok(()) => {}
                        Err(Escape::Break) => break,
                        Err(Escape::Continue) => continue,
                        Err(other) => return Err(other),
                    }
                }
            }
            Stmt::Fn { name, params, body } => {
                let f = FnDef {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    closure: env.clone(),
                };
                Env::define(env, name, Value::Fn(Rc::new(f)));
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.eval(e, env)?,
                    None => Value::Data(Json::Null),
                };
                return Err(Escape::Return(v));
            }
            Stmt::Break => return Err(Escape::Break),
            Stmt::Continue => return Err(Escape::Continue),
        }
        Ok(())
    }

    fn exec_block(&mut self, stmts: &[Stmt], env: &Rc<RefCell<Env>>) -> Result<(), Escape> {
        let scope = child(env);
        for s in stmts {
            self.exec(s, &scope)?;
        }
        Ok(())
    }

    fn eval(&mut self, e: &Expr, env: &Rc<RefCell<Env>>) -> EvalResult {
        match e {
            Expr::Num(n) => Ok(Value::Data(num_to_json(*n))),
            Expr::Str(s) => Ok(Value::Data(json!(s))),
            Expr::Bool(b) => Ok(Value::Data(json!(b))),
            Expr::Nil => Ok(Value::Data(Json::Null)),
            Expr::Var(name) => Env::get(env, name)
                .ok_or_else(|| Escape::Error(format!("undefined variable '{name}'"))),
            Expr::Array(items) => {
                let mut arr = Vec::new();
                for item in items {
                    arr.push(self.eval_to_data(item, env)?);
                }
                Ok(Value::Data(Json::Array(arr)))
            }
            Expr::Object(pairs) => {
                let mut map = serde_json::Map::new();
                for (k, v) in pairs {
                    map.insert(k.clone(), self.eval_to_data(v, env)?);
                }
                Ok(Value::Data(Json::Object(map)))
            }
            Expr::Attr(obj, name) => {
                let ov = self.eval(obj, env)?;
                match ov {
                    Value::Ref { lang, .. } => Ok(self.worker(lang)?.getattr(&ov, name)?),
                    _ => Err(Escape::Error(format!(
                        "only worker refs have attributes (tried .{name})"
                    ))),
                }
            }
            Expr::Index(obj, key) => {
                let ov = self.eval(obj, env)?;
                let kv = self.eval(key, env)?;
                match (&ov, &kv) {
                    (Value::Ref { lang, .. }, _) => {
                        let lang = *lang;
                        Ok(self.worker(lang)?.index(&ov, &kv)?)
                    }
                    (Value::Data(Json::Array(a)), Value::Data(k)) => {
                        let i = k
                            .as_f64()
                            .ok_or_else(|| Escape::Error("array index must be a number".into()))?
                            as usize;
                        a.get(i).cloned().map(Value::Data).ok_or_else(|| {
                            Escape::Error(format!("index {i} out of bounds (len {})", a.len()))
                        })
                    }
                    (Value::Data(Json::Object(m)), Value::Data(Json::String(s))) => m
                        .get(s)
                        .cloned()
                        .map(Value::Data)
                        .ok_or_else(|| Escape::Error(format!("no key '{s}'"))),
                    _ => Err(Escape::Error("cannot index that value".to_string())),
                }
            }
            Expr::Call { callee, args, kwargs } => {
                if let Expr::Var(name) = callee.as_ref() {
                    if Env::get(env, name).is_none() {
                        if let Some(v) = self.try_builtin(name, args, kwargs, env)? {
                            return Ok(v);
                        }
                    }
                }
                let cv = self.eval(callee, env)?;
                let mut argv = Vec::new();
                for a in args {
                    argv.push(self.eval(a, env)?);
                }
                match cv {
                    Value::Ref { lang, .. } => {
                        let mut kwv = Vec::new();
                        for (k, v) in kwargs {
                            kwv.push((k.clone(), self.eval(v, env)?));
                        }
                        Ok(self.worker(lang)?.call(&cv, &argv, &kwv)?)
                    }
                    Value::Fn(f) => self.call_fn(&f, argv, kwargs),
                    Value::Data(_) => Err(Escape::Error("value is not callable".to_string())),
                }
            }
            Expr::Binop(op, a, b) => {
                let av = self.eval(a, env)?;
                let bv = self.eval(b, env)?;
                binop(*op, &av, &bv)
            }
            Expr::And(a, b) => {
                let av = self.eval(a, env)?;
                if truthy(&av) { self.eval(b, env) } else { Ok(av) }
            }
            Expr::Or(a, b) => {
                let av = self.eval(a, env)?;
                if truthy(&av) { Ok(av) } else { self.eval(b, env) }
            }
            Expr::Not(a) => {
                let av = self.eval(a, env)?;
                Ok(Value::Data(json!(!truthy(&av))))
            }
            Expr::Neg(a) => match self.eval(a, env)? {
                Value::Data(j) => {
                    let n = j
                        .as_f64()
                        .ok_or_else(|| Escape::Error(format!("cannot negate {j}")))?;
                    Ok(Value::Data(num_to_json(-n)))
                }
                _ => Err(Escape::Error("cannot negate that value".to_string())),
            },
        }
    }

    fn eval_to_data(&mut self, e: &Expr, env: &Rc<RefCell<Env>>) -> Result<Json, Escape> {
        match self.eval(e, env)? {
            Value::Data(j) => Ok(j),
            other => Err(Escape::Error(format!(
                "array/object literals hold data values only, not {}",
                display(&other)
            ))),
        }
    }

    fn call_fn(&mut self, f: &Rc<FnDef>, argv: Vec<Value>, kwargs: &[(String, Expr)]) -> EvalResult {
        if !kwargs.is_empty() {
            return Err(Escape::Error(format!(
                "seam functions don't take keyword arguments (calling {})",
                f.name
            )));
        }
        if argv.len() != f.params.len() {
            return Err(Escape::Error(format!(
                "{} expects {} argument(s), got {}",
                f.name,
                f.params.len(),
                argv.len()
            )));
        }
        let call_env = child(&f.closure);
        for (p, a) in f.params.iter().zip(argv) {
            Env::define(&call_env, p, a);
        }
        for s in &f.body {
            match self.exec(s, &call_env) {
                Ok(()) => {}
                Err(Escape::Return(v)) => return Ok(v),
                Err(other) => return Err(other),
            }
        }
        Ok(Value::Data(Json::Null))
    }

    fn try_builtin(
        &mut self,
        name: &str,
        args: &[Expr],
        kwargs: &[(String, Expr)],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>, Escape> {
        if !matches!(name, "print" | "len" | "range" | "str") {
            return Ok(None);
        }
        if !kwargs.is_empty() {
            return Err(Escape::Error(format!("{name} doesn't take keyword arguments")));
        }
        let mut argv = Vec::new();
        for a in args {
            argv.push(self.eval(a, env)?);
        }
        let v = match (name, argv.len()) {
            ("print", _) => {
                let mut parts = Vec::new();
                for a in &argv {
                    parts.push(self.stringify(a)?);
                }
                println!("{}", parts.join(" "));
                Value::Data(Json::Null)
            }
            ("str", 1) => Value::Data(json!(self.stringify(&argv[0])?)),
            ("len", 1) => match &argv[0] {
                Value::Data(Json::String(s)) => Value::Data(json!(s.chars().count())),
                Value::Data(Json::Array(a)) => Value::Data(json!(a.len())),
                Value::Data(Json::Object(m)) => Value::Data(json!(m.len())),
                r @ Value::Ref { lang: Lang::Py, .. } => {
                    let f = self.worker(Lang::Py)?.getattr(r, "__len__")?;
                    self.worker(Lang::Py)?.call(&f, &[], &[])?
                }
                r @ Value::Ref { lang: Lang::Js, .. } => {
                    match self.worker(Lang::Js)?.getattr(r, "length")? {
                        Value::Data(j) if j.is_number() => Value::Data(j),
                        _ => return Err(Escape::Error("len: js value has no numeric .length".into())),
                    }
                }
                _ => return Err(Escape::Error("len wants a string, array, object, or worker ref".into())),
            },
            ("range", 1 | 2) => {
                let int = |v: &Value| -> Result<i64, Escape> {
                    match v {
                        Value::Data(j) => j
                            .as_f64()
                            .map(|f| f as i64)
                            .ok_or_else(|| Escape::Error("range wants numbers".into())),
                        _ => Err(Escape::Error("range wants numbers".into())),
                    }
                };
                let (lo, hi) = if argv.len() == 1 {
                    (0, int(&argv[0])?)
                } else {
                    (int(&argv[0])?, int(&argv[1])?)
                };
                Value::Data(Json::Array((lo..hi).map(|i| json!(i)).collect()))
            }
            (name, n) => {
                return Err(Escape::Error(format!("{name} doesn't take {n} argument(s)")))
            }
        };
        Ok(Some(v))
    }

    fn stringify(&mut self, v: &Value) -> Result<String, Escape> {
        match v {
            Value::Data(Json::String(s)) => Ok(s.clone()),
            Value::Data(j) => Ok(serde_json::to_string(j).unwrap_or_default()),
            r @ Value::Ref { lang, .. } => {
                let lang = *lang;
                match self.worker(lang)?.str_of(r)? {
                    Value::Data(Json::String(s)) => Ok(s),
                    other => Ok(display(&other)),
                }
            }
            Value::Fn(_) => Ok(display(v)),
        }
    }
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Data(Json::Null) | Value::Data(Json::Bool(false)))
}

fn binop(op: BinOp, a: &Value, b: &Value) -> EvalResult {
    let (Value::Data(ja), Value::Data(jb)) = (a, b) else {
        return Err(Escape::Error(
            "operators need data values, not worker refs — pull the data across first (e.g. .item() or .tolist())"
                .to_string(),
        ));
    };
    match op {
        BinOp::Eq => return Ok(Value::Data(json!(ja == jb))),
        BinOp::Ne => return Ok(Value::Data(json!(ja != jb))),
        BinOp::Add if ja.is_string() || jb.is_string() => {
            return Ok(Value::Data(Json::String(format!(
                "{}{}",
                json_plain(ja),
                json_plain(jb)
            ))));
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if let (Json::String(x), Json::String(y)) = (ja, jb) {
                let r = match op {
                    BinOp::Lt => x < y,
                    BinOp::Le => x <= y,
                    BinOp::Gt => x > y,
                    _ => x >= y,
                };
                return Ok(Value::Data(json!(r)));
            }
        }
        _ => {}
    }
    let (Some(x), Some(y)) = (ja.as_f64(), jb.as_f64()) else {
        return Err(Escape::Error(format!("cannot apply {op:?} to {ja} and {jb}")));
    };
    let v = match op {
        BinOp::Add => num_to_json(x + y),
        BinOp::Sub => num_to_json(x - y),
        BinOp::Mul => num_to_json(x * y),
        BinOp::Div => num_to_json(x / y),
        BinOp::Mod => num_to_json(x % y),
        BinOp::Lt => json!(x < y),
        BinOp::Le => json!(x <= y),
        BinOp::Gt => json!(x > y),
        BinOp::Ge => json!(x >= y),
        BinOp::Eq | BinOp::Ne => unreachable!(),
    };
    Ok(Value::Data(v))
}

fn num_to_json(f: f64) -> Json {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

fn json_plain(j: &Json) -> String {
    match j {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn display(v: &Value) -> String {
    match v {
        Value::Data(j) => serde_json::to_string(j).unwrap_or_default(),
        Value::Ref { lang, id, repr } => format!("<{lang}:{id} {repr}>"),
        Value::Fn(f) => format!("<fn {}({})>", f.name, f.params.join(", ")),
    }
}

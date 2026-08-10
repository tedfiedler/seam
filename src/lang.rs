use std::collections::HashMap;

use serde_json::{json, Value as Json};

use crate::worker::Worker;

#[derive(Clone, Debug)]
pub enum Value {
    Data(Json),
    PyRef { id: u64, repr: String },
}

// ---------- lexer ----------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    Dot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Eq,
    Plus,
    Minus,
    Star,
    Slash,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '#' => break,
            '.' => { toks.push(Tok::Dot); i += 1 }
            '(' => { toks.push(Tok::LParen); i += 1 }
            ')' => { toks.push(Tok::RParen); i += 1 }
            '[' => { toks.push(Tok::LBracket); i += 1 }
            ']' => { toks.push(Tok::RBracket); i += 1 }
            '{' => { toks.push(Tok::LBrace); i += 1 }
            '}' => { toks.push(Tok::RBrace); i += 1 }
            ',' => { toks.push(Tok::Comma); i += 1 }
            ':' => { toks.push(Tok::Colon); i += 1 }
            '=' => { toks.push(Tok::Eq); i += 1 }
            '+' => { toks.push(Tok::Plus); i += 1 }
            '-' => { toks.push(Tok::Minus); i += 1 }
            '*' => { toks.push(Tok::Star); i += 1 }
            '/' => { toks.push(Tok::Slash); i += 1 }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string".to_string()),
                        Some('"') => { i += 1; break }
                        Some('\\') => {
                            i += 1;
                            let e = chars.get(i).ok_or("dangling escape")?;
                            s.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                '\\' => '\\',
                                '"' => '"',
                                other => return Err(format!("unknown escape \\{other}")),
                            });
                            i += 1;
                        }
                        Some(ch) => { s.push(*ch); i += 1 }
                    }
                }
                toks.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() { j += 1 }
                if j < chars.len() && chars[j] == '.' && chars.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                    while j < chars.len() && chars[j].is_ascii_digit() { j += 1 }
                }
                let s: String = chars[i..j].iter().collect();
                toks.push(Tok::Num(s.parse().map_err(|_| format!("bad number {s}"))?));
                i = j;
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut j = i;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') { j += 1 }
                toks.push(Tok::Ident(chars[i..j].iter().collect()));
                i = j;
            }
            other => return Err(format!("unexpected character {other:?}")),
        }
    }
    Ok(toks)
}

// ---------- parser ----------

#[derive(Debug)]
enum Stmt {
    UsePy { module: String, alias: String },
    Let(String, Expr),
    Expr(Expr),
}

#[derive(Debug)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Nil,
    Var(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    Attr(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call { callee: Box<Expr>, args: Vec<Expr>, kwargs: Vec<(String, Expr)> },
    Binop(char, Box<Expr>, Box<Expr>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: Tok) -> Result<(), String> {
        if self.eat(&t) {
            Ok(())
        } else {
            Err(format!("expected {t:?}, found {:?}", self.peek()))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            other => Err(format!("expected identifier, found {other:?}")),
        }
    }

    fn parse_stmt(mut self) -> Result<Stmt, String> {
        let stmt = match self.peek() {
            Some(Tok::Ident(s)) if s == "use" => {
                self.pos += 1;
                let lang = self.ident()?;
                if lang != "py" {
                    return Err(format!("only 'py' workers exist yet (got '{lang}')"));
                }
                let module = match self.next() {
                    Some(Tok::Str(s)) => s,
                    other => return Err(format!("expected module name string, found {other:?}")),
                };
                if self.ident()? != "as" {
                    return Err("expected 'as' after module name".to_string());
                }
                Stmt::UsePy { module, alias: self.ident()? }
            }
            Some(Tok::Ident(s)) if s == "let" => {
                self.pos += 1;
                let name = self.ident()?;
                self.expect(Tok::Eq)?;
                Stmt::Let(name, self.parse_expr()?)
            }
            _ => Stmt::Expr(self.parse_expr()?),
        };
        if self.pos < self.toks.len() {
            return Err(format!("unexpected trailing input: {:?}", self.peek()));
        }
        Ok(stmt)
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_factor()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => '+',
                Some(Tok::Minus) => '-',
                _ => break,
            };
            self.pos += 1;
            left = Expr::Binop(op, Box::new(left), Box::new(self.parse_factor()?));
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => '*',
                Some(Tok::Slash) => '/',
                _ => break,
            };
            self.pos += 1;
            left = Expr::Binop(op, Box::new(left), Box::new(self.parse_unary()?));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.eat(&Tok::Minus) {
            Ok(Expr::Binop('-', Box::new(Expr::Num(0.0)), Box::new(self.parse_unary()?)))
        } else {
            self.parse_postfix()
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.parse_primary()?;
        loop {
            if self.eat(&Tok::Dot) {
                e = Expr::Attr(Box::new(e), self.ident()?);
            } else if self.eat(&Tok::LParen) {
                let (args, kwargs) = self.parse_args()?;
                e = Expr::Call { callee: Box::new(e), args, kwargs };
            } else if self.eat(&Tok::LBracket) {
                let idx = self.parse_expr()?;
                self.expect(Tok::RBracket)?;
                e = Expr::Index(Box::new(e), Box::new(idx));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_args(&mut self) -> Result<(Vec<Expr>, Vec<(String, Expr)>), String> {
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok((args, kwargs));
        }
        loop {
            if let (Some(Tok::Ident(name)), Some(Tok::Eq)) =
                (self.toks.get(self.pos), self.toks.get(self.pos + 1))
            {
                let name = name.clone();
                self.pos += 2;
                kwargs.push((name, self.parse_expr()?));
            } else {
                args.push(self.parse_expr()?);
            }
            if self.eat(&Tok::Comma) {
                continue;
            }
            self.expect(Tok::RParen)?;
            break;
        }
        Ok((args, kwargs))
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Ident(s)) => match s.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "nil" => Ok(Expr::Nil),
                _ => Ok(Expr::Var(s)),
            },
            Some(Tok::LParen) => {
                let e = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::LBracket) => {
                let mut items = Vec::new();
                if !self.eat(&Tok::RBracket) {
                    loop {
                        items.push(self.parse_expr()?);
                        if self.eat(&Tok::Comma) {
                            continue;
                        }
                        self.expect(Tok::RBracket)?;
                        break;
                    }
                }
                Ok(Expr::Array(items))
            }
            Some(Tok::LBrace) => {
                let mut pairs = Vec::new();
                if !self.eat(&Tok::RBrace) {
                    loop {
                        let key = match self.next() {
                            Some(Tok::Ident(s)) | Some(Tok::Str(s)) => s,
                            other => return Err(format!("expected object key, found {other:?}")),
                        };
                        self.expect(Tok::Colon)?;
                        pairs.push((key, self.parse_expr()?));
                        if self.eat(&Tok::Comma) {
                            continue;
                        }
                        self.expect(Tok::RBrace)?;
                        break;
                    }
                }
                Ok(Expr::Object(pairs))
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

// ---------- interpreter ----------

pub struct Interp {
    env: HashMap<String, Value>,
    py: Option<Worker>,
}

impl Interp {
    pub fn new() -> Interp {
        Interp { env: HashMap::new(), py: None }
    }

    fn py(&mut self) -> Result<&mut Worker, String> {
        if self.py.is_none() {
            let w = Worker::spawn_python()?;
            eprintln!("· spawned python worker ({})", w.python);
            self.py = Some(w);
        }
        Ok(self.py.as_mut().unwrap())
    }

    /// Evaluate one line. Ok(Some(s)) is a result to display; Ok(None) is silent.
    pub fn eval_line(&mut self, line: &str) -> Result<Option<String>, String> {
        let toks = lex(line)?;
        if toks.is_empty() {
            return Ok(None);
        }
        match (Parser { toks, pos: 0 }).parse_stmt()? {
            Stmt::UsePy { module, alias } => {
                let v = self.py()?.import(&module)?;
                self.env.insert(alias, v);
                Ok(None)
            }
            Stmt::Let(name, e) => {
                let v = self.eval(&e)?;
                self.env.insert(name, v);
                Ok(None)
            }
            Stmt::Expr(e) => {
                let v = self.eval(&e)?;
                if matches!(v, Value::Data(Json::Null)) {
                    Ok(None)
                } else {
                    Ok(Some(display(&v)))
                }
            }
        }
    }

    fn eval(&mut self, e: &Expr) -> Result<Value, String> {
        match e {
            Expr::Num(n) => Ok(Value::Data(num_to_json(*n))),
            Expr::Str(s) => Ok(Value::Data(json!(s))),
            Expr::Bool(b) => Ok(Value::Data(json!(b))),
            Expr::Nil => Ok(Value::Data(Json::Null)),
            Expr::Var(name) => self
                .env
                .get(name)
                .cloned()
                .ok_or_else(|| format!("undefined variable '{name}'")),
            Expr::Array(items) => {
                let mut arr = Vec::new();
                for item in items {
                    arr.push(self.eval_to_data(item)?);
                }
                Ok(Value::Data(Json::Array(arr)))
            }
            Expr::Object(pairs) => {
                let mut map = serde_json::Map::new();
                for (k, v) in pairs {
                    map.insert(k.clone(), self.eval_to_data(v)?);
                }
                Ok(Value::Data(Json::Object(map)))
            }
            Expr::Attr(obj, name) => {
                let ov = self.eval(obj)?;
                match ov {
                    Value::PyRef { .. } => self.py()?.getattr(&ov, name),
                    Value::Data(_) => Err(format!("data values have no attributes (tried .{name})")),
                }
            }
            Expr::Index(obj, key) => {
                let ov = self.eval(obj)?;
                let kv = self.eval(key)?;
                match (&ov, &kv) {
                    (Value::PyRef { .. }, _) => self.py()?.index(&ov, &kv),
                    (Value::Data(Json::Array(a)), Value::Data(k)) => {
                        let i = k.as_f64().ok_or("array index must be a number")? as usize;
                        a.get(i)
                            .cloned()
                            .map(Value::Data)
                            .ok_or_else(|| format!("index {i} out of bounds (len {})", a.len()))
                    }
                    (Value::Data(Json::Object(m)), Value::Data(Json::String(s))) => m
                        .get(s)
                        .cloned()
                        .map(Value::Data)
                        .ok_or_else(|| format!("no key '{s}'")),
                    _ => Err("cannot index that value".to_string()),
                }
            }
            Expr::Call { callee, args, kwargs } => {
                if let Expr::Var(name) = callee.as_ref() {
                    if name == "print" && !self.env.contains_key("print") {
                        return self.builtin_print(args, kwargs);
                    }
                }
                let cv = self.eval(callee)?;
                match cv {
                    Value::PyRef { .. } => {
                        let mut argv = Vec::new();
                        for a in args {
                            argv.push(self.eval(a)?);
                        }
                        let mut kwv = Vec::new();
                        for (k, v) in kwargs {
                            kwv.push((k.clone(), self.eval(v)?));
                        }
                        self.py()?.call(&cv, &argv, &kwv)
                    }
                    Value::Data(_) => Err("value is not callable".to_string()),
                }
            }
            Expr::Binop(op, a, b) => {
                let av = self.eval(a)?;
                let bv = self.eval(b)?;
                binop(*op, &av, &bv)
            }
        }
    }

    fn eval_to_data(&mut self, e: &Expr) -> Result<Json, String> {
        match self.eval(e)? {
            Value::Data(j) => Ok(j),
            Value::PyRef { repr, .. } => Err(format!(
                "python refs can't go inside array/object literals yet ({repr})"
            )),
        }
    }

    fn builtin_print(&mut self, args: &[Expr], kwargs: &[(String, Expr)]) -> Result<Value, String> {
        if !kwargs.is_empty() || args.len() != 1 {
            return Err("print takes exactly one argument".to_string());
        }
        let v = self.eval(&args[0])?;
        let s = match &v {
            Value::Data(Json::String(s)) => s.clone(),
            Value::Data(j) => serde_json::to_string(j).unwrap_or_default(),
            r @ Value::PyRef { .. } => match self.py()?.str_of(r)? {
                Value::Data(Json::String(s)) => s,
                other => display(&other),
            },
        };
        println!("{s}");
        Ok(Value::Data(Json::Null))
    }
}

fn binop(op: char, a: &Value, b: &Value) -> Result<Value, String> {
    let (Value::Data(ja), Value::Data(jb)) = (a, b) else {
        return Err(
            "arithmetic needs data values, not python refs — pull the data out first (e.g. .to_string() or .item())"
                .to_string(),
        );
    };
    if op == '+' && (ja.is_string() || jb.is_string()) {
        return Ok(Value::Data(Json::String(format!(
            "{}{}",
            json_plain(ja),
            json_plain(jb)
        ))));
    }
    let (Some(x), Some(y)) = (ja.as_f64(), jb.as_f64()) else {
        return Err(format!("cannot apply '{op}' to {ja} and {jb}"));
    };
    let r = match op {
        '+' => x + y,
        '-' => x - y,
        '*' => x * y,
        '/' => x / y,
        _ => unreachable!(),
    };
    Ok(Value::Data(num_to_json(r)))
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
        Value::PyRef { id, repr } => format!("<py:{id} {repr}>"),
    }
}

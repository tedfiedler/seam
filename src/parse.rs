use crate::lex::Tok;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Py,
    Js,
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Lang::Py => "py",
            Lang::Js => "js",
        })
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Use { lang: Lang, module: String, alias: String },
    Let(String, Expr),
    Assign(String, Expr),
    Expr(Expr),
    If { cond: Expr, then: Vec<Stmt>, els: Vec<Stmt> },
    While { cond: Expr, body: Vec<Stmt> },
    For { var: String, iter: Expr, body: Vec<Stmt> },
    Fn { name: String, params: Vec<String>, body: Vec<Stmt> },
    Return(Option<Expr>),
    Break,
    Continue,
}

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
pub enum Expr {
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
    Binop(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
}

pub struct Parser {
    toks: Vec<(Tok, u32)>,
    pos: usize,
}

impl Parser {
    pub fn new(toks: Vec<(Tok, u32)>) -> Parser {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn peek2(&self) -> Option<&Tok> {
        self.toks.get(self.pos + 1).map(|(t, _)| t)
    }

    fn line(&self) -> u32 {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map(|(_, l)| *l)
            .unwrap_or(1)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|(t, _)| t.clone());
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

    fn eat_kw(&mut self, kw: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(s)) if s == kw) {
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
            Err(format!("line {}: expected {t:?}, found {:?}", self.line(), self.peek()))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            other => Err(format!("line {}: expected identifier, found {other:?}", self.line())),
        }
    }

    fn skip_nl(&mut self) {
        while self.eat(&Tok::Nl) {}
    }

    pub fn parse_program(mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_nl();
        while self.peek().is_some() {
            stmts.push(self.parse_stmt()?);
            if self.peek().is_some() && !self.eat(&Tok::Nl) {
                return Err(format!(
                    "line {}: expected newline after statement, found {:?}",
                    self.line(),
                    self.peek()
                ));
            }
            self.skip_nl();
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        if let Some(Tok::Ident(kw)) = self.peek() {
            match kw.as_str() {
                "use" => {
                    self.pos += 1;
                    let lang = match self.ident()?.as_str() {
                        "py" => Lang::Py,
                        "js" => Lang::Js,
                        other => {
                            return Err(format!("unknown worker language '{other}' (try py or js)"))
                        }
                    };
                    let module = match self.next() {
                        Some(Tok::Str(s)) => s,
                        other => {
                            return Err(format!(
                                "line {}: expected module name string, found {other:?}",
                                self.line()
                            ))
                        }
                    };
                    if !self.eat_kw("as") {
                        return Err(format!("line {}: expected 'as' after module name", self.line()));
                    }
                    return Ok(Stmt::Use { lang, module, alias: self.ident()? });
                }
                "let" => {
                    self.pos += 1;
                    let name = self.ident()?;
                    self.expect(Tok::Eq)?;
                    return Ok(Stmt::Let(name, self.parse_expr()?));
                }
                "fn" => {
                    self.pos += 1;
                    let name = self.ident()?;
                    self.expect(Tok::LParen)?;
                    let mut params = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            params.push(self.ident()?);
                            if self.eat(&Tok::Comma) {
                                continue;
                            }
                            self.expect(Tok::RParen)?;
                            break;
                        }
                    }
                    return Ok(Stmt::Fn { name, params, body: self.parse_block()? });
                }
                "if" => {
                    self.pos += 1;
                    return self.parse_if();
                }
                "while" => {
                    self.pos += 1;
                    self.expect(Tok::LParen)?;
                    let cond = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    return Ok(Stmt::While { cond, body: self.parse_block()? });
                }
                "for" => {
                    self.pos += 1;
                    self.expect(Tok::LParen)?;
                    let var = self.ident()?;
                    if !self.eat_kw("in") {
                        return Err(format!("line {}: expected 'in' in for loop", self.line()));
                    }
                    let iter = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    return Ok(Stmt::For { var, iter, body: self.parse_block()? });
                }
                "return" => {
                    self.pos += 1;
                    let value = match self.peek() {
                        None | Some(Tok::Nl) | Some(Tok::RBrace) => None,
                        _ => Some(self.parse_expr()?),
                    };
                    return Ok(Stmt::Return(value));
                }
                "break" => {
                    self.pos += 1;
                    return Ok(Stmt::Break);
                }
                "continue" => {
                    self.pos += 1;
                    return Ok(Stmt::Continue);
                }
                _ => {}
            }
        }
        // assignment lookahead: Ident = expr (but not ==)
        if let (Some(Tok::Ident(name)), Some(Tok::Eq)) = (self.peek(), self.peek2()) {
            let name = name.clone();
            self.pos += 2;
            return Ok(Stmt::Assign(name, self.parse_expr()?));
        }
        Ok(Stmt::Expr(self.parse_expr()?))
    }

    /// 'if' keyword already consumed. `else` must sit on the same line as `}`.
    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.expect(Tok::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(Tok::RParen)?;
        let then = self.parse_block()?;
        let mut els = Vec::new();
        if self.eat_kw("else") {
            if self.eat_kw("if") {
                els.push(self.parse_if()?);
            } else {
                els = self.parse_block()?;
            }
        }
        Ok(Stmt::If { cond, then, els })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        self.skip_nl();
        while self.peek() != Some(&Tok::RBrace) {
            if self.peek().is_none() {
                return Err("unexpected end of input in block (missing '}')".to_string());
            }
            stmts.push(self.parse_stmt()?);
            match self.peek() {
                Some(Tok::Nl) => self.skip_nl(),
                Some(Tok::RBrace) => break,
                other => {
                    return Err(format!(
                        "line {}: expected newline or '}}' after statement, found {other:?}",
                        self.line()
                    ))
                }
            }
        }
        self.expect(Tok::RBrace)?;
        Ok(stmts)
    }

    // precedence (loosest to tightest): or, and, not, comparison, + -, * / %, unary -, postfix
    fn parse_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.eat_kw("or") {
            left = Expr::Or(Box::new(left), Box::new(self.parse_and()?));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_not()?;
        while self.eat_kw("and") {
            left = Expr::And(Box::new(left), Box::new(self.parse_not()?));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.eat_kw("not") {
            Ok(Expr::Not(Box::new(self.parse_not()?)))
        } else {
            self.parse_cmp()
        }
    }

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        let left = self.parse_add()?;
        let op = match self.peek() {
            Some(Tok::EqEq) => BinOp::Eq,
            Some(Tok::BangEq) => BinOp::Ne,
            Some(Tok::Lt) => BinOp::Lt,
            Some(Tok::Le) => BinOp::Le,
            Some(Tok::Gt) => BinOp::Gt,
            Some(Tok::Ge) => BinOp::Ge,
            _ => return Ok(left),
        };
        self.pos += 1;
        let right = self.parse_add()?;
        Ok(Expr::Binop(op, Box::new(left), Box::new(right)))
    }

    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            left = Expr::Binop(op, Box::new(left), Box::new(self.parse_mul()?));
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                Some(Tok::Percent) => BinOp::Mod,
                _ => break,
            };
            self.pos += 1;
            left = Expr::Binop(op, Box::new(left), Box::new(self.parse_unary()?));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.eat(&Tok::Minus) {
            Ok(Expr::Neg(Box::new(self.parse_unary()?)))
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
            if let (Some(Tok::Ident(name)), Some(Tok::Eq)) = (self.peek(), self.peek2()) {
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
                self.skip_nl();
                if !self.eat(&Tok::RBrace) {
                    loop {
                        let key = match self.next() {
                            Some(Tok::Ident(s)) | Some(Tok::Str(s)) => s,
                            other => {
                                return Err(format!(
                                    "line {}: expected object key, found {other:?}",
                                    self.line()
                                ))
                            }
                        };
                        self.expect(Tok::Colon)?;
                        pairs.push((key, self.parse_expr()?));
                        let had_comma = self.eat(&Tok::Comma);
                        self.skip_nl();
                        if had_comma {
                            if self.eat(&Tok::RBrace) {
                                break;
                            }
                            continue;
                        }
                        self.expect(Tok::RBrace)?;
                        break;
                    }
                }
                Ok(Expr::Object(pairs))
            }
            other => Err(format!("line {}: unexpected {other:?}", self.line())),
        }
    }
}

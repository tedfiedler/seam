#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    Nl,
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
    Percent,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Tokenize source. Newlines become Tok::Nl (statement separators), except
/// inside ( ) and [ ] where they are dropped so args and arrays can span lines.
pub fn lex(src: &str) -> Result<Vec<(Tok, u32)>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks: Vec<(Tok, u32)> = Vec::new();
    let mut i = 0;
    let mut line: u32 = 1;
    let mut depth: i32 = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                if depth <= 0 && !matches!(toks.last(), None | Some((Tok::Nl, _))) {
                    toks.push((Tok::Nl, line));
                }
                line += 1;
                i += 1;
            }
            ' ' | '\t' | '\r' => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '(' => { depth += 1; toks.push((Tok::LParen, line)); i += 1 }
            ')' => { depth -= 1; toks.push((Tok::RParen, line)); i += 1 }
            '[' => { depth += 1; toks.push((Tok::LBracket, line)); i += 1 }
            ']' => { depth -= 1; toks.push((Tok::RBracket, line)); i += 1 }
            '{' => { toks.push((Tok::LBrace, line)); i += 1 }
            '}' => { toks.push((Tok::RBrace, line)); i += 1 }
            ',' => { toks.push((Tok::Comma, line)); i += 1 }
            ':' => { toks.push((Tok::Colon, line)); i += 1 }
            '.' => { toks.push((Tok::Dot, line)); i += 1 }
            '+' => { toks.push((Tok::Plus, line)); i += 1 }
            '-' => { toks.push((Tok::Minus, line)); i += 1 }
            '*' => { toks.push((Tok::Star, line)); i += 1 }
            '/' => { toks.push((Tok::Slash, line)); i += 1 }
            '%' => { toks.push((Tok::Percent, line)); i += 1 }
            '=' => {
                if chars.get(i + 1) == Some(&'=') {
                    toks.push((Tok::EqEq, line));
                    i += 2;
                } else {
                    toks.push((Tok::Eq, line));
                    i += 1;
                }
            }
            '!' => {
                if chars.get(i + 1) == Some(&'=') {
                    toks.push((Tok::BangEq, line));
                    i += 2;
                } else {
                    return Err(format!("line {line}: unexpected '!' (use 'not')"));
                }
            }
            '<' => {
                if chars.get(i + 1) == Some(&'=') {
                    toks.push((Tok::Le, line));
                    i += 2;
                } else {
                    toks.push((Tok::Lt, line));
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') {
                    toks.push((Tok::Ge, line));
                    i += 2;
                } else {
                    toks.push((Tok::Gt, line));
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    match chars.get(i) {
                        None | Some('\n') => {
                            return Err(format!("line {line}: unterminated string"))
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some('\\') => {
                            i += 1;
                            match chars.get(i) {
                                Some('n') => { s.push('\n'); i += 1 }
                                Some('t') => { s.push('\t'); i += 1 }
                                Some('\\') => { s.push('\\'); i += 1 }
                                Some('"') => { s.push('"'); i += 1 }
                                Some('x') => {
                                    let hs: String = chars.iter().skip(i + 1).take(2).collect();
                                    if hs.len() < 2 || !hs.chars().all(|c| c.is_ascii_hexdigit()) {
                                        return Err(format!("line {line}: \\x needs two hex digits"));
                                    }
                                    let code = u32::from_str_radix(&hs, 16).unwrap();
                                    s.push(char::from_u32(code).unwrap());
                                    i += 3;
                                }
                                Some('u') => {
                                    if chars.get(i + 1) != Some(&'{') {
                                        return Err(format!("line {line}: \\u needs {{hex}}, e.g. \\u{{1F44B}}"));
                                    }
                                    let mut j = i + 2;
                                    let mut hs = String::new();
                                    while j < chars.len() && chars[j] != '}' {
                                        hs.push(chars[j]);
                                        j += 1;
                                    }
                                    if j >= chars.len()
                                        || hs.is_empty()
                                        || hs.len() > 6
                                        || !hs.chars().all(|c| c.is_ascii_hexdigit())
                                    {
                                        return Err(format!("line {line}: bad \\u{{...}} escape"));
                                    }
                                    let code = u32::from_str_radix(&hs, 16).unwrap();
                                    s.push(
                                        char::from_u32(code)
                                            .ok_or(format!("line {line}: invalid codepoint \\u{{{hs}}}"))?,
                                    );
                                    i = j + 1;
                                }
                                other => {
                                    return Err(format!("line {line}: unknown escape \\{other:?}"))
                                }
                            }
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                toks.push((Tok::Str(s), line));
            }
            c if c.is_ascii_digit() => {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len()
                    && chars[j] == '.'
                    && chars.get(j + 1).is_some_and(|c| c.is_ascii_digit())
                {
                    j += 1;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                let s: String = chars[i..j].iter().collect();
                toks.push((
                    Tok::Num(s.parse().map_err(|_| format!("line {line}: bad number {s}"))?),
                    line,
                ));
                i = j;
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut j = i;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                toks.push((Tok::Ident(chars[i..j].iter().collect()), line));
                i = j;
            }
            other => return Err(format!("line {line}: unexpected character {other:?}")),
        }
    }
    Ok(toks)
}

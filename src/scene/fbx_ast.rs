//! Токенизатор и парсер FBX ASCII (7.x).
//!
//! Grammar:
//!   File     := (Stmt Newline*)*
//!   Stmt     := Ident ':' Args ('{' (Stmt Newline*)* '}')?
//!   Args     := Value (','? Value)*
//!   Value    := Int | Float | String | Ident | '*' Int
//!
//! Newlines внутри Args разрешены, если следующая строка не начинается
//! с `Ident ':'` (т.е. не новый statement) и не `}`.

use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone)]
pub enum FbxArg {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Star(u64),
    // === Массивы (только binary FBX порождает их) ===
    IntArray(Vec<i64>),
    FloatArray(Vec<f64>),
    BoolArray(Vec<u8>),
    Raw(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct FbxNode {
    pub name: String,
    pub args: Vec<FbxArg>,
    pub children: Vec<FbxNode>,
}

impl FbxNode {
    pub fn child(&self, name: &str) -> Option<&FbxNode> {
        self.children.iter().find(|n| n.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a FbxNode> {
        self.children.iter().filter(move |n| n.name == name)
    }

    pub fn first_int(&self) -> Option<i64> {
        self.args.iter().find_map(|a| match a {
            FbxArg::Int(i) => Some(*i),
            _ => None,
        })
    }

    pub fn first_str(&self) -> Option<&str> {
        self.args.iter().find_map(|a| match a {
            FbxArg::Str(s) => Some(s.as_str()),
            FbxArg::Ident(s) => Some(s.as_str()),
            _ => None,
        })
    }

    /// Все скалярные числовые args как f32.
    /// Массивы НЕ включаются (для них — `get_array_f32`).
    pub fn floats(&self) -> Vec<f32> {
        self.args
            .iter()
            .filter_map(|a| match a {
                FbxArg::Int(i) => Some(*i as f32),
                FbxArg::Float(f) => Some(*f as f32),
                _ => None,
            })
            .collect()
    }

    /// Универсальный доступ к массиву f32.
    ///
    /// Binary: `args` содержит `FloatArray`/`IntArray`/`BoolArray`.
    /// ASCII:  `args` содержит `Star(N)`, а данные лежат в child-узле `a`.
    pub fn get_array_f32(&self) -> Option<Vec<f32>> {
        for a in &self.args {
            match a {
                FbxArg::FloatArray(v) => {
                    return Some(v.iter().map(|&x| x as f32).collect());
                }
                FbxArg::IntArray(v) => {
                    return Some(v.iter().map(|&x| x as f32).collect());
                }
                FbxArg::BoolArray(v) => {
                    return Some(v.iter().map(|&x| x as f32).collect());
                }
                _ => {}
            }
        }
        // ASCII-fallback.
        let a = self.child("a")?;
        Some(
            a.args
                .iter()
                .filter_map(|x| match x {
                    FbxArg::Int(i) => Some(*i as f32),
                    FbxArg::Float(f) => Some(*f as f32),
                    _ => None,
                })
                .collect(),
        )
    }

    /// Универсальный доступ к массиву i64.
    pub fn get_array_i64(&self) -> Option<Vec<i64>> {
        for a in &self.args {
            match a {
                FbxArg::IntArray(v) => return Some(v.clone()),
                FbxArg::FloatArray(v) => {
                    return Some(v.iter().map(|&x| x as i64).collect());
                }
                FbxArg::BoolArray(v) => {
                    return Some(v.iter().map(|&x| x as i64).collect());
                }
                _ => {}
            }
        }
        let a = self.child("a")?;
        Some(
            a.args
                .iter()
                .filter_map(|x| match x {
                    FbxArg::Int(i) => Some(*i),
                    FbxArg::Float(f) => Some(*f as i64),
                    _ => None,
                })
                .collect(),
        )
    }
}

// ============================================================
// Tokenizer
// ============================================================

#[derive(Debug, Clone)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64, bool),   // (value, is_int)
    Star(u64),
    Colon,
    Comma,
    LBrace,
    RBrace,
    Newline,
}

fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;

        if c == '\r' { i += 1; continue; }
        if c == '\n' {
            out.push(Tok::Newline);
            i += 1;
            continue;
        }
        if c == ' ' || c == '\t' { i += 1; continue; }

        if c == ';' {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }

        match c {
            ':' => { out.push(Tok::Colon); i += 1; }
            ',' => { out.push(Tok::Comma); i += 1; }
            '{' => { out.push(Tok::LBrace); i += 1; }
            '}' => { out.push(Tok::RBrace); i += 1; }
            '"' => {
                i += 1;
                let start = i;
                while i < b.len() && b[i] != b'"' { i += 1; }
                if i >= b.len() {
                    bail!("unterminated string at byte {}", start);
                }
                let s = std::str::from_utf8(&b[start..i])?.to_string();
                i += 1;
                out.push(Tok::Str(s));
            }
            '*' => {
                i += 1;
                let start = i;
                while i < b.len() && (b[i] as char).is_ascii_digit() { i += 1; }
                let s = std::str::from_utf8(&b[start..i])?;
                let n: u64 = s.parse()
                    .map_err(|e| anyhow!("bad *count '{}': {}", s, e))?;
                out.push(Tok::Star(n));
            }
            _ if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                let start = i;
                if c == '-' || c == '+' { i += 1; }
                let mut has_dot = false;
                let mut has_exp = false;
                while i < b.len() {
                    let c2 = b[i] as char;
                    if c2.is_ascii_digit() { i += 1; }
                    else if c2 == '.' && !has_dot { has_dot = true; i += 1; }
                    else if (c2 == 'e' || c2 == 'E') && !has_exp {
                        has_exp = true;
                        i += 1;
                        if i < b.len() && (b[i] == b'-' || b[i] == b'+') { i += 1; }
                    } else { break; }
                }
                let s = std::str::from_utf8(&b[start..i])?;
                let v: f64 = s.parse()
                    .map_err(|e| anyhow!("bad number '{}': {}", s, e))?;
                let is_int = !has_dot && !has_exp;
                out.push(Tok::Num(v, is_int));
            }
            _ if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() {
                    let c2 = b[i] as char;
                    if c2.is_ascii_alphanumeric() || c2 == '_' || c2 == '.' {
                        i += 1;
                    } else { break; }
                }
                let s = std::str::from_utf8(&b[start..i])?.to_string();
                out.push(Tok::Ident(s));
            }
            _ => bail!("unexpected char '{}' at byte {}", c, i),
        }
    }
    Ok(out)
}

// ============================================================
// Parser
// ============================================================

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn peek_at(&self, off: usize) -> Option<&Tok> { self.toks.get(self.pos + off) }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() { self.pos += 1; }
        t
    }

    fn eat_colon(&mut self) -> bool {
        if matches!(self.peek(), Some(Tok::Colon)) { self.pos += 1; true } else { false }
    }
    fn eat_lbrace(&mut self) -> bool {
        if matches!(self.peek(), Some(Tok::LBrace)) { self.pos += 1; true } else { false }
    }
    fn eat_rbrace(&mut self) -> bool {
        if matches!(self.peek(), Some(Tok::RBrace)) { self.pos += 1; true } else { false }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(Tok::Newline)) { self.pos += 1; }
    }

    fn parse_body(&mut self) -> Result<Vec<FbxNode>> {
        let mut nodes = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek().is_none() || matches!(self.peek(), Some(Tok::RBrace)) {
                break;
            }
            nodes.push(self.parse_node()?);
        }
        Ok(nodes)
    }

    fn parse_node(&mut self) -> Result<FbxNode> {
        let name = match self.next() {
            Some(Tok::Ident(s)) => s,
            Some(t) => bail!("expected ident at pos {}, got {:?}", self.pos, t),
            None => bail!("unexpected EOF while reading a node name"),
        };
        if !self.eat_colon() {
            bail!("expected ':' after '{}'", name);
        }
        let args = self.parse_arg_list()?;
        let children = if self.eat_lbrace() {
            let body = self.parse_body()?;
            if !self.eat_rbrace() {
                bail!("expected '}}' closing '{}'", name);
            }
            body
        } else {
            Vec::new()
        };
        Ok(FbxNode { name, args, children })
    }

    fn parse_arg_list(&mut self) -> Result<Vec<FbxArg>> {
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Tok::LBrace) | Some(Tok::RBrace) | None => break,
                Some(Tok::Newline) => {
                    let mut j = 1;
                    while matches!(self.peek_at(j), Some(Tok::Newline)) { j += 1; }
                    let next = self.peek_at(j);
                    let stop = matches!(next, Some(Tok::RBrace))
                        || (matches!(next, Some(Tok::Ident(_)))
                            && matches!(self.peek_at(j + 1), Some(Tok::Colon)));
                    if stop { break; }
                    self.pos += 1;
                    continue;
                }
                _ => {}
            }
            match self.next() {
                Some(Tok::Num(v, is_int)) => {
                    args.push(if is_int { FbxArg::Int(v as i64) } else { FbxArg::Float(v) });
                }
                Some(Tok::Str(s)) => args.push(FbxArg::Str(s)),
                Some(Tok::Ident(s)) => args.push(FbxArg::Ident(s)),
                Some(Tok::Star(n)) => args.push(FbxArg::Star(n)),
                Some(Tok::Comma) => {}
                Some(_) => { self.pos -= 1; break; }
                None => break,
            }
        }
        Ok(args)
    }
}

pub fn parse_fbx_ascii(src: &str) -> Result<Vec<FbxNode>> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    p.parse_body()
}
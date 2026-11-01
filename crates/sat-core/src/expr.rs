//! The expression language of filters, sticker rules and counter cases (ADR 0013 §2). @sat:expr
//!
//! Owns: parsing an expression into a checked tree (positions in errors), and evaluating it against any source of
//! metric values.
//! Not here: what the metrics are and where values come from (`@sat:metrics`).
//!
//! `size > 1 GB && plugins > 0`, `ext.dds >= 100 || name ~ "texture"`, `!enabled && unique > 0`,
//! `max(wins, loses) > 10`. The monolith's counter language (`compileCounterExpressionV529`) plus `~` (contains,
//! ignoring case) and size units (`KB MB GB TB`, powers of 1024). Identifiers are metric ids — there is nothing
//! else to reach, so nothing to block. A metric without a value (not scanned yet) is `null`: comparisons with it
//! are false, arithmetic with it is `null`.

use std::fmt;

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Text(String),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Text(s) => !s.is_empty(),
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }
}

/// A parse or check error, with the byte offset where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub at: usize,
    pub message: String,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at {}: {}", self.at, self.message)
    }
}

impl std::error::Error for ExprError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Lit(Value),
    Metric(String),
    Not(Box<Node>),
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Min,
    Max,
    Round,
    Floor,
    Ceil,
    Abs,
    Clamp,
}

// @sat:expr.compile
/// A checked expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    root: Node,
    source: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Text(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

const UNITS: &[(&str, f64)] = &[
    ("tb", 1_099_511_627_776.0),
    ("gb", 1_073_741_824.0),
    ("mb", 1_048_576.0),
    ("kb", 1024.0),
];

fn err<T>(at: usize, message: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError {
        at,
        message: message.into(),
    })
}

fn lex(src: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            let text: String = src[start..i].chars().filter(|&c| c != '_').collect();
            let mut n: f64 = text.parse().map_err(|_| ExprError {
                at: start,
                message: format!("bad number {text:?}"),
            })?;
            // a size unit may follow, with or without a space
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let word_end = (j..b.len())
                .find(|&k| !b[k].is_ascii_alphabetic())
                .unwrap_or(b.len());
            if let Some((_, m)) = UNITS
                .iter()
                .find(|(u, _)| src[j..word_end].eq_ignore_ascii_case(u))
            {
                n *= m;
                i = word_end;
            }
            out.push((start, Tok::Num(n)));
            continue;
        }
        if c == b'"' || c == b'\'' {
            let q = c;
            i += 1;
            let mut s = String::new();
            loop {
                match b.get(i) {
                    None => return err(start, "unterminated text"),
                    Some(&x) if x == q => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') if b.get(i + 1).is_some() => {
                        s.push(src[i + 1..].chars().next().expect("char"));
                        i += 1 + src[i + 1..].chars().next().expect("char").len_utf8();
                    }
                    Some(_) => {
                        let ch = src[i..].chars().next().expect("char");
                        s.push(ch);
                        i += ch.len_utf8();
                    }
                }
            }
            out.push((start, Tok::Text(s)));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push((start, Tok::Ident(src[start..i].to_string())));
            continue;
        }
        let two = src.get(i..i + 2).unwrap_or("");
        let op: Option<&'static str> = ["&&", "||", "==", "!=", "<=", ">="]
            .into_iter()
            .find(|o| *o == two)
            .or_else(|| {
                ["<", ">", "+", "-", "*", "/", "%", "!", "~"]
                    .into_iter()
                    .find(|o| o.as_bytes()[0] == c)
            });
        match (op, c) {
            (Some(o), _) => {
                i += o.len();
                out.push((start, Tok::Op(o)));
            }
            (None, b'(') => {
                i += 1;
                out.push((start, Tok::LParen));
            }
            (None, b')') => {
                i += 1;
                out.push((start, Tok::RParen));
            }
            (None, b',') => {
                i += 1;
                out.push((start, Tok::Comma));
            }
            _ => {
                return err(
                    start,
                    format!("unexpected {:?}", src[i..].chars().next().unwrap_or('?')),
                );
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<(usize, Tok)>,
    pos: usize,
    end: usize,
    known: &'a dyn Fn(&str) -> bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(_, t)| t)
    }
    fn at(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |(a, _)| *a)
    }
    fn eat_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) if ops.contains(o) => {
                let o = *o;
                self.pos += 1;
                Some(o)
            }
            _ => None,
        }
    }
    fn binary(
        &mut self,
        ops: &[&'static str],
        next: fn(&mut Self) -> Result<Node, ExprError>,
    ) -> Result<Node, ExprError> {
        let mut left = next(self)?;
        while let Some(o) = self.eat_op(ops) {
            let right = next(self)?;
            let op = match o {
                "||" => Op::Or,
                "&&" => Op::And,
                "==" => Op::Eq,
                "!=" => Op::Ne,
                "<" => Op::Lt,
                "<=" => Op::Le,
                ">" => Op::Gt,
                ">=" => Op::Ge,
                "~" => Op::Contains,
                "+" => Op::Add,
                "-" => Op::Sub,
                "*" => Op::Mul,
                "/" => Op::Div,
                _ => Op::Rem,
            };
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn or(&mut self) -> Result<Node, ExprError> {
        self.binary(&["||"], Self::and)
    }
    fn and(&mut self) -> Result<Node, ExprError> {
        self.binary(&["&&"], Self::cmp)
    }
    fn cmp(&mut self) -> Result<Node, ExprError> {
        self.binary(&["==", "!=", "<=", ">=", "<", ">", "~"], Self::add)
    }
    fn add(&mut self) -> Result<Node, ExprError> {
        self.binary(&["+", "-"], Self::mul)
    }
    fn mul(&mut self) -> Result<Node, ExprError> {
        self.binary(&["*", "/", "%"], Self::unary)
    }
    fn unary(&mut self) -> Result<Node, ExprError> {
        match self.eat_op(&["!", "-"]) {
            Some("!") => Ok(Node::Not(Box::new(self.unary()?))),
            Some(_) => Ok(Node::Neg(Box::new(self.unary()?))),
            None => self.primary(),
        }
    }
    fn primary(&mut self) -> Result<Node, ExprError> {
        let at = self.at();
        let Some((_, t)) = self.toks.get(self.pos).cloned() else {
            return err(at, "expression ends too early");
        };
        self.pos += 1;
        match t {
            Tok::Num(n) => Ok(Node::Lit(Value::Num(n))),
            Tok::Text(s) => Ok(Node::Lit(Value::Text(s))),
            Tok::LParen => {
                let e = self.or()?;
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(e)
                    }
                    _ => err(self.at(), "expected `)`"),
                }
            }
            Tok::Ident(id) => {
                let lower = id.to_lowercase();
                match lower.as_str() {
                    "true" => return Ok(Node::Lit(Value::Bool(true))),
                    "false" => return Ok(Node::Lit(Value::Bool(false))),
                    "null" => return Ok(Node::Lit(Value::Null)),
                    _ => {}
                }
                if self.peek() == Some(&Tok::LParen) {
                    let (f, min, max) = match lower.as_str() {
                        "min" => (Func::Min, 1, usize::MAX),
                        "max" => (Func::Max, 1, usize::MAX),
                        "round" => (Func::Round, 1, 1),
                        "floor" => (Func::Floor, 1, 1),
                        "ceil" => (Func::Ceil, 1, 1),
                        "abs" => (Func::Abs, 1, 1),
                        "clamp" => (Func::Clamp, 3, 3),
                        _ => return err(at, format!("unknown function {id:?}")),
                    };
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.or()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.pos += 1,
                                _ => break,
                            }
                        }
                    }
                    if self.peek() != Some(&Tok::RParen) {
                        return err(self.at(), "expected `)`");
                    }
                    self.pos += 1;
                    if args.len() < min || args.len() > max {
                        return err(
                            at,
                            format!(
                                "{id} takes {min}{} argument(s)",
                                if max == usize::MAX { "+" } else { "" }
                            ),
                        );
                    }
                    return Ok(Node::Call(f, args));
                }
                if !(self.known)(&lower) {
                    return err(at, format!("unknown metric {id:?}"));
                }
                Ok(Node::Metric(lower))
            }
            _ => err(at, "expected a value"),
        }
    }
}

impl Expr {
    /// Parses and checks; `known` says whether an identifier is a metric.
    pub fn compile(source: &str, known: &dyn Fn(&str) -> bool) -> Result<Self, ExprError> {
        let mut p = Parser {
            toks: lex(source)?,
            pos: 0,
            end: source.len(),
            known,
        };
        if p.toks.is_empty() {
            return err(0, "empty expression");
        }
        let root = p.or()?;
        if p.pos < p.toks.len() {
            return err(p.at(), "unexpected text after the expression");
        }
        Ok(Self {
            root,
            source: source.to_string(),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Metric ids the expression reads (to know which facts it needs).
    pub fn metrics(&self) -> Vec<&str> {
        fn walk<'a>(n: &'a Node, out: &mut Vec<&'a str>) {
            match n {
                Node::Metric(m) => out.push(m),
                Node::Not(a) | Node::Neg(a) => walk(a, out),
                Node::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Node::Call(_, args) => args.iter().for_each(|a| walk(a, out)),
                Node::Lit(_) => {}
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v.sort_unstable();
        v.dedup();
        v
    }

    // @sat:expr.eval
    /// Evaluates with metric values from `get` (ids are lower case).
    pub fn eval(&self, get: &dyn Fn(&str) -> Value) -> Value {
        eval(&self.root, get)
    }

    /// Whether the expression holds (a filter, a sticker rule, a counter case).
    pub fn test(&self, get: &dyn Fn(&str) -> Value) -> bool {
        self.eval(get).truthy()
    }
}

fn eval(n: &Node, get: &dyn Fn(&str) -> Value) -> Value {
    use Value::{Bool, Null, Num, Text};
    match n {
        Node::Lit(v) => v.clone(),
        Node::Metric(m) => get(m),
        Node::Not(a) => Bool(!eval(a, get).truthy()),
        Node::Neg(a) => eval(a, get).num().map_or(Null, |x| Num(-x)),
        Node::Bin(Op::Or, a, b) => Bool(eval(a, get).truthy() || eval(b, get).truthy()),
        Node::Bin(Op::And, a, b) => Bool(eval(a, get).truthy() && eval(b, get).truthy()),
        Node::Bin(op, a, b) => {
            let (x, y) = (eval(a, get), eval(b, get));
            match op {
                Op::Contains => match (&x, &y) {
                    (Text(h), Text(n)) => Bool(h.to_lowercase().contains(&n.to_lowercase())),
                    _ => Bool(false),
                },
                Op::Eq | Op::Ne => {
                    let same = match (&x, &y) {
                        (Null, _) | (_, Null) => return Bool(false),
                        (Text(p), Text(q)) => p.to_lowercase() == q.to_lowercase(),
                        _ => match (x.num(), y.num()) {
                            (Some(p), Some(q)) => p == q,
                            _ => false,
                        },
                    };
                    Bool(same == (*op == Op::Eq))
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => match (x.num(), y.num()) {
                    (Some(p), Some(q)) => Bool(match op {
                        Op::Lt => p < q,
                        Op::Le => p <= q,
                        Op::Gt => p > q,
                        _ => p >= q,
                    }),
                    _ => Bool(false),
                },
                _ => match (x.num(), y.num()) {
                    (Some(p), Some(q)) => match op {
                        Op::Add => Num(p + q),
                        Op::Sub => Num(p - q),
                        Op::Mul => Num(p * q),
                        Op::Div if q != 0.0 => Num(p / q),
                        Op::Rem if q != 0.0 => Num(p % q),
                        _ => Null,
                    },
                    _ => Null,
                },
            }
        }
        Node::Call(f, args) => {
            let v: Option<Vec<f64>> = args.iter().map(|a| eval(a, get).num()).collect();
            let Some(v) = v else { return Null };
            Num(match f {
                Func::Min => v.iter().copied().fold(f64::INFINITY, f64::min),
                Func::Max => v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                Func::Round => v[0].round(),
                Func::Floor => v[0].floor(),
                Func::Ceil => v[0].ceil(),
                Func::Abs => v[0].abs(),
                Func::Clamp => v[0].clamp(v[1].min(v[2]), v[1].max(v[2])),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(id: &str) -> bool {
        [
            "size", "plugins", "name", "enabled", "wins", "loses", "ext.dds", "vram",
        ]
        .contains(&id)
    }

    fn mod_a(id: &str) -> Value {
        match id {
            "size" => Value::Num(3.0 * 1_073_741_824.0),
            "plugins" => Value::Num(2.0),
            "name" => Value::Text("Skyrim 2020 Textures".into()),
            "enabled" => Value::Bool(true),
            "wins" => Value::Num(40.0),
            "loses" => Value::Num(7.0),
            "ext.dds" => Value::Num(1500.0),
            _ => Value::Null, // vram: not scanned
        }
    }

    fn t(src: &str) -> bool {
        Expr::compile(src, &known).unwrap().test(&mod_a)
    }

    #[test]
    fn filters_read_like_the_owner_writes_them() {
        assert!(t("size > 1 GB"));
        assert!(t("size > 1GB && plugins > 0"));
        assert!(!t("size > 4 gb"));
        assert!(t("ext.dds >= 1_000 || name ~ \"nothing\""));
        assert!(t("name ~ 'TEXTURES' && enabled"));
        assert!(t("!(loses > wins)"));
        assert!(t("max(wins, loses) == 40 && clamp(loses, 0, 5) == 5"));
        assert!(t(
            "size / 1 MB > 3000 && round(2.6) == 3 && 7 % 4 == 3 && -plugins < 0"
        ));
        assert!(
            t("name == \"skyrim 2020 textures\""),
            "text compares ignoring case"
        );
    }

    #[test]
    fn a_metric_without_a_value_is_null_and_never_matches() {
        assert!(!t("vram > 0"));
        assert!(!t("vram == 0"));
        assert!(!t("vram + 1 > 0"));
        assert!(t("!(vram > 0)"));
    }

    #[test]
    fn errors_point_at_the_place() {
        let e = Expr::compile("size > 1 GB && weight > 2", &known).unwrap_err();
        assert_eq!((e.at, e.message.as_str()), (15, "unknown metric \"weight\""));
        assert_eq!(
            Expr::compile("size >", &known).unwrap_err().message,
            "expression ends too early"
        );
        assert_eq!(
            Expr::compile("(size > 1", &known).unwrap_err().message,
            "expected `)`"
        );
        assert!(Expr::compile("clamp(size)", &known).is_err());
        assert!(Expr::compile("name ~ \"open", &known).is_err());
        assert!(Expr::compile("size > 1 2", &known).is_err());
        assert!(Expr::compile("", &known).is_err());
        assert_eq!(
            Expr::compile("plugins > 0 && size > 0", &known)
                .unwrap()
                .metrics(),
            ["plugins", "size"]
        );
    }
}

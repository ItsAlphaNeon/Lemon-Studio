// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The formula language of links ("1-Input", "Input*0.8") and of Fruity Formula Controller ("a+Rand*b",
//! "Frac(SongTime/4)"): numbers, variables, operators and functions over doubles. Names are not case-sensitive. What a
//! target gets is the result put into 0..1.
//!
// spec: plugins/fruity-formula-controller.md "The formula language", flp/events/automation.md "Controller links",
// plugins/lemon-peak-controller.md "Links"

/// A parsed formula.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula(Node);

/// What a formula's variables are when it is worked out.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vars {
    /// The value a link carries (`Input`).
    pub input: f64,
    /// Formula Controller's knobs, 0..1.
    pub a: f64,
    pub b: f64,
    pub c: f64,
    /// The song position in beats (`SongTime`).
    pub song_time: f64,
    /// Beats per minute (`GetTempo`).
    pub tempo: f64,
    /// The system clock: milliseconds (`Time`) and days (`Date`).
    pub time_ms: f64,
    pub date: f64,
    /// The mouse position (`MouseX`, `MouseY`).
    pub mouse_x: f64,
    pub mouse_y: f64,
}

/// The random numbers of `Rand` (xorshift; the original's own generator is not reproduced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng(pub u32);

impl Default for Rng {
    fn default() -> Self {
        Rng(0x2545_F491)
    }
}

impl Rng {
    /// A uniform value in 0..1.
    pub fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        f64::from(x) / 4_294_967_296.0
    }
}

/// A value in 0..1 for a seed: the same seed gives the same value (`SeededRand`).
pub fn seeded(seed: f64) -> f64 {
    let s = if seed.is_finite() { seed.trunc() as i64 as u64 } else { 0 };
    // SplitMix64.
    let mut z = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    Input,
    A,
    B,
    C,
    SongTime,
    Tempo,
    Time,
    Date,
    MouseX,
    MouseY,
    Rand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    IntDiv,
    Mod,
    FMod,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    Var(Var),
    Neg(Box<Node>),
    Not(Box<Node>),
    Percent(Box<Node>),
    Fact(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

macro_rules! funcs {
    ($($f:ident $n:literal [$($name:literal),+]),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Func { $($f),* }

        impl Func {
            /// The function and how many arguments it takes.
            fn of(name: &str) -> Option<(Func, usize)> {
                match name {
                    $($($name)|+ => Some((Func::$f, $n)),)*
                    _ => None,
                }
            }
        }
    };
}

funcs! {
    Sin 1 ["sin"], Cos 1 ["cos"], Tan 1 ["tan", "tg"], Ctg 1 ["ctg", "cot"], Sec 1 ["sec"], Cosec 1 ["cosec", "csc"],
    Sinh 1 ["sinh"], Cosh 1 ["cosh"], Tanh 1 ["tanh"],
    ArcSin 1 ["arcsin", "asin"], ArcCos 1 ["arccos", "acos"], ArcTg 1 ["arctg", "arctan", "atan"],
    ArcCosh 1 ["arccosh"], ArcTanh 1 ["arctanh"], ATan2 2 ["atan2"],
    DegToRad 1 ["degtorad"], RadToDeg 1 ["radtodeg"],
    Exp 1 ["exp"], Ln 1 ["ln"], Log10 1 ["log10", "log"], Log2 1 ["log2"], LogN 2 ["logn"],
    Sqrt 1 ["sqrt"], Sqr 1 ["sqr"], Power 2 ["power", "pow"], IntPower 2 ["intpower"],
    Abs 1 ["abs"], Neg 1 ["neg"], Ceil 1 ["ceil"], Floor 1 ["floor"], Round 1 ["round"], Trunc 1 ["trunc", "int"],
    Frac 1 ["frac"], Sign 1 ["sign"],
    Case 3 ["case"], If 3 ["if"], IfE 2 ["ife"], IfG 2 ["ifg"], IfGE 2 ["ifge"], IfL 2 ["ifl"], IfLE 2 ["ifle"],
    Inter 3 ["inter"], InterNoSwap 3 ["internoswap"],
    Min 2 ["min"], Max 2 ["max"], Sum 2 ["sum"],
    Fold 1 ["fold"], Warp 2 ["warp"], Map 3 ["map"], Tension 2 ["tension"],
    SeededRand 1 ["seededrand"],
}

/// Why a formula was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Nothing to work out.
    Empty,
    /// A name that is no variable, constant or function.
    Unknown(String),
    /// A function or operator without all its arguments.
    Arguments,
    /// Something else where it does not belong (its byte offset).
    Syntax(usize),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Empty => write!(f, "no formula"),
            Error::Unknown(n) => write!(f, "unknown name {n}"),
            Error::Arguments => write!(f, "function or operand has too few arguments"),
            Error::Syntax(at) => write!(f, "unexpected character at {}", at + 1),
        }
    }
}

impl Formula {
    /// Parses `text`; `None` when it is not a formula (a link then passes its value on as it is).
    pub fn parse(text: &str) -> Option<Formula> {
        Formula::compile(text).ok()
    }

    /// Parses `text`, saying what is wrong with it.
    pub fn compile(text: &str) -> Result<Formula, Error> {
        let mut p = Parser { s: text.as_bytes(), at: 0 };
        if p.peek().is_none() {
            return Err(Error::Empty);
        }
        let n = p.expr()?;
        match p.peek() {
            None => Ok(Formula(n)),
            Some(b')' | b',') => Err(Error::Syntax(p.at)),
            Some(_) => Err(Error::Syntax(p.at)),
        }
    }

    /// The formula for a link's `input`, put into 0..1 (NaN gives 0).
    pub fn map(&self, input: f64) -> f64 {
        self.value(&Vars { input, ..Vars::default() }, &mut Rng::default())
    }

    /// The formula's value, put into 0..1 (NaN gives 0).
    pub fn value(&self, v: &Vars, rng: &mut Rng) -> f64 {
        let x = self.eval(v, rng);
        if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
    }

    /// The formula's value as worked out.
    pub fn eval(&self, v: &Vars, rng: &mut Rng) -> f64 {
        eval(&self.0, v, rng)
    }

    /// Plain "Input": the value as it is.
    pub fn is_identity(&self) -> bool {
        self.0 == Node::Var(Var::Input)
    }
}

fn truth(x: f64) -> bool {
    x > 0.5
}

fn bool01(b: bool) -> f64 {
    f64::from(u8::from(b))
}

/// An int of `x` for the integer operators (rounded, ties to even, as the original does with fractions: spec
/// "Operators").
fn int(x: f64) -> i64 {
    if x.is_finite() { x.round_ties_even() as i64 } else { 0 }
}

fn eval(n: &Node, v: &Vars, rng: &mut Rng) -> f64 {
    match n {
        Node::Num(x) => *x,
        Node::Var(var) => match var {
            Var::Input => v.input,
            Var::A => v.a,
            Var::B => v.b,
            Var::C => v.c,
            Var::SongTime => v.song_time,
            Var::Tempo => v.tempo,
            Var::Time => v.time_ms,
            Var::Date => v.date,
            Var::MouseX => v.mouse_x,
            Var::MouseY => v.mouse_y,
            Var::Rand => rng.next(),
        },
        Node::Neg(a) => -eval(a, v, rng),
        Node::Not(a) => bool01(!truth(eval(a, v, rng))),
        Node::Percent(a) => eval(a, v, rng) / 100.0,
        // The factorial of the nearest whole number (ties to even, as Round): 1 for 0 and below.
        Node::Fact(a) => {
            let x = eval(a, v, rng).round_ties_even();
            if x.is_nan() { x } else { (1..=x.clamp(0.0, 171.0) as u32).map(f64::from).product() }
        }
        Node::Bin(op, a, b) => {
            let (a, b) = (eval(a, v, rng), eval(b, v, rng));
            match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                Op::Pow => a.powf(b),
                Op::IntDiv => {
                    let (x, y) = (int(a), int(b));
                    if y == 0 { f64::NAN } else { (x / y) as f64 }
                }
                Op::Mod => {
                    let (x, y) = (int(a), int(b));
                    if y == 0 { f64::NAN } else { (x % y) as f64 }
                }
                Op::FMod => a % b,
                Op::Shl => (int(a).wrapping_shl(int(b) as u32)) as f64,
                Op::Shr => (int(a).wrapping_shr(int(b) as u32)) as f64,
                Op::And => bool01(truth(a) && truth(b)),
                Op::Or => bool01(truth(a) || truth(b)),
                Op::Xor => bool01(truth(a) != truth(b)),
                Op::Eq => bool01(a == b),
                Op::Ne => bool01(a != b),
                Op::Lt => bool01(a < b),
                Op::Le => bool01(a <= b),
                Op::Gt => bool01(a > b),
                Op::Ge => bool01(a >= b),
            }
        }
        Node::Call(f, args) => {
            let x: Vec<f64> = args.iter().map(|n| eval(n, v, rng)).collect();
            call(*f, &x)
        }
    }
}

fn call(f: Func, x: &[f64]) -> f64 {
    let a = x[0];
    let b = || x[1];
    let c = || x[2];
    match f {
        Func::Sin => a.sin(),
        Func::Cos => a.cos(),
        Func::Tan => a.tan(),
        Func::Ctg => 1.0 / a.tan(),
        Func::Sec => 1.0 / a.cos(),
        Func::Cosec => 1.0 / a.sin(),
        Func::Sinh => a.sinh(),
        Func::Cosh => a.cosh(),
        Func::Tanh => a.tanh(),
        Func::ArcSin => a.asin(),
        Func::ArcCos => a.acos(),
        Func::ArcTg => a.atan(),
        Func::ArcCosh => a.acosh(),
        Func::ArcTanh => a.atanh(),
        Func::ATan2 => a.atan2(b()),
        Func::DegToRad => a.to_radians(),
        Func::RadToDeg => a.to_degrees(),
        Func::Exp => a.exp(),
        Func::Ln => a.ln(),
        Func::Log10 => a.log10(),
        Func::Log2 => a.log2(),
        Func::LogN => b().ln() / a.ln(),
        Func::Sqrt => a.sqrt(),
        Func::Sqr => a * a,
        Func::Power => a.powf(b()),
        Func::IntPower => a.powi(int(b()) as i32),
        Func::Abs => a.abs(),
        Func::Neg => -a,
        Func::Ceil => a.ceil(),
        Func::Floor => a.floor(),
        Func::Round => a.round_ties_even(),
        Func::Trunc => a.trunc(),
        Func::Frac => a.fract(),
        Func::Sign => {
            if a > 0.0 { 1.0 } else if a < 0.0 { -1.0 } else { 0.0 }
        }
        Func::Case => {
            if a == 1.0 { b() } else { c() }
        }
        Func::If => {
            if truth(a) { b() } else { c() }
        }
        Func::IfE => bool01(a == b()),
        Func::IfG => bool01(a > b()),
        Func::IfGE => bool01(a >= b()),
        Func::IfL => bool01(a < b()),
        Func::IfLE => bool01(a <= b()),
        Func::Inter => {
            let (lo, hi) = if c() < b() { (c(), b()) } else { (b(), c()) };
            bool01(inside(a, lo, hi))
        }
        Func::InterNoSwap => bool01(inside(a, b(), c())),
        Func::Min => a.min(b()),
        Func::Max => a.max(b()),
        Func::Sum => a + b(),
        Func::Fold => fold(a),
        Func::Warp => warp(a, b()),
        Func::Map => a * (c() - b()) + b(),
        Func::Tension => tension(a, b()),
        Func::SeededRand => seeded(a),
    }
}

/// `x` between `lo` and `hi`, with their ends and a margin of [`INTER_MARGIN`] past them.
fn inside(x: f64, lo: f64, hi: f64) -> bool {
    x >= lo - INTER_MARGIN && x <= hi + INTER_MARGIN
}

/// How far past its ends `Inter` still holds (measured: 0.000477 holds, 0.000521 does not).
pub const INTER_MARGIN: f64 = 0.0005;

/// Folds `x` back into 0..1 at its ends (a triangle: 1.2 gives 0.8, -0.3 gives 0.3).
fn fold(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let p = x.rem_euclid(2.0);
    if p > 1.0 { 2.0 - p } else { p }
}

/// `x` as it is inside 0..1; past its ends, the fraction of `x + w` (taking the sign of `x + w`: below 0, mostly
/// negative). 1.04 with `w` 0.5 gives 0.54, 1.6 gives 0.1.
fn warp(x: f64, w: f64) -> f64 {
    if (0.0..=1.0).contains(&x) { x } else { (x + w).fract() }
}

/// `x` (0..1) curved by `t`: 0 leaves it, positive bends it up, negative down; the curve is exponential,
/// `(r^x - 1) / (r - 1)` with `r = 1 / (1 + t)` (`1 + |t|` for a negative `t`).
fn tension(x: f64, t: f64) -> f64 {
    if t == 0.0 || !t.is_finite() {
        return x;
    }
    let k = -t.signum() * t.abs().ln_1p();
    (k * x).exp_m1() / k.exp_m1()
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.s.get(self.at).is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.s.get(self.at).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == Some(c);
        if hit {
            self.at += 1;
        }
        hit
    }

    /// The word at the position (letters, digits, `_`), lowercased, without taking it.
    fn word(&mut self) -> Option<(String, usize)> {
        self.space();
        let start = self.at;
        if !self.s.get(start).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        let mut end = start;
        while self.s.get(end).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            end += 1;
        }
        Some((std::str::from_utf8(&self.s[start..end]).ok()?.to_ascii_lowercase(), end))
    }

    /// Takes the operator word `w` if it is next.
    fn eat_word(&mut self, w: &str) -> bool {
        match self.word() {
            Some((x, end)) if x == w => {
                self.at = end;
                true
            }
            _ => false,
        }
    }

    /// expr := and (('or' | 'xor') and)*
    fn expr(&mut self) -> Result<Node, Error> {
        let mut a = self.and()?;
        loop {
            let op = if self.eat_word("or") {
                Op::Or
            } else if self.eat_word("xor") {
                Op::Xor
            } else {
                return Ok(a);
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.and()?));
        }
    }

    /// and := not ('and' not)*
    fn and(&mut self) -> Result<Node, Error> {
        let mut a = self.not()?;
        while self.eat_word("and") {
            a = Node::Bin(Op::And, Box::new(a), Box::new(self.not()?));
        }
        Ok(a)
    }

    /// not := 'not' not | relation
    fn not(&mut self) -> Result<Node, Error> {
        if self.eat_word("not") {
            return Ok(Node::Not(Box::new(self.not()?)));
        }
        self.relation()
    }

    /// relation := sum (('=' | 'in' | '<>' | '<' | '<=' | '>' | '>=') sum)*
    fn relation(&mut self) -> Result<Node, Error> {
        let mut a = self.sum()?;
        loop {
            let op = match self.peek() {
                Some(b'=') => {
                    self.at += 1;
                    Op::Eq
                }
                Some(b'<') => {
                    self.at += 1;
                    if self.s.get(self.at) == Some(&b'>') {
                        self.at += 1;
                        Op::Ne
                    } else if self.s.get(self.at) == Some(&b'=') {
                        self.at += 1;
                        Op::Le
                    } else {
                        Op::Lt
                    }
                }
                Some(b'>') => {
                    self.at += 1;
                    if self.s.get(self.at) == Some(&b'=') {
                        self.at += 1;
                        Op::Ge
                    } else {
                        Op::Gt
                    }
                }
                _ if self.eat_word("in") => Op::Eq,
                _ => return Ok(a),
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.sum()?));
        }
    }

    /// sum := product (('+' | '-') product)*
    fn sum(&mut self) -> Result<Node, Error> {
        let mut a = self.product()?;
        loop {
            let op = match self.peek() {
                Some(b'+') => {
                    self.at += 1;
                    Op::Add
                }
                Some(b'-') => {
                    self.at += 1;
                    Op::Sub
                }
                _ => return Ok(a),
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.product()?));
        }
    }

    /// product := power (('*' | '/' | 'div' | 'mod' | 'fmod' | 'shl' | 'shr') power)*
    fn product(&mut self) -> Result<Node, Error> {
        let mut a = self.power()?;
        loop {
            let op = match self.peek() {
                Some(b'*') => {
                    self.at += 1;
                    Op::Mul
                }
                Some(b'/') => {
                    self.at += 1;
                    Op::Div
                }
                _ => match self.word() {
                    Some((w, end)) => {
                        let op = match w.as_str() {
                            "div" => Op::IntDiv,
                            "mod" => Op::Mod,
                            "fmod" => Op::FMod,
                            "shl" => Op::Shl,
                            "shr" => Op::Shr,
                            _ => return Ok(a),
                        };
                        self.at = end;
                        op
                    }
                    None => return Ok(a),
                },
            };
            a = Node::Bin(op, Box::new(a), Box::new(self.power()?));
        }
    }

    /// power := unary ('^' unary)*: from the left, the sign before it (`-0.5^2` is 0.25, `2^3^2` is 64).
    fn power(&mut self) -> Result<Node, Error> {
        let mut a = self.unary()?;
        while self.eat(b'^') {
            a = Node::Bin(Op::Pow, Box::new(a), Box::new(self.unary()?));
        }
        Ok(a)
    }

    /// unary := ('-' | '+') unary | postfix
    fn unary(&mut self) -> Result<Node, Error> {
        if self.eat(b'-') {
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        if self.eat(b'+') {
            return self.unary();
        }
        self.postfix()
    }

    /// postfix := atom ('%' | '!')*
    fn postfix(&mut self) -> Result<Node, Error> {
        let mut a = self.atom()?;
        loop {
            if self.eat(b'%') {
                a = Node::Percent(Box::new(a));
            } else if self.eat(b'!') {
                a = Node::Fact(Box::new(a));
            } else {
                return Ok(a);
            }
        }
    }

    fn atom(&mut self) -> Result<Node, Error> {
        let Some(c) = self.peek() else { return Err(Error::Arguments) };
        if c == b'(' {
            self.at += 1;
            let n = self.expr()?;
            return if self.eat(b')') { Ok(n) } else { Err(Error::Syntax(self.at)) };
        }
        if c.is_ascii_digit() || c == b'.' {
            let start = self.at;
            while self.s.get(self.at).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                self.at += 1;
            }
            // An exponent: 1e-3.
            if self.s.get(self.at).is_some_and(|c| *c == b'e' || *c == b'E')
                && self.s.get(self.at + 1).is_some_and(|c| c.is_ascii_digit() || *c == b'-' || *c == b'+')
            {
                self.at += 2;
                while self.s.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
            }
            let text = std::str::from_utf8(&self.s[start..self.at]).map_err(|_| Error::Syntax(start))?;
            return text.parse().map(Node::Num).map_err(|_| Error::Syntax(start));
        }
        let Some((name, end)) = self.word() else { return Err(Error::Syntax(self.at)) };
        self.at = end;
        let var = match name.as_str() {
            "input" => Some(Var::Input),
            "a" => Some(Var::A),
            "b" => Some(Var::B),
            "c" => Some(Var::C),
            "songtime" => Some(Var::SongTime),
            "gettempo" => Some(Var::Tempo),
            "time" => Some(Var::Time),
            "date" => Some(Var::Date),
            "mousex" => Some(Var::MouseX),
            "mousey" => Some(Var::MouseY),
            "rand" => Some(Var::Rand),
            "pi" => return Ok(Node::Num(std::f64::consts::PI)),
            _ => None,
        };
        if let Some(var) = var {
            // `SongTime()`, `Rand()`: the empty brackets may follow.
            let back = self.at;
            if !(self.eat(b'(') && self.eat(b')')) {
                self.at = back;
            }
            return Ok(Node::Var(var));
        }
        let Some((f, n)) = Func::of(&name) else { return Err(Error::Unknown(name)) };
        if !self.eat(b'(') {
            return Err(Error::Arguments);
        }
        let mut args = vec![self.expr()?];
        while self.eat(b',') {
            args.push(self.expr()?);
        }
        if args.len() < n {
            return Err(Error::Arguments);
        }
        if args.len() > n || !self.eat(b')') {
            return Err(Error::Syntax(self.at));
        }
        Ok(Node::Call(f, args))
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Formula, Rng, Vars};

    fn f(s: &str) -> Formula {
        Formula::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    fn at(s: &str, v: Vars) -> f64 {
        f(s).eval(&v, &mut Rng::default())
    }

    fn x(s: &str) -> f64 {
        at(s, Vars::default())
    }

    /// The formulas the corpus links carry.
    #[test]
    fn the_corpus_formulas() {
        assert_eq!(f("1-Input").map(0.25), 0.75);
        assert!((f("1-Input*0.8-0.2").map(0.25) - 0.6).abs() < 1e-12);
        assert!((f("1-Input*0.5-0.5").map(0.4) - 0.3).abs() < 1e-12);
        assert_eq!(f("1-Input*0.5-0.5").map(1.0), 0.0);
        assert_eq!(f("Input*2").map(0.7), 1.0, "put into 0..1");
        assert_eq!(f("Input*0.5").map(0.5), 0.25);
        assert!((f("1-Cos(Input*Pi)*0.5-0.5").map(0.5) - 0.5).abs() < 1e-12);
        assert!((f("(Input*.6+.4)*0.800003051757813").map(1.0) - 0.800003051757813).abs() < 1e-12);
        assert!((f("1-Input*1.6").map(0.25) - 0.6).abs() < 1e-12);
        assert!(f("Input").is_identity());
        assert!(f(" input ").is_identity());
    }

    #[test]
    fn precedence_functions_and_errors() {
        assert_eq!(x("2-1-0.5"), 0.5);
        assert_eq!(f("-Input+1").map(0.25), 0.75);
        assert_eq!(f("Input^2").map(0.5), 0.25);
        assert_eq!(f("max(Input, 0.3)").map(0.1), 0.3);
        assert_eq!(f("min(Input*4,1)/2").map(0.5), 0.5);
        assert_eq!(f("Input/0").map(0.5), 1.0, "infinity, put into 0..1");
        assert_eq!(Formula::compile(""), Err(Error::Empty));
        assert_eq!(Formula::compile("a*"), Err(Error::Arguments));
        assert_eq!(Formula::compile("foo(Input)"), Err(Error::Unknown("foo".into())));
        for bad in ["1-", "Inp", "(Input", "Input)", "max(1)", "max(1,2,3)", "sin"] {
            assert!(Formula::parse(bad).is_none(), "{bad}");
        }
    }

    /// The rules measured on the original (spec: plugins/fruity-formula-controller.md "Precedence" and "Operators"): each
    /// formula with the value the original gives it.
    #[test]
    fn the_originals_rules() {
        for (s, want) in [
            ("-0.5^2+0.5", 0.75),
            ("2^3^0.5/8", 8f64.sqrt() / 8.0),
            ("(0.2+0.3>0.4)*0.5", 0.5),
            ("(1>0.5 and 0.2)*0.5", 0.0),
            ("1 and 1*0.4", 0.0),
            ("(1 or 0+0.3)*0.5", 0.5),
            ("(not 0.2+0.3)*0.5", 0.5),
            ("1-20%", 0.8),
            ("2.5!/10", 0.2),
            ("7 div 2/10", 0.3),
            ("(-7 mod 3)*0.1+0.5", 0.4),
            ("(-7 div 2)*0.1+0.6", 0.3),
            ("Round(2.5)*0.1+0.5", 0.7),
            ("Round(-2.5)*0.1+0.6", 0.4),
            ("Int(-1.5)*0.1+0.5", 0.4),
            ("Frac(-1.25)*0.4+0.5", 0.4),
            ("(5.7 shr 1)*0.1", 0.3),
            ("(1.7 shl 1)*0.1", 0.4),
            ("(0.1+0.2=0.3)*0.5+0.25", 0.25),
            ("If(0.5,0.3,0.7)", 0.7),
            ("Case(0.9,0.3,0.7)", 0.7),
            ("1-6 div 4*0.5", 0.5),
            ("1-7 mod 4*0.2", 0.4),
            ("(1 xor 0>0.5)*0.5+0.1", 0.6),
        ] {
            let got = f(s).value(&Vars::default(), &mut Rng::default());
            assert!((got - want).abs() < 1e-12, "{s}: {got}");
        }
        assert_eq!(x("Inter(0.3004,0.6,0.3004)"), 1.0);
        assert_eq!(x("Inter(0.6004,0.3,0.6)"), 1.0);
        assert_eq!(x("Inter(0.6006,0.3,0.6)"), 0.0);
        assert!(Formula::parse("e*0.1").is_none(), "no constant e");
    }

    /// The formulas of the original's Formula Controller presets all compile.
    #[test]
    fn the_preset_formulas() {
        for s in [
            "a+b+c",
            "a+(Sin(SongTime*Pi*2*c)*b)^9*0.5",
            "a+Sin(SongTime*Pi*2*c)*Sin(SongTime/10)*(b-0.5)*Sin(SongTime*Pi*1.4*c)",
            "a+Sin(SongTime*Pi*2*c)*Sin(Time/200)*(b-0.5) ",
            "a+Frac(SongTime/(5-c*4.5))*(b*2-1)-b+0.5",
            "a+Round(Frac(SongTime/(5-c*4.5)))*(b*2-1)-b+0.5",
            "a+(Rand-0.5)*b",
            "a+(SeededRand(Round(SongTime*4))-0.5)*b",
            "IfLE(songtime,32)*(songtime/32)*0.5*sin(2*pi*songtime/4)+IfG(songtime,32)*(1-(songtime-32)/32)*0.5*sin(2*pi*songtime/4)+0.5",
            "Case(Round(1-c),a,b)",
            "Case(b,1-a,a)",
            "Min(Max(a,b),c)",
            "b+a*(c-b)",
            "Round(a*(b*16+1))/(b*16+1)",
            "Tension(a,(b-0.5)*100)",
            "MouseY",
        ] {
            assert!(Formula::compile(s).is_ok(), "{s}");
        }
        let v = Vars { a: 0.25, b: 1.0, ..Vars::default() };
        assert_eq!(at("Case(b,1-a,a)", v), 0.75);
    }

    /// Formula Controller's variables (spec: plugins/fruity-formula-controller.md "The formula language").
    #[test]
    fn variables() {
        let v = Vars { a: 0.25, b: 0.5, c: 0.75, song_time: 3.5, tempo: 130.0, ..Vars::default() };
        assert_eq!(at("a*0.5+b*0.25+c*0.125", v), 0.25 * 0.5 + 0.5 * 0.25 + 0.75 * 0.125);
        assert_eq!(at("Frac(SongTime)", v), 0.5);
        assert_eq!(at("SongTime()/7", v), 0.5);
        assert_eq!(at("GetTempo/260", v), 0.5);
        assert_eq!(at("SIN(pi/2)", v), 1.0);
    }

    #[test]
    fn operators_and_functions() {
        assert_eq!(x("50%*0.5"), 0.25);
        assert_eq!(x("3!"), 6.0);
        assert_eq!(x("7 div 2"), 3.0);
        assert_eq!(x("7 mod 3"), 1.0);
        assert!((x("2.5 fmod 0.7") - 0.4).abs() < 1e-12);
        assert_eq!(x("5 shr 1"), 2.0);
        assert_eq!(x("3 shl 2"), 12.0);
        assert_eq!(x("not 0.2"), 1.0);
        assert_eq!(x("1 and 0.6"), 1.0);
        assert_eq!(x("0.6 xor 0.2"), 1.0);
        assert_eq!(x("(2 in 2)+(1<>2)+(1<2)+(2<=2)+(3>2)+(3>=4)"), 5.0);
        assert_eq!(x("If(0.7,0.3,0.9)"), 0.3);
        assert_eq!(x("Case(1,0.3,0.9)"), 0.3);
        assert_eq!(x("Inter(0.5,0.6,0.3)"), 1.0);
        assert_eq!(x("InterNoSwap(0.5,0.6,0.3)"), 0.0);
        assert_eq!(x("Map(0.5,0.2,0.6)"), 0.4);
        assert_eq!(x("Fold(1.25)"), 0.75);
        assert!((x("LogN(2,8)") - 3.0).abs() < 1e-12);
        assert_eq!(x("IntPower(2,3)"), 8.0);
        assert_eq!(x("Sum(0.25,0.5)"), 0.75);
        assert!((x("ATan2(1,1)") - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert_eq!(x("Warp(0.3,0.5)"), 0.3);
        assert!((x("Warp(1.04,0.5)") - 0.54).abs() < 1e-12);
        assert!((x("Warp(1.6,0.5)") - 0.1).abs() < 1e-12);
        assert!((x("Tension(0.5,0.5)") - 0.550_510_257).abs() < 1e-6);
        assert!((x("Tension(0.5,-0.5)") - 0.449_489_743).abs() < 1e-6);
        assert_eq!(super::seeded(3.0), super::seeded(3.0));
        assert_ne!(super::seeded(3.0), super::seeded(4.0));
    }
}

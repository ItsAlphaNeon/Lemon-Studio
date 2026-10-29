// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mapping formulas of links ("1-Input", "Input*0.8", "1-Cos(Input*Pi)*0.5-0.5"): an expression of the value the link
//! carries (`Input`, 0..1), worked out in doubles; what the target gets is the result put into 0..1.
//!
// spec: flp/events/automation.md "Controller links" (the formula event), plugins/lemon-peak-controller.md "Links"

/// A parsed formula.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula(Node);

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    Input,
    Neg(Box<Node>),
    Bin(char, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Abs,
    Sqrt,
    Exp,
    Ln,
    Log10,
    Log2,
    Round,
    Trunc,
    Frac,
    Sign,
    Min,
    Max,
    Pow,
}

impl Func {
    fn of(name: &str) -> Option<(Func, usize)> {
        Some(match name {
            "sin" => (Func::Sin, 1),
            "cos" => (Func::Cos, 1),
            "tan" => (Func::Tan, 1),
            "abs" => (Func::Abs, 1),
            "sqrt" => (Func::Sqrt, 1),
            "exp" => (Func::Exp, 1),
            "ln" | "log" => (Func::Ln, 1),
            "log10" => (Func::Log10, 1),
            "log2" => (Func::Log2, 1),
            "round" => (Func::Round, 1),
            "trunc" | "int" => (Func::Trunc, 1),
            "frac" => (Func::Frac, 1),
            "sign" => (Func::Sign, 1),
            "min" => (Func::Min, 2),
            "max" => (Func::Max, 2),
            "pow" | "power" => (Func::Pow, 2),
            _ => return None,
        })
    }
}

impl Formula {
    /// Parses `text`; `None` when it is not a formula this module understands (the link then passes its value on as it
    /// is). Names are not case-sensitive.
    pub fn parse(text: &str) -> Option<Formula> {
        let mut p = Parser { s: text.as_bytes(), at: 0 };
        let n = p.sum()?;
        p.space();
        (p.at == p.s.len()).then_some(Formula(n))
    }

    /// The formula for `input`, put into 0..1 (NaN gives 0).
    pub fn map(&self, input: f64) -> f64 {
        let v = eval(&self.0, input);
        if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
    }

    /// Plain "Input": the value as it is.
    pub fn is_identity(&self) -> bool {
        self.0 == Node::Input
    }
}

fn eval(n: &Node, x: f64) -> f64 {
    match n {
        Node::Num(v) => *v,
        Node::Input => x,
        Node::Neg(a) => -eval(a, x),
        Node::Bin(op, a, b) => {
            let (a, b) = (eval(a, x), eval(b, x));
            match op {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                '/' => {
                    if b == 0.0 { 0.0 } else { a / b }
                }
                _ => a.powf(b),
            }
        }
        Node::Call(f, args) => {
            let a = eval(&args[0], x);
            let b = || eval(&args[1], x);
            match f {
                Func::Sin => a.sin(),
                Func::Cos => a.cos(),
                Func::Tan => a.tan(),
                Func::Abs => a.abs(),
                Func::Sqrt => a.max(0.0).sqrt(),
                Func::Exp => a.exp(),
                Func::Ln => a.ln(),
                Func::Log10 => a.log10(),
                Func::Log2 => a.log2(),
                Func::Round => a.round(),
                Func::Trunc => a.trunc(),
                Func::Frac => a.fract(),
                Func::Sign => {
                    if a > 0.0 { 1.0 } else if a < 0.0 { -1.0 } else { 0.0 }
                }
                Func::Min => a.min(b()),
                Func::Max => a.max(b()),
                Func::Pow => a.powf(b()),
            }
        }
    }
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

    /// sum := product (('+' | '-') product)*
    fn sum(&mut self) -> Option<Node> {
        let mut a = self.product()?;
        loop {
            let op = match self.peek() {
                Some(b'+') => '+',
                Some(b'-') => '-',
                _ => return Some(a),
            };
            self.at += 1;
            a = Node::Bin(op, Box::new(a), Box::new(self.product()?));
        }
    }

    /// product := unary (('*' | '/') unary)*
    fn product(&mut self) -> Option<Node> {
        let mut a = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(b'*') => '*',
                Some(b'/') => '/',
                _ => return Some(a),
            };
            self.at += 1;
            a = Node::Bin(op, Box::new(a), Box::new(self.unary()?));
        }
    }

    /// unary := ('-' | '+') unary | power
    fn unary(&mut self) -> Option<Node> {
        if self.eat(b'-') {
            return Some(Node::Neg(Box::new(self.unary()?)));
        }
        if self.eat(b'+') {
            return self.unary();
        }
        self.power()
    }

    /// power := atom ('^' unary)?
    fn power(&mut self) -> Option<Node> {
        let a = self.atom()?;
        if self.eat(b'^') {
            return Some(Node::Bin('^', Box::new(a), Box::new(self.unary()?)));
        }
        Some(a)
    }

    fn atom(&mut self) -> Option<Node> {
        let c = self.peek()?;
        if c == b'(' {
            self.at += 1;
            let n = self.sum()?;
            return self.eat(b')').then_some(n);
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
            return std::str::from_utf8(&self.s[start..self.at]).ok()?.parse().ok().map(Node::Num);
        }
        if c.is_ascii_alphabetic() {
            let start = self.at;
            while self.s.get(self.at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                self.at += 1;
            }
            let name = std::str::from_utf8(&self.s[start..self.at]).ok()?.to_ascii_lowercase();
            return match name.as_str() {
                "input" => Some(Node::Input),
                "pi" => Some(Node::Num(std::f64::consts::PI)),
                "e" => Some(Node::Num(std::f64::consts::E)),
                _ => {
                    let (f, n) = Func::of(&name)?;
                    if !self.eat(b'(') {
                        return None;
                    }
                    let mut args = vec![self.sum()?];
                    while args.len() < n && self.eat(b',') {
                        args.push(self.sum()?);
                    }
                    (args.len() == n && self.eat(b')')).then_some(Node::Call(f, args))
                }
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Formula;

    fn f(s: &str) -> Formula {
        Formula::parse(s).unwrap_or_else(|| panic!("{s}"))
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
        assert_eq!(f("2-1-0.5").map(0.0), 0.5);
        assert_eq!(f("-Input+1").map(0.25), 0.75);
        assert_eq!(f("Input^2").map(0.5), 0.25);
        assert_eq!(f("max(Input, 0.3)").map(0.1), 0.3);
        assert_eq!(f("min(Input*4,1)/2").map(0.5), 0.5);
        assert_eq!(f("Input/0").map(0.5), 0.0);
        for bad in ["", "1-", "Inp", "foo(Input)", "(Input", "Input)", "max(1)"] {
            assert!(Formula::parse(bad).is_none(), "{bad}");
        }
    }
}

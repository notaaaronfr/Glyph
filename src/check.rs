//! Static checks run before any code executes: names, arity, types and effects.
//!
//! The effect rule: a function may only perform the effects it declares with `uses`,
//! and calling a function counts as performing all of that function's effects.
//! So a signature is an honest, complete list of what the code can touch.

use crate::ast::*;
use crate::error::{Error, Pos};
use std::collections::{HashMap, HashSet};

pub const EFFECTS: [(&str, &str); 4] = [
    ("io", "print to / read from the terminal"),
    ("fs", "read and write files"),
    ("time", "read the clock"),
    ("rand", "generate random numbers"),
];

/// (min args, max args or None for variadic, effect performed)
pub fn builtin(name: &str) -> Option<(usize, Option<usize>, Option<&'static str>)> {
    Some(match name {
        "print" => (0, None, Some("io")),
        "input" => (0, Some(1), Some("io")),
        "read_file" => (1, Some(1), Some("fs")),
        "write_file" => (2, Some(2), Some("fs")),
        "now" => (0, Some(0), Some("time")),
        "random" => (1, Some(1), Some("rand")),
        "len" | "str" | "int" | "abs" => (1, Some(1), None),
        "push" | "join" | "split" | "contains" => (2, Some(2), None),
        "range" => (1, Some(2), None),
        _ => return None,
    })
}

/// The static type of every expression, keyed by node address. The program must
/// not move between `check` and the consumer (the native compiler).
pub struct Types(HashMap<*const Expr, Type>);

impl Types {
    pub fn of(&self, e: &Expr) -> &Type {
        self.0.get(&(e as *const Expr)).unwrap_or(&Type::Any)
    }
}

/// `Any` is compatible with everything; lists are compatible element-wise.
pub fn compat(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Any, _) | (_, Type::Any) => true,
        (Type::List(x), Type::List(y)) => compat(x, y),
        _ => a == b,
    }
}

fn is(t: &Type, want: &Type) -> bool {
    *t == Type::Any || t == want
}

pub fn check(p: &Program) -> Result<Types, Vec<Error>> {
    let mut c = Checker {
        sigs: HashMap::new(),
        errors: vec![],
        scopes: vec![],
        declared: HashSet::new(),
        ctx: String::new(),
        in_fn: false,
        ret: Type::Unit,
        types: HashMap::new(),
    };
    for f in &p.funcs {
        if builtin(&f.name).is_some() {
            c.errors.push(Error::new(
                "name",
                f.pos,
                format!(
                    "`{}` is a built-in function and cannot be redefined",
                    f.name
                ),
            ));
        } else if c.sigs.insert(&f.name, f).is_some() {
            c.errors.push(Error::new(
                "name",
                f.pos,
                format!("function `{}` is defined more than once", f.name),
            ));
        }
    }
    for f in &p.funcs {
        c.func(f);
    }
    for t in &p.tests {
        c.validate_effects(&t.effects);
        c.declared = t.effects.iter().map(|e| e.0.clone()).collect();
        c.ctx = format!("test \"{}\"", t.name);
        c.in_fn = false;
        c.ret = Type::Unit;
        c.scopes = vec![HashMap::new()];
        c.block(&t.body);
    }
    if c.errors.is_empty() {
        Ok(Types(c.types))
    } else {
        Err(c.errors)
    }
}

/// True when every path through the block ends in `return`.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::If(_, t, Some(e)) => always_returns(t) && always_returns(e),
        _ => false,
    })
}

struct Checker<'a> {
    sigs: HashMap<&'a str, &'a Func>,
    errors: Vec<Error>,
    scopes: Vec<HashMap<String, Type>>,
    declared: HashSet<String>,
    ctx: String,
    in_fn: bool,
    ret: Type,
    types: HashMap<*const Expr, Type>,
}

impl<'a> Checker<'a> {
    fn err(&mut self, kind: &'static str, pos: Pos, msg: String) {
        self.errors.push(Error::new(kind, pos, msg));
    }

    fn validate_effects(&mut self, effects: &[(String, Pos)]) {
        for (e, pos) in effects {
            if !EFFECTS.iter().any(|(n, _)| n == e) {
                let known: Vec<_> = EFFECTS.iter().map(|(n, _)| *n).collect();
                self.errors.push(
                    Error::new("effect", *pos, format!("unknown effect `{}`", e))
                        .hint(format!("known effects: {}", known.join(", "))),
                );
            }
        }
    }

    fn func(&mut self, f: &'a Func) {
        self.validate_effects(&f.effects);
        let mut params = HashMap::new();
        for p in &f.params {
            if params.insert(p.name.clone(), p.ty.clone()).is_some() {
                self.err(
                    "name",
                    f.pos,
                    format!("parameter `{}` appears twice in `{}`", p.name, f.name),
                );
            }
        }

        // Contracts must be pure: they are specifications, not behaviour.
        self.declared = HashSet::new();
        self.in_fn = true;
        self.ctx = format!("a contract of `{}`", f.name);
        self.scopes = vec![params.clone()];
        for e in &f.requires {
            self.cond(e);
        }
        let mut with_result = params.clone();
        with_result.insert("result".into(), f.ret.clone());
        self.scopes = vec![with_result];
        for e in &f.ensures {
            self.cond(e);
        }

        self.declared = f.effects.iter().map(|e| e.0.clone()).collect();
        self.ctx = format!("`{}`", f.name);
        self.ret = f.ret.clone();
        self.scopes = vec![params];
        self.block(&f.body);
        if f.ret != Type::Unit && f.ret != Type::Any && !always_returns(&f.body) {
            self.errors.push(
                Error::new(
                    "type",
                    f.pos,
                    format!("`{}` must return {} on every path", f.name, f.ret),
                )
                .hint("add a `return` at the end of the function"),
            );
        }
    }

    fn need_effect(&mut self, effect: &str, via: &str, pos: Pos) {
        if self.declared.contains(effect) {
            return;
        }
        let hint = if self.ctx.starts_with("a contract") {
            "contracts must be pure; they describe behaviour, they cannot perform it".to_string()
        } else {
            format!("add `uses {}` to the signature if this is intended", effect)
        };
        self.errors.push(
            Error::new(
                "effect",
                pos,
                format!(
                    "{} performs effect `{}` (by calling `{}`) but does not declare it",
                    self.ctx, effect, via
                ),
            )
            .hint(hint),
        );
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
    }

    fn block(&mut self, stmts: &'a [Stmt]) {
        self.scopes.push(HashMap::new());
        for s in stmts {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Let(n, ann, e) => {
                let t = match ann {
                    Some(a) => {
                        self.expect(e, a, &format!("the value of `{}`", n));
                        a.clone()
                    }
                    None => self.expr(e, None),
                };
                self.scopes.last_mut().unwrap().insert(n.clone(), t);
            }
            Stmt::Assign(n, e, pos) => match self.lookup(n) {
                Some(t) => {
                    self.expect(e, &t, &format!("the new value of `{}`", n));
                }
                None => {
                    self.expr(e, None);
                    self.errors.push(
                        Error::new(
                            "name",
                            *pos,
                            format!("assignment to undefined variable `{}`", n),
                        )
                        .hint(format!("declare it first with `let {} = ...`", n)),
                    );
                }
            },
            Stmt::Expr(e) => {
                self.expr(e, None);
            }
            Stmt::Assert(e) => self.cond(e),
            Stmt::If(c, t, els) => {
                self.cond(c);
                self.block(t);
                if let Some(b) = els {
                    self.block(b);
                }
            }
            Stmt::While(c, b) => {
                self.cond(c);
                self.block(b);
            }
            Stmt::For(n, e, b, pos) => {
                let elem = match self.expr(e, None) {
                    Type::List(x) => *x,
                    Type::Str => Type::Str,
                    Type::Any => Type::Any,
                    t => {
                        self.err("type", *pos, format!("cannot loop over {}", t));
                        Type::Any
                    }
                };
                self.scopes.push([(n.clone(), elem)].into());
                self.block(b);
                self.scopes.pop();
            }
            Stmt::Return(e, pos) => {
                if !self.in_fn {
                    self.err(
                        "syntax",
                        *pos,
                        "`return` is only allowed inside a function".into(),
                    );
                }
                let ret = self.ret.clone();
                match e {
                    Some(e) => {
                        self.expect(e, &ret, "the return value");
                    }
                    None if !is(&ret, &Type::Unit) => {
                        self.err(
                            "type",
                            *pos,
                            format!("`return` needs a value: this function returns {}", ret),
                        );
                    }
                    None => {}
                }
            }
        }
    }

    fn cond(&mut self, e: &'a Expr) {
        self.expect(e, &Type::Bool, "a condition");
    }

    /// Check `e` against an expected type and report a mismatch.
    fn expect(&mut self, e: &'a Expr, want: &Type, what: &str) -> Type {
        let t = self.expr(e, Some(want));
        if !compat(&t, want) {
            self.err(
                "type",
                e.pos,
                format!("{} must be {}, found {}", what, want, t),
            );
        }
        t
    }

    fn expr(&mut self, e: &'a Expr, expected: Option<&Type>) -> Type {
        let t = self.infer(e, expected);
        self.types.insert(e as *const Expr, t.clone());
        t
    }

    fn infer(&mut self, e: &'a Expr, expected: Option<&Type>) -> Type {
        use Type::*;
        match &e.kind {
            ExprKind::Int(_) => Int,
            ExprKind::Str(_) => Str,
            ExprKind::Bool(_) => Bool,
            ExprKind::Var(n) => self.lookup(n).unwrap_or_else(|| {
                self.err("name", e.pos, format!("undefined variable `{}`", n));
                Any
            }),
            ExprKind::List(items) => {
                let want = match expected {
                    Some(List(x)) => Some((**x).clone()),
                    Some(Any) => Some(Any),
                    _ => None,
                };
                let Some(first) = items.first() else {
                    return match want {
                        Some(t) => List(Box::new(t)),
                        None => {
                            self.errors.push(
                                Error::new("type", e.pos, "cannot tell what type of list `[]` is")
                                    .hint("annotate the variable, e.g. `let xs: List[Int] = []`"),
                            );
                            List(Box::new(Any))
                        }
                    };
                };
                let t0 = self.expr(first, want.as_ref());
                let elem = match want {
                    Some(w) if w != Any => w,
                    _ => t0,
                };
                for it in &items[1..] {
                    self.expect(it, &elem, "every list element");
                }
                List(Box::new(elem))
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                self.expect(x, &Int, "the operand of `-`");
                Int
            }
            ExprKind::Unary(UnOp::Not, x) => {
                self.expect(x, &Bool, "the operand of `not`");
                Bool
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), l, r) => {
                let what = format!("the operands of `{}`", op.symbol());
                self.expect(l, &Bool, &what);
                self.expect(r, &Bool, &what);
                Bool
            }
            ExprKind::Binary(op, l, r) => {
                let lt = self.expr(l, None);
                let rt = self.expr(r, Some(&lt));
                self.binary(*op, &lt, &rt, e)
            }
            ExprKind::Index(target, idx) => {
                let tt = self.expr(target, None);
                self.expect(idx, &Int, "an index");
                match tt {
                    List(x) => *x,
                    Str => Str,
                    Any => Any,
                    t => {
                        self.err("type", e.pos, format!("cannot index into {}", t));
                        Any
                    }
                }
            }
            ExprKind::Call(name, args) => self.call(name, args, e.pos),
        }
    }

    fn binary(&mut self, op: BinOp, lt: &Type, rt: &Type, e: &Expr) -> Type {
        use BinOp::*;
        use Type::*;
        let result = match op {
            Add => match (lt, rt) {
                (Any, t) | (t, Any) => Some(t.clone()),
                (Int, Int) => Some(Int),
                (Str, Str) => Some(Str),
                (List(a), List(b)) if compat(a, b) => {
                    Some(List(if **a == Any { b.clone() } else { a.clone() }))
                }
                _ => None,
            },
            Sub | Mul | Div | Mod => (is(lt, &Int) && is(rt, &Int)).then_some(Int),
            Eq | Ne => compat(lt, rt).then_some(Bool),
            Lt | Le | Gt | Ge => {
                ((is(lt, &Int) && is(rt, &Int)) || (is(lt, &Str) && is(rt, &Str))).then_some(Bool)
            }
            And | Or => Some(Bool),
        };
        result.unwrap_or_else(|| {
            let mut err = Error::new(
                "type",
                e.pos,
                format!("cannot apply `{}` to {} and {}", op.symbol(), lt, rt),
            );
            if op == Add && (*lt == Str || *rt == Str) {
                err = err.hint("convert with `str(...)` first, e.g. \"total: \" + str(n)");
            }
            self.errors.push(err);
            Any
        })
    }

    fn call(&mut self, name: &str, args: &'a [Expr], pos: Pos) -> Type {
        let n = args.len();
        if let Some((min, max, effect)) = builtin(name) {
            if let Some(eff) = effect {
                self.need_effect(eff, name, pos);
            }
            if n < min || max.is_some_and(|m| n > m) {
                let want = match max {
                    None => format!("at least {}", min),
                    Some(m) if m == min => min.to_string(),
                    Some(m) => format!("{} to {}", min, m),
                };
                self.err(
                    "arity",
                    pos,
                    format!("`{}` takes {} argument(s), got {}", name, want, n),
                );
                args.iter().for_each(|a| {
                    self.expr(a, None);
                });
                return Type::Any;
            }
            return self.builtin_call(name, args, pos);
        }
        let Some(f) = self.sigs.get(name).copied() else {
            self.err("name", pos, format!("unknown function `{}`", name));
            args.iter().for_each(|a| {
                self.expr(a, None);
            });
            return Type::Any;
        };
        for (eff, _) in &f.effects {
            self.need_effect(eff, name, pos);
        }
        if f.params.len() != n {
            self.err(
                "arity",
                pos,
                format!("`{}` takes {} argument(s), got {}", name, f.params.len(), n),
            );
            args.iter().for_each(|a| {
                self.expr(a, None);
            });
        } else {
            for (p, a) in f.params.iter().zip(args) {
                self.expect(a, &p.ty, &format!("argument `{}` of `{}`", p.name, name));
            }
        }
        f.ret.clone()
    }

    /// Argument counts are already validated.
    fn builtin_call(&mut self, name: &str, args: &'a [Expr], pos: Pos) -> Type {
        use Type::*;
        let arg = |i: usize| format!("argument {} of `{}`", i + 1, name);
        match name {
            "print" => {
                args.iter().for_each(|a| {
                    self.expr(a, None);
                });
                Unit
            }
            "input" => {
                if let Some(a) = args.first() {
                    self.expect(a, &Str, "the prompt of `input`");
                }
                Str
            }
            "len" => {
                let t = self.expr(&args[0], None);
                if !matches!(t, List(_) | Str | Any) {
                    self.err(
                        "type",
                        pos,
                        format!("`len` needs a list or a string, found {}", t),
                    );
                }
                Int
            }
            "str" => {
                self.expr(&args[0], None);
                Str
            }
            "int" => {
                let t = self.expr(&args[0], None);
                if !matches!(t, Int | Str | Any) {
                    self.err(
                        "type",
                        pos,
                        format!("`int` needs a string or an integer, found {}", t),
                    );
                }
                Int
            }
            "abs" | "random" => {
                self.expect(&args[0], &Int, &arg(0));
                Int
            }
            "push" | "contains" => {
                let elem = match self.expr(&args[0], None) {
                    List(x) => *x,
                    Str if name == "contains" => Str,
                    Any => Any,
                    t => {
                        self.err(
                            "type",
                            pos,
                            format!("`{}` needs a list first, found {}", name, t),
                        );
                        Any
                    }
                };
                self.expect(&args[1], &elem, &arg(1));
                if name == "push" {
                    List(Box::new(elem))
                } else {
                    Bool
                }
            }
            "range" => {
                for (i, a) in args.iter().enumerate() {
                    self.expect(a, &Int, &arg(i));
                }
                List(Box::new(Int))
            }
            "join" => {
                let t = self.expr(&args[0], None);
                if !matches!(t, List(_) | Any) {
                    self.err(
                        "type",
                        pos,
                        format!("`join` needs a list first, found {}", t),
                    );
                }
                self.expect(&args[1], &Str, &arg(1));
                Str
            }
            "split" => {
                self.expect(&args[0], &Str, &arg(0));
                self.expect(&args[1], &Str, &arg(1));
                List(Box::new(Str))
            }
            "read_file" => {
                self.expect(&args[0], &Str, &arg(0));
                Str
            }
            "write_file" => {
                self.expect(&args[0], &Str, &arg(0));
                self.expect(&args[1], &Str, &arg(1));
                Unit
            }
            "now" => Int,
            _ => unreachable!("builtin `{}` has no type rule", name),
        }
    }
}

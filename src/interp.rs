//! Tree-walking interpreter. Checks types at function boundaries and
//! evaluates `requires` / `ensures` on every call.

use crate::ast::*;
use crate::error::{Error, Pos};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::rc::Rc;

/// A list with value semantics, built the same way as the native runtime's lists.
/// Several lists may share one buffer: each sees only its first `len` items, and
/// items are never overwritten, so the list ending at the buffer's tip can be
/// extended in place. That makes `xs = push(xs, v)` O(1) instead of a full copy.
#[derive(Debug, Clone)]
pub struct ListVal {
    buf: Rc<RefCell<Vec<Value>>>,
    len: usize,
}

impl ListVal {
    pub fn new(items: Vec<Value>) -> Self {
        let len = items.len();
        ListVal {
            buf: Rc::new(RefCell::new(items)),
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, i: usize) -> Option<Value> {
        if i < self.len {
            Some(self.buf.borrow()[i].clone())
        } else {
            None
        }
    }

    pub fn items(&self) -> Vec<Value> {
        self.buf.borrow()[..self.len].to_vec()
    }

    pub fn contains(&self, v: &Value) -> bool {
        self.buf.borrow()[..self.len].contains(v)
    }

    pub fn push(&self, v: Value) -> ListVal {
        let mut buf = self.buf.borrow_mut();
        if buf.len() == self.len {
            buf.push(v);
            return ListVal {
                buf: self.buf.clone(),
                len: self.len + 1,
            };
        }
        let mut items = buf[..self.len].to_vec();
        items.push(v);
        ListVal::new(items)
    }
}

impl PartialEq for ListVal {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.buf.borrow()[..self.len] == other.buf.borrow()[..other.len]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Str(String),
    Bool(bool),
    List(ListVal),
    Record(Rc<RecordVal>),
    Enum(Rc<EnumVal>),
    Unit,
}

/// An enum value: which variant it is, and that variant's fields in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVal {
    pub enum_name: String,
    pub variant: String,
    pub fields: Vec<(String, Value)>,
}

/// A record value. Fields are stored in declaration order; records are immutable,
/// so `p.x = v` builds an updated copy.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordVal {
    pub name: String,
    pub fields: Vec<(String, Value)>,
}

impl Value {
    /// Like Display, but strings are quoted. Used inside lists and in diagnostics.
    pub fn repr(&self) -> String {
        match self {
            Value::Str(s) => format!("{:?}", s),
            v => v.to_string(),
        }
    }

    fn type_name(&self) -> String {
        match self {
            Value::Int(_) => "Int".into(),
            Value::Str(_) => "Str".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Unit => "Unit".into(),
            Value::List(_) => "List".into(),
            Value::Record(r) => r.name.clone(),
            Value::Enum(e) => e.enum_name.clone(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Unit => write!(f, "()"),
            Value::List(xs) => {
                let items: Vec<_> = xs.items().iter().map(|x| x.repr()).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Value::Enum(e) if e.fields.is_empty() => write!(f, "{}", e.variant),
            Value::Enum(e) => {
                let fields: Vec<_> = e
                    .fields
                    .iter()
                    .map(|(n, v)| format!("{}: {}", n, v.repr()))
                    .collect();
                write!(f, "{} {{ {} }}", e.variant, fields.join(", "))
            }
            Value::Record(r) if r.fields.is_empty() => write!(f, "{} {{}}", r.name),
            Value::Record(r) => {
                let fields: Vec<_> = r
                    .fields
                    .iter()
                    .map(|(n, v)| format!("{}: {}", n, v.repr()))
                    .collect();
                write!(f, "{} {{ {} }}", r.name, fields.join(", "))
            }
        }
    }
}

fn has_type(v: &Value, t: &Type) -> bool {
    match (t, v) {
        (Type::Any, _) => true,
        (Type::Int, Value::Int(_))
        | (Type::Str, Value::Str(_))
        | (Type::Bool, Value::Bool(_))
        | (Type::Unit, Value::Unit) => true,
        // Element types are guaranteed by the static checker; re-checking them here
        // would make every call that passes a list O(n).
        (Type::List(_), Value::List(_)) => true,
        (Type::Record(n, _), Value::Record(r)) => *n == r.name,
        (Type::Enum(n, _), Value::Enum(e)) => *n == e.enum_name,
        // Generic code is checked statically; any value can stand for `T` at runtime.
        (Type::Param(_), _) => true,
        _ => false,
    }
}

enum Flow {
    Next,
    Ret(Value),
}

type R<T> = Result<T, Error>;

const MAX_DEPTH: usize = 10_000;

pub struct Interp<'a> {
    funcs: HashMap<&'a str, &'a Func>,
    records: HashMap<&'a str, &'a TypeDecl>,
    prog: &'a Program,
    scopes: Vec<HashMap<String, Value>>,
    depth: usize,
    rng: u64,
}

impl<'a> Interp<'a> {
    pub fn new(p: &'a Program) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Interp {
            funcs: p.funcs.iter().map(|f| (f.name.as_str(), f)).collect(),
            records: p.types.iter().map(|t| (t.name.as_str(), t)).collect(),
            prog: p,
            scopes: vec![HashMap::new()],
            depth: 0,
            rng: seed | 1,
        }
    }

    pub fn run_test(&mut self, t: &'a Test) -> R<()> {
        self.scopes = vec![HashMap::new()];
        self.block(&t.body).map(|_| ())
    }

    pub fn call(&mut self, name: &str, args: Vec<Value>, pos: Pos) -> R<Value> {
        match self.funcs.get(name).copied() {
            Some(f) => self.call_user(f, args, pos),
            None => self.builtin(name, args, pos),
        }
    }

    fn call_user(&mut self, f: &'a Func, args: Vec<Value>, pos: Pos) -> R<Value> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::new(
                "runtime",
                pos,
                format!("recursion deeper than {} calls in `{}`", MAX_DEPTH, f.name),
            )
            .hint("check that the recursion has a base case that is always reached"));
        }
        for (p, a) in f.params.iter().zip(&args) {
            if !has_type(a, &p.ty) {
                return Err(Error::new(
                    "type",
                    pos,
                    format!(
                        "`{}` expects `{}: {}` but got {} {}",
                        f.name,
                        p.name,
                        p.ty,
                        a.type_name(),
                        a.repr()
                    ),
                ));
            }
        }
        // Only needed for error messages, so only formatted on failure (formatting a
        // large argument on every call would make recursion over big values quadratic).
        let arg_values = args.clone();
        let inputs = move || {
            let parts: Vec<String> = f
                .params
                .iter()
                .zip(&arg_values)
                .map(|(p, a)| format!("{} = {}", p.name, a.repr()))
                .collect();
            if parts.is_empty() {
                "no arguments".to_string()
            } else {
                parts.join(", ")
            }
        };
        let frame: HashMap<String, Value> =
            f.params.iter().map(|p| p.name.clone()).zip(args).collect();

        // `ensures` describes the inputs the function was called with, even if the body
        // reassigns its parameters, so keep the originals.
        let originals = (!f.ensures.is_empty()).then(|| frame.clone());
        let saved = std::mem::replace(&mut self.scopes, vec![frame]);
        self.depth += 1;
        let res = self.run_fn(f, pos, &inputs, originals);
        self.depth -= 1;
        self.scopes = saved;
        res
    }

    fn run_fn(
        &mut self,
        f: &'a Func,
        call_pos: Pos,
        inputs: &dyn Fn() -> String,
        originals: Option<HashMap<String, Value>>,
    ) -> R<Value> {
        // A failed precondition is the caller's fault, so it is reported at the call site.
        for c in &f.requires {
            if !self.cond(c)? {
                return Err(Error::new(
                    "contract",
                    call_pos,
                    format!("precondition of `{}` violated: `{}`", f.name, c.text),
                )
                .hint(format!("called with {}", inputs())));
            }
        }
        let v = match self.block(&f.body)? {
            Flow::Ret(v) => v,
            Flow::Next => Value::Unit,
        };
        if !has_type(&v, &f.ret) {
            return Err(Error::new(
                "type",
                f.pos,
                format!(
                    "`{}` should return {} but returned {} {}",
                    f.name,
                    f.ret,
                    v.type_name(),
                    v.repr()
                ),
            )
            .hint(format!("called with {}", inputs())));
        }
        // A failed postcondition is the function's own fault, so it is reported at the clause.
        if let Some(mut frame) = originals {
            frame.insert("result".into(), v.clone());
            self.scopes = vec![frame];
            for c in &f.ensures {
                if !self.cond(c)? {
                    return Err(Error::new(
                        "contract",
                        c.pos,
                        format!("postcondition of `{}` violated: `{}`", f.name, c.text),
                    )
                    .hint(format!(
                        "called with {}, returned {}",
                        inputs(),
                        v.repr()
                    )));
                }
            }
        }
        Ok(v)
    }

    fn cond(&mut self, e: &Expr) -> R<bool> {
        match self.eval(e)? {
            Value::Bool(b) => Ok(b),
            v => Err(Error::new(
                "type",
                e.pos,
                format!("condition must be Bool, got {} {}", v.type_name(), v.repr()),
            )),
        }
    }

    fn block(&mut self, stmts: &'a [Stmt]) -> R<Flow> {
        self.scopes.push(HashMap::new());
        let mut out = Ok(Flow::Next);
        for s in stmts {
            match self.stmt(s) {
                Ok(Flow::Next) => {}
                other => {
                    out = other;
                    break;
                }
            }
        }
        self.scopes.pop();
        out
    }

    fn stmt(&mut self, s: &'a Stmt) -> R<Flow> {
        match s {
            Stmt::Let(n, _, e) => {
                let v = self.eval(e)?;
                self.scopes.last_mut().unwrap().insert(n.clone(), v);
            }
            Stmt::SetField(n, path, e, pos) => {
                let v = self.eval(e)?;
                let cur = self.eval(&Expr {
                    kind: ExprKind::Var(n.clone()),
                    pos: *pos,
                    text: n.clone(),
                })?;
                let updated = set_path(cur, path, v)?;
                if let Some(slot) = self.scopes.iter_mut().rev().find_map(|s| s.get_mut(n)) {
                    *slot = updated;
                }
            }
            Stmt::Match(scrutinee, arms, pos) => {
                let Value::Enum(v) = self.eval(scrutinee)? else {
                    return Err(Error::new(
                        "type",
                        scrutinee.pos,
                        "`match` needs an enum value",
                    ));
                };
                let arm = arms.iter().find(|a| match &a.pattern {
                    Pattern::Wildcard(_) => true,
                    Pattern::Variant(name, _, _) => *name == v.variant,
                });
                let Some(arm) = arm else {
                    return Err(Error::new(
                        "runtime",
                        *pos,
                        format!("no `match` arm for `{}`", v.variant),
                    ));
                };
                let mut scope = HashMap::new();
                if let Pattern::Variant(_, binds, _) = &arm.pattern {
                    for (f, name, _) in binds {
                        if let Some((_, val)) = v.fields.iter().find(|(n, _)| n == f) {
                            scope.insert(name.clone(), val.clone());
                        }
                    }
                }
                self.scopes.push(scope);
                let r = self.block(&arm.body);
                self.scopes.pop();
                return r;
            }
            Stmt::Assign(n, e, pos) => {
                let v = self.eval(e)?;
                match self.scopes.iter_mut().rev().find_map(|s| s.get_mut(n)) {
                    Some(slot) => *slot = v,
                    None => {
                        return Err(Error::new(
                            "name",
                            *pos,
                            format!("undefined variable `{}`", n),
                        ))
                    }
                }
            }
            Stmt::Expr(e) => {
                self.eval(e)?;
            }
            Stmt::If(c, then, els) => {
                if self.cond(c)? {
                    return self.block(then);
                } else if let Some(b) = els {
                    return self.block(b);
                }
            }
            Stmt::While(c, body) => {
                while self.cond(c)? {
                    if let Flow::Ret(v) = self.block(body)? {
                        return Ok(Flow::Ret(v));
                    }
                }
            }
            Stmt::For(n, e, body, pos) => {
                let items = match self.eval(e)? {
                    Value::List(xs) => xs.items(),
                    Value::Str(s) => s.chars().map(|c| Value::Str(c.to_string())).collect(),
                    v => {
                        return Err(Error::new(
                            "type",
                            *pos,
                            format!("cannot loop over {} {}", v.type_name(), v.repr()),
                        ))
                    }
                };
                for item in items {
                    self.scopes.push([(n.clone(), item)].into());
                    let r = self.block(body);
                    self.scopes.pop();
                    if let Flow::Ret(v) = r? {
                        return Ok(Flow::Ret(v));
                    }
                }
            }
            Stmt::Return(e, _) => {
                let v = match e {
                    Some(e) => self.eval(e)?,
                    None => Value::Unit,
                };
                return Ok(Flow::Ret(v));
            }
            Stmt::Assert(e) => {
                if !self.cond(e)? {
                    let mut err = Error::new(
                        "assertion",
                        e.pos,
                        format!("assertion failed: `{}`", e.text),
                    );
                    if let ExprKind::Binary(op, l, r) = &e.kind {
                        if matches!(op, BinOp::And | BinOp::Or) {
                            return Err(err);
                        }
                        let (l, r) = (self.eval(l)?, self.eval(r)?);
                        err = err.hint(format!(
                            "left side = {}, right side = {}",
                            l.repr(),
                            r.repr()
                        ));
                    }
                    return Err(err);
                }
            }
        }
        Ok(Flow::Next)
    }

    fn eval(&mut self, e: &Expr) -> R<Value> {
        use Value::*;
        Ok(match &e.kind {
            ExprKind::Int(n) => Int(*n),
            ExprKind::Str(s) => Str(s.clone()),
            ExprKind::Bool(b) => Bool(*b),
            ExprKind::Var(n) => match self.scopes.iter().rev().find_map(|s| s.get(n)) {
                Some(v) => v.clone(),
                None => {
                    return Err(Error::new(
                        "name",
                        e.pos,
                        format!("undefined variable `{}`", n),
                    ))
                }
            },
            ExprKind::List(xs) => List(ListVal::new(
                xs.iter().map(|x| self.eval(x)).collect::<R<_>>()?,
            )),
            ExprKind::Unary(op, x) => match (op, self.eval(x)?) {
                (UnOp::Neg, Int(n)) => Int(n.checked_neg().ok_or_else(|| overflow(e))?),
                (UnOp::Not, Bool(b)) => Bool(!b),
                (_, v) => {
                    return Err(Error::new(
                        "type",
                        e.pos,
                        format!(
                            "cannot apply `{}` to {} {}",
                            e.text,
                            v.type_name(),
                            v.repr()
                        ),
                    ))
                }
            },
            ExprKind::Binary(BinOp::And, l, r) => Bool(self.cond(l)? && self.cond(r)?),
            ExprKind::Binary(BinOp::Or, l, r) => Bool(self.cond(l)? || self.cond(r)?),
            ExprKind::Binary(op, l, r) => {
                let (a, b) = (self.eval(l)?, self.eval(r)?);
                binary(*op, a, b, e)?
            }
            ExprKind::Call(name, args) => {
                let vals = args.iter().map(|a| self.eval(a)).collect::<R<Vec<_>>>()?;
                self.call(name, vals, e.pos)?
            }
            ExprKind::Record(name, fields) => {
                // Evaluate in source order (effects happen in the order written),
                // then store in declaration order.
                let mut vals = HashMap::new();
                for (f, fe, _) in fields {
                    vals.insert(f.as_str(), self.eval(fe)?);
                }
                let decl = self.records[name.as_str()];
                let fields = decl
                    .fields
                    .iter()
                    .map(|(f, _, _)| (f.clone(), vals.remove(f.as_str()).unwrap_or(Unit)))
                    .collect();
                Record(Rc::new(RecordVal {
                    name: name.clone(),
                    fields,
                }))
            }
            ExprKind::Variant(name, fields) => {
                let mut vals = HashMap::new();
                for (f, fe, _) in fields {
                    vals.insert(f.as_str(), self.eval(fe)?);
                }
                let (decl, i) = self.prog.variant(name).expect("checked");
                let fields = decl.variants[i]
                    .fields
                    .iter()
                    .map(|(f, _, _)| (f.clone(), vals.remove(f.as_str()).unwrap_or(Unit)))
                    .collect();
                Enum(Rc::new(EnumVal {
                    enum_name: decl.name.clone(),
                    variant: name.clone(),
                    fields,
                }))
            }
            ExprKind::Field(target, f) => match self.eval(target)? {
                Record(r) => match r.fields.iter().find(|(n, _)| n == f) {
                    Some((_, v)) => v.clone(),
                    None => {
                        return Err(Error::new(
                            "type",
                            e.pos,
                            format!("`{}` has no field `{}`", r.name, f),
                        ))
                    }
                },
                v => {
                    return Err(Error::new(
                        "type",
                        e.pos,
                        format!("{} has no fields", v.type_name()),
                    ))
                }
            },
            ExprKind::Index(target, idx) => {
                let (t, i) = (self.eval(target)?, self.eval(idx)?);
                let oob = |len: usize, i: i64| {
                    Error::new(
                        "runtime",
                        e.pos,
                        format!("index {} out of bounds (length {})", i, len),
                    )
                };
                match (t, i) {
                    (List(xs), Int(i)) => {
                        let len = xs.len();
                        usize::try_from(i)
                            .ok()
                            .and_then(|u| xs.get(u))
                            .ok_or_else(|| oob(len, i))?
                    }
                    (Str(s), Int(i)) => {
                        let len = s.chars().count();
                        let c = usize::try_from(i)
                            .ok()
                            .and_then(|u| s.chars().nth(u))
                            .ok_or_else(|| oob(len, i))?;
                        Str(c.to_string())
                    }
                    (t, i) => {
                        return Err(Error::new(
                            "type",
                            e.pos,
                            format!("cannot index {} with {}", t.type_name(), i.type_name()),
                        ))
                    }
                }
            }
        })
    }

    fn builtin(&mut self, name: &str, args: Vec<Value>, pos: Pos) -> R<Value> {
        use Value::*;
        let rt = |msg: String| Error::new("runtime", pos, msg);
        Ok(match (name, args.as_slice()) {
            ("print", _) => {
                let parts: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                println!("{}", parts.join(" "));
                Unit
            }
            ("input", _) => {
                if let [Str(prompt)] = args.as_slice() {
                    print!("{}", prompt);
                    let _ = std::io::stdout().flush();
                }
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| rt(e.to_string()))?;
                Str(line.trim_end_matches(['\n', '\r']).to_string())
            }
            ("len", [List(xs)]) => Int(xs.len() as i64),
            ("len", [Str(s)]) => Int(s.chars().count() as i64),
            ("str", [v]) => Str(v.to_string()),
            ("int", [Int(n)]) => Int(*n),
            ("int", [Str(s)]) => Int(s
                .trim()
                .parse()
                .map_err(|_| rt(format!("cannot convert {:?} to Int", s)))?),
            ("parse_int", [Str(s)]) => {
                let (variant, fields) = match s.trim().parse::<i64>() {
                    Ok(n) => ("Some", vec![("value".to_string(), Int(n))]),
                    Err(_) => ("None", vec![]),
                };
                Enum(Rc::new(EnumVal {
                    enum_name: "Option".into(),
                    variant: variant.into(),
                    fields,
                }))
            }
            ("abs", [Int(n)]) => Int(n
                .checked_abs()
                .ok_or_else(|| rt("integer overflow in `abs`".into()))?),
            ("push", [List(xs), v]) => List(xs.push(v.clone())),
            ("range", [Int(n)]) => List(ListVal::new((0..*n).map(Int).collect())),
            ("range", [Int(a), Int(b)]) => List(ListVal::new((*a..*b).map(Int).collect())),
            ("join", [List(xs), Str(sep)]) => Str(xs
                .items()
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(sep)),
            ("split", [Str(_), Str(sep)]) if sep.is_empty() => {
                return Err(rt(
                    "invalid arguments to `split`: the separator must not be empty".into(),
                ))
            }
            ("split", [Str(s), Str(sep)]) => List(ListVal::new(
                s.split(sep.as_str()).map(|p| Str(p.into())).collect(),
            )),
            ("contains", [List(xs), v]) => Bool(xs.contains(v)),
            ("contains", [Str(s), Str(sub)]) => Bool(s.contains(sub.as_str())),
            ("read_file", [Str(p)]) => Str(std::fs::read_to_string(p)
                .map_err(|e| rt(format!("cannot read {:?}: {}", p, e)))?),
            ("write_file", [Str(p), Str(c)]) => {
                std::fs::write(p, c).map_err(|e| rt(format!("cannot write {:?}: {}", p, e)))?;
                Unit
            }
            ("now", []) => Int(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)),
            ("sleep", [Int(ms)]) if *ms < 0 => {
                return Err(rt(
                    "invalid arguments to `sleep`: the time must not be negative".into(),
                ))
            }
            ("sleep", [Int(ms)]) => {
                let _ = std::io::stdout().flush();
                std::thread::sleep(std::time::Duration::from_millis(*ms as u64));
                Unit
            }
            ("random", [Int(n)]) if *n <= 0 => {
                return Err(rt(
                    "invalid arguments to `random`: the limit must be positive".into(),
                ))
            }
            ("random", [Int(n)]) => {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                Int((self.rng % (*n as u64)) as i64)
            }
            _ => {
                let got: Vec<String> = args
                    .iter()
                    .map(|a| format!("{} {}", a.type_name(), a.repr()))
                    .collect();
                return Err(Error::new(
                    "type",
                    pos,
                    format!("invalid arguments to `{}`: ({})", name, got.join(", ")),
                ));
            }
        })
    }
}

fn overflow(e: &Expr) -> Error {
    Error::new(
        "runtime",
        e.pos,
        format!("integer overflow in `{}`", e.text),
    )
}

/// A copy of `v` with the field at `path` replaced by `new`.
fn set_path(v: Value, path: &[(String, Pos)], new: Value) -> R<Value> {
    let Some(((f, pos), rest)) = path.split_first() else {
        return Ok(new);
    };
    match v {
        Value::Record(r) => {
            let mut r = (*r).clone();
            let Some(slot) = r.fields.iter_mut().find(|(n, _)| n == f) else {
                return Err(Error::new(
                    "type",
                    *pos,
                    format!("`{}` has no field `{}`", r.name, f),
                ));
            };
            slot.1 = set_path(std::mem::replace(&mut slot.1, Value::Unit), rest, new)?;
            Ok(Value::Record(Rc::new(r)))
        }
        v => Err(Error::new(
            "type",
            *pos,
            format!("{} has no fields", v.type_name()),
        )),
    }
}

fn binary(op: BinOp, a: Value, b: Value, e: &Expr) -> R<Value> {
    use BinOp::*;
    use Value::*;
    let checked = |r: Option<i64>| r.map(Int).ok_or_else(|| overflow(e));
    Ok(match (op, a, b) {
        (Add, Int(x), Int(y)) => checked(x.checked_add(y))?,
        (Sub, Int(x), Int(y)) => checked(x.checked_sub(y))?,
        (Mul, Int(x), Int(y)) => checked(x.checked_mul(y))?,
        (Div | Mod, Int(_), Int(0)) => {
            return Err(Error::new(
                "runtime",
                e.pos,
                format!("division by zero in `{}`", e.text),
            )
            .hint("guard it with `requires` on the divisor, e.g. `requires b != 0`"))
        }
        (Div, Int(x), Int(y)) => checked(x.checked_div(y))?,
        (Mod, Int(x), Int(y)) => checked(x.checked_rem(y))?,
        (Add, Str(x), Str(y)) => Str(x + &y),
        (Add, List(x), List(y)) => List(ListVal::new([x.items(), y.items()].concat())),
        (Eq, x, y) => Bool(x == y),
        (Ne, x, y) => Bool(x != y),
        (Lt, Int(x), Int(y)) => Bool(x < y),
        (Le, Int(x), Int(y)) => Bool(x <= y),
        (Gt, Int(x), Int(y)) => Bool(x > y),
        (Ge, Int(x), Int(y)) => Bool(x >= y),
        (Lt, Str(x), Str(y)) => Bool(x < y),
        (Le, Str(x), Str(y)) => Bool(x <= y),
        (Gt, Str(x), Str(y)) => Bool(x > y),
        (Ge, Str(x), Str(y)) => Bool(x >= y),
        (_, x, y) => {
            let mut err = Error::new(
                "type",
                e.pos,
                format!(
                    "cannot apply `{}` to {} {} and {} {}",
                    op.symbol(),
                    x.type_name(),
                    x.repr(),
                    y.type_name(),
                    y.repr()
                ),
            );
            if op == Add && (matches!(x, Str(_)) || matches!(y, Str(_))) {
                err = err.hint("convert with `str(...)` first, e.g. \"total: \" + str(n)");
            }
            return Err(err);
        }
    })
}

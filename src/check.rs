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
        "len" | "str" | "int" | "abs" | "parse_int" => (1, Some(1), None),
        "push" | "join" | "split" | "contains" => (2, Some(2), None),
        "range" => (1, Some(2), None),
        _ => return None,
    })
}

/// The static type of every expression, keyed by node address. The program must
/// not move between `check` and the consumer (the native compiler).
pub struct Types {
    types: HashMap<*const Expr, Type>,
    /// The type arguments chosen at each call of a generic function.
    targs: HashMap<*const Expr, Vec<Type>>,
}

impl Types {
    pub fn of(&self, e: &Expr) -> &Type {
        self.types.get(&(e as *const Expr)).unwrap_or(&Type::Any)
    }

    pub fn targs(&self, e: &Expr) -> &[Type] {
        self.targs
            .get(&(e as *const Expr))
            .map_or(&[], |v| v.as_slice())
    }
}

/// `Any` is compatible with everything; lists and generic types are compatible
/// argument by argument.
pub fn compat(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Any, _) | (_, Type::Any) => true,
        (Type::List(x), Type::List(y)) => compat(x, y),
        (Type::Record(n, xs), Type::Record(m, ys)) | (Type::Enum(n, xs), Type::Enum(m, ys)) => {
            n == m && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| compat(x, y))
        }
        _ => a == b,
    }
}

fn has_any(t: &Type) -> bool {
    match t {
        Type::Any => true,
        Type::List(x) => has_any(x),
        Type::Record(_, xs) | Type::Enum(_, xs) => xs.iter().any(has_any),
        _ => false,
    }
}

/// Matches `pat`, which may mention the type parameters `params`, against `actual`,
/// recording what each parameter must be. Returns false on a mismatch.
fn bind(pat: &Type, actual: &Type, params: &[String], b: &mut HashMap<String, Type>) -> bool {
    match (pat, actual) {
        (_, Type::Any) | (Type::Any, _) => true,
        (Type::Param(p), _) if params.contains(p) => match b.get(p) {
            Some(known) if !has_any(known) || has_any(actual) => compat(known, actual),
            Some(known) if !compat(known, actual) => false,
            // Unknown, or known only partly (like `List[Any]` from `[]`): learn more.
            _ => {
                b.insert(p.clone(), actual.clone());
                true
            }
        },
        (Type::List(x), Type::List(y)) => bind(x, y, params, b),
        (Type::Record(n, xs), Type::Record(m, ys)) | (Type::Enum(n, xs), Type::Enum(m, ys)) => {
            n == m && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| bind(x, y, params, b))
        }
        _ => pat == actual,
    }
}

/// `t` with what is known substituted, and still-unknown `params` replaced by `Any`.
fn hint(t: &Type, b: &HashMap<String, Type>, params: &[String]) -> Type {
    let unknown: HashMap<String, Type> = params
        .iter()
        .filter(|p| !b.contains_key(*p))
        .map(|p| (p.clone(), Type::Any))
        .collect();
    t.subst(b).subst(&unknown)
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
        targs: HashMap::new(),
        records: HashMap::new(),
        prog: p,
    };
    // Every user-defined name (record, enum, variant) shares one namespace.
    let mut names: HashMap<&str, Pos> = HashMap::new();
    let all_names = p
        .types
        .iter()
        .map(|t| (&t.name, t.pos))
        .chain(p.enums.iter().map(|e| (&e.name, e.pos)))
        .chain(
            p.enums
                .iter()
                .flat_map(|e| e.variants.iter().map(|v| (&v.name, v.pos))),
        );
    for (n, pos) in all_names {
        if let Some(first) = names.insert(n, pos) {
            let mut e = Error::new(
                "name",
                pos,
                format!("the name `{}` is defined more than once", n),
            );
            if first.line == 0 {
                e = e.hint(format!(
                    "`{}` is built in (from `Option` or `Result`); choose another name",
                    n
                ));
            }
            c.errors.push(e);
        }
    }
    for v in p.enums.iter().flat_map(|e| &e.variants) {
        let mut seen = HashSet::new();
        for (f, _, pos) in &v.fields {
            if !seen.insert(f) {
                c.err(
                    "name",
                    *pos,
                    format!("field `{}` appears twice in `{}`", f, v.name),
                );
            }
        }
    }
    for t in &p.types {
        c.records.insert(&t.name, t);
        let mut seen = HashSet::new();
        for (f, _, pos) in &t.fields {
            if !seen.insert(f) {
                c.err(
                    "name",
                    *pos,
                    format!("field `{}` appears twice in `{}`", f, t.name),
                );
            }
        }
    }
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
    let decl_fields = p.types.iter().flat_map(|t| &t.fields).chain(
        p.enums
            .iter()
            .flat_map(|e| &e.variants)
            .flat_map(|v| &v.fields),
    );
    for (_, ft, pos) in decl_fields {
        c.check_type(ft, *pos);
    }
    for f in &p.funcs {
        for t in f.params.iter().map(|p| &p.ty).chain([&f.ret]) {
            c.check_type(t, f.pos);
        }
        if f.name == "main" && !f.tparams.is_empty() {
            c.err("type", f.pos, "`main` can't have type parameters".into());
        }
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
        Ok(Types {
            types: c.types,
            targs: c.targs,
        })
    } else {
        Err(c.errors)
    }
}

/// True when every path through the block ends in `return`.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::If(_, t, Some(e)) => always_returns(t) && always_returns(e),
        // The checker guarantees every `match` is exhaustive.
        Stmt::Match(_, arms, _) => !arms.is_empty() && arms.iter().all(|a| always_returns(&a.body)),
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
    targs: HashMap<*const Expr, Vec<Type>>,
    records: HashMap<&'a str, &'a TypeDecl>,
    prog: &'a Program,
}

impl<'a> Checker<'a> {
    fn err(&mut self, kind: &'static str, pos: Pos, msg: String) {
        self.errors.push(Error::new(kind, pos, msg));
    }

    /// The type parameters of the record or enum called `name`, if it exists.
    fn decl_params(&self, name: &str) -> Option<&'a [String]> {
        let prog = self.prog;
        prog.types
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.params.as_slice())
            .or_else(|| {
                prog.enums
                    .iter()
                    .find(|e| e.name == name)
                    .map(|e| e.params.as_slice())
            })
    }

    /// Reports generic types written with the wrong number of type arguments.
    /// Returns false if there was a problem.
    fn check_type(&mut self, t: &Type, pos: Pos) -> bool {
        let before = self.errors.len();
        self.check_type_inner(t, pos);
        self.errors.len() == before
    }

    fn check_type_inner(&mut self, t: &Type, pos: Pos) {
        match t {
            Type::List(x) => self.check_type_inner(x, pos),
            Type::Record(n, args) | Type::Enum(n, args) => {
                let params = self.decl_params(n).unwrap_or(&[]);
                if args.len() != params.len() {
                    let msg = if params.is_empty() {
                        format!("`{}` takes no type arguments", n)
                    } else {
                        format!(
                            "`{}` needs {} type argument(s), like `{}[{}]`",
                            n,
                            params.len(),
                            n,
                            params.join(", ")
                        )
                    };
                    self.err("type", pos, msg);
                }
                for a in args {
                    self.check_type_inner(a, pos);
                }
            }
            _ => {}
        }
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
                    Some(a) if self.check_type(a, e.pos) => {
                        self.expect(e, a, &format!("the value of `{}`", n));
                        a.clone()
                    }
                    // A malformed annotation was already reported; don't pile on.
                    Some(_) => {
                        self.expr(e, Some(&Type::Any));
                        Type::Any
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
            Stmt::SetField(n, path, e, pos) => {
                let Some(mut t) = self.lookup(n) else {
                    self.expr(e, None);
                    self.err(
                        "name",
                        *pos,
                        format!("assignment to undefined variable `{}`", n),
                    );
                    return;
                };
                let mut what = n.clone();
                for (f, fpos) in path {
                    t = self.field_type(&t, f, *fpos);
                    what = format!("{}.{}", what, f);
                }
                self.expect(e, &t, &format!("the new value of `{}`", what));
            }
            Stmt::Expr(e) => {
                self.expr(e, None);
            }
            Stmt::Assert(e) => self.cond(e),
            Stmt::Match(e, arms, pos) => self.match_stmt(e, arms, *pos),
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
            ExprKind::Call(name, args) => self.call(name, args, e, expected),
            ExprKind::Field(target, f) => {
                let t = self.expr(target, None);
                self.field_type(&t, f, e.pos)
            }
            ExprKind::Record(name, fields) => {
                let Some(decl) = self.records.get(name.as_str()).copied() else {
                    return Any;
                };
                let args = self.construct(
                    name,
                    name,
                    &decl.params,
                    &decl.fields,
                    fields,
                    expected,
                    e.pos,
                );
                Record(name.clone(), args)
            }
            ExprKind::Variant(name, fields) => {
                let Some((decl, i)) = self.prog.variant(name) else {
                    return Any;
                };
                let args = self.construct(
                    name,
                    &decl.name,
                    &decl.params,
                    &decl.variants[i].fields,
                    fields,
                    expected,
                    e.pos,
                );
                Enum(decl.name.clone(), args)
            }
        }
    }

    /// Checks a record or variant value against its (possibly generic) declaration
    /// `owner`, and works out the type arguments: from the expected type, as in
    /// `let x: Option[Int] = None`, and from the field values.
    #[allow(clippy::too_many_arguments)]
    fn construct(
        &mut self,
        name: &str,
        owner: &str,
        params: &[String],
        decl: &'a [(String, Type, Pos)],
        given: &'a [(String, Expr, Pos)],
        expected: Option<&Type>,
        pos: Pos,
    ) -> Vec<Type> {
        let mut b = HashMap::new();
        if let Some(Type::Record(n, args) | Type::Enum(n, args)) = expected {
            if n == owner && args.len() == params.len() {
                for (p, a) in params.iter().zip(args) {
                    if !has_any(a) {
                        b.insert(p.clone(), a.clone());
                    }
                }
            }
        }
        self.field_values(name, params, &mut b, decl, given, pos);
        // As an argument to a generic call (`unwrap_or(None, 5)`), the call itself
        // works out what is still unknown here.
        let context_decides = match expected {
            Some(Type::Record(n, _) | Type::Enum(n, _)) => n == owner,
            Some(Type::Any) => true,
            _ => false,
        };
        params
            .iter()
            .map(|p| match b.get(p) {
                Some(t) => t.clone(),
                None if context_decides => Type::Any,
                None => {
                    self.errors.push(
                        Error::new(
                            "type",
                            pos,
                            format!("cannot tell what `{}` is in this `{}`", p, name),
                        )
                        .hint(format!(
                            "give it a type annotation, e.g. `let x: {}[...] = ...`",
                            owner
                        )),
                    );
                    Type::Any
                }
            })
            .collect()
    }

    /// Checks the fields given in a record or variant value against its declaration,
    /// learning type parameters into `b` as it goes.
    fn field_values(
        &mut self,
        name: &str,
        params: &[String],
        b: &mut HashMap<String, Type>,
        decl: &'a [(String, Type, Pos)],
        given: &'a [(String, Expr, Pos)],
        pos: Pos,
    ) {
        let mut seen = HashSet::new();
        for (f, fe, fpos) in given {
            if !seen.insert(f.as_str()) {
                self.err("name", *fpos, format!("field `{}` is given twice", f));
            }
            match decl.iter().find(|d| d.0 == *f) {
                Some((_, ft, _)) => {
                    let want = hint(ft, b, params);
                    let actual = self.expr(fe, Some(&want));
                    if !bind(ft, &actual, params, b) {
                        self.err(
                            "type",
                            fe.pos,
                            format!(
                                "field `{}` of `{}` must be {}, found {}",
                                f,
                                name,
                                hint(ft, b, params),
                                actual
                            ),
                        );
                    }
                }
                None => {
                    self.expr(fe, None);
                    let names: Vec<&str> = decl.iter().map(|d| d.0.as_str()).collect();
                    let hint = if names.is_empty() {
                        format!("`{}` has no fields", name)
                    } else {
                        format!("its fields are: {}", names.join(", "))
                    };
                    self.errors.push(
                        Error::new("type", *fpos, format!("`{}` has no field `{}`", name, f))
                            .hint(hint),
                    );
                }
            }
        }
        let missing: Vec<&str> = decl
            .iter()
            .map(|d| d.0.as_str())
            .filter(|f| !seen.contains(f))
            .collect();
        if !missing.is_empty() {
            self.errors.push(
                Error::new(
                    "type",
                    pos,
                    format!("`{}` is missing field(s): {}", name, missing.join(", ")),
                )
                .hint("every field must be given a value"),
            );
        }
    }

    fn match_stmt(&mut self, scrutinee: &'a Expr, arms: &'a [Arm], pos: Pos) {
        let t = self.expr(scrutinee, None);
        let mut targs = HashMap::new();
        let decl = match &t {
            Type::Enum(n, args) => {
                let d = self.prog.enums.iter().find(|e| &e.name == n);
                if let Some(d) = d {
                    targs = bindings(&d.params, args);
                }
                d
            }
            Type::Any => None,
            other => {
                self.err(
                    "type",
                    scrutinee.pos,
                    format!("`match` needs an enum value, found {}", other),
                );
                None
            }
        };
        let mut covered = HashSet::new();
        let mut wildcard = false;
        for arm in arms {
            let mut scope = HashMap::new();
            match &arm.pattern {
                Pattern::Wildcard(ppos) => {
                    if wildcard {
                        self.err("type", *ppos, "this `_` arm can never run".into());
                    }
                    wildcard = true;
                }
                Pattern::Variant(v, binds, ppos) => {
                    if wildcard {
                        self.errors.push(
                            Error::new("type", *ppos, format!("the `{}` arm can never run", v))
                                .hint("the `_` arm above it already matches everything"),
                        );
                    }
                    let variant = decl.and_then(|d| d.variants.iter().find(|x| &x.name == v));
                    match (decl, variant) {
                        (Some(d), None) => {
                            let names: Vec<&str> =
                                d.variants.iter().map(|x| x.name.as_str()).collect();
                            self.errors.push(
                                Error::new(
                                    "type",
                                    *ppos,
                                    format!("`{}` is not a variant of `{}`", v, d.name),
                                )
                                .hint(format!("its variants are: {}", names.join(", "))),
                            );
                        }
                        (_, Some(var)) => {
                            if !covered.insert(v.as_str()) {
                                self.err(
                                    "type",
                                    *ppos,
                                    format!("`{}` is already matched by an earlier arm", v),
                                );
                            }
                            for (f, name, fpos) in binds {
                                match var.field(f) {
                                    Some((_, ft)) => {
                                        if scope.insert(name.clone(), ft.subst(&targs)).is_some() {
                                            self.err(
                                                "name",
                                                *fpos,
                                                format!("`{}` is bound twice", name),
                                            );
                                        }
                                    }
                                    None => {
                                        self.err(
                                            "type",
                                            *fpos,
                                            format!("`{}` has no field `{}`", v, f),
                                        );
                                        // Bind anyway, so uses of it don't add more errors.
                                        scope.insert(name.clone(), Type::Any);
                                    }
                                }
                            }
                        }
                        (None, None) => {}
                    }
                }
            }
            self.scopes.push(scope);
            self.block(&arm.body);
            self.scopes.pop();
        }
        if let Some(d) = decl {
            let missing: Vec<&str> = d
                .variants
                .iter()
                .map(|v| v.name.as_str())
                .filter(|v| !covered.contains(v))
                .collect();
            if !wildcard && !missing.is_empty() {
                self.errors.push(
                    Error::new(
                        "type",
                        pos,
                        format!("`match` doesn't handle: {}", missing.join(", ")),
                    )
                    .hint("add an arm for each, or `_ => { ... }` for the rest"),
                );
            }
        }
    }

    /// The type of field `f` of a value of type `t`, reporting an error if there is none.
    fn field_type(&mut self, t: &Type, f: &str, pos: Pos) -> Type {
        match t {
            Type::Any => Type::Any,
            Type::Record(n, args) => match self.records.get(n.as_str()).copied() {
                Some(decl) => match decl.field(f) {
                    Some((_, ft)) => ft.subst(&bindings(&decl.params, args)),
                    None => {
                        self.errors.push(self.no_field(decl, f, pos));
                        Type::Any
                    }
                },
                None => Type::Any,
            },
            Type::Enum(n, _) => {
                self.errors.push(
                    Error::new(
                        "type",
                        pos,
                        format!("can't read `.{}` directly from an enum `{}`", f, n),
                    )
                    .hint("use `match` to find out which variant it is and bind its fields"),
                );
                Type::Any
            }
            _ => {
                self.err(
                    "type",
                    pos,
                    format!("{} has no fields, so `.{}` is not allowed", t, f),
                );
                Type::Any
            }
        }
    }

    fn no_field(&self, decl: &TypeDecl, f: &str, pos: Pos) -> Error {
        let names: Vec<&str> = decl.fields.iter().map(|f| f.0.as_str()).collect();
        Error::new("type", pos, format!("`{}` has no field `{}`", decl.name, f))
            .hint(format!("its fields are: {}", names.join(", ")))
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

    fn call(&mut self, name: &str, args: &'a [Expr], e: &'a Expr, expected: Option<&Type>) -> Type {
        let pos = e.pos;
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
            let mut err = Error::new("name", pos, format!("unknown function `{}`", name));
            // `dist2` when an imported module has `lib.geometry.dist2`: suggest `geometry.dist2`.
            let suffix = format!(".{}", name);
            let mut found: Vec<&str> = self
                .sigs
                .keys()
                .filter(|k| k.ends_with(&suffix))
                .copied()
                .collect();
            found.sort();
            if let Some(full) = found.first() {
                let parts: Vec<&str> = full.rsplitn(3, '.').collect();
                if parts.len() >= 2 {
                    err = err.hint(format!("did you mean `{}.{}`?", parts[1], parts[0]));
                }
            }
            self.errors.push(err);
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
            return Type::Any;
        }
        // Work out the type parameters: from the expected result type, then from the arguments.
        let tps = &f.tparams;
        let mut b = HashMap::new();
        if let Some(exp) = expected.filter(|_| !tps.is_empty()) {
            let mut from_expected = HashMap::new();
            if bind(&f.ret, exp, tps, &mut from_expected) {
                b = from_expected;
            }
        }
        for (p, a) in f.params.iter().zip(args) {
            let want = hint(&p.ty, &b, tps);
            let actual = self.expr(a, Some(&want));
            if !bind(&p.ty, &actual, tps, &mut b) {
                self.err(
                    "type",
                    a.pos,
                    format!(
                        "argument `{}` of `{}` must be {}, found {}",
                        p.name,
                        name,
                        hint(&p.ty, &b, tps),
                        actual
                    ),
                );
            }
        }
        if !tps.is_empty() {
            let mut targs = vec![];
            for tp in tps {
                match b.get(tp) {
                    Some(t) => targs.push(t.clone()),
                    None => {
                        self.errors.push(
                            Error::new(
                                "type",
                                pos,
                                format!("cannot tell what `{}` is in this call to `{}`", tp, name),
                            )
                            .hint(format!(
                                "give the result a type annotation, e.g. `let x: {} = {}(...)`",
                                f.ret, name
                            )),
                        );
                        targs.push(Type::Any);
                    }
                }
            }
            self.targs.insert(e as *const Expr, targs);
        }
        hint(&f.ret, &b, tps)
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
            "parse_int" => {
                self.expect(&args[0], &Str, &arg(0));
                Enum("Option".into(), vec![Int])
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

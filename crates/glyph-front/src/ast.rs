use crate::error::Pos;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Str,
    Bool,
    Unit,
    Any,
    List(Box<Type>),
    /// A user-defined record type with its type arguments: `Pair[Int, Str]`.
    Record(String, Vec<Type>),
    /// A user-defined enum type with its type arguments: `Option[Int]`.
    Enum(String, Vec<Type>),
    /// A type parameter of a generic declaration or function: `T`.
    Param(String),
}

impl Type {
    /// Replaces type parameters that `bindings` knows; others stay as they are.
    pub fn subst(&self, bindings: &HashMap<String, Type>) -> Type {
        match self {
            Type::Param(p) => bindings.get(p).cloned().unwrap_or_else(|| self.clone()),
            Type::List(x) => Type::List(Box::new(x.subst(bindings))),
            Type::Record(n, args) => {
                Type::Record(n.clone(), args.iter().map(|a| a.subst(bindings)).collect())
            }
            Type::Enum(n, args) => {
                Type::Enum(n.clone(), args.iter().map(|a| a.subst(bindings)).collect())
            }
            _ => self.clone(),
        }
    }
}

/// `params[i] -> args[i]`, for substituting a generic declaration's type parameters.
pub fn bindings(params: &[String], args: &[Type]) -> HashMap<String, Type> {
    params.iter().cloned().zip(args.iter().cloned()).collect()
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Str => write!(f, "Str"),
            Type::Bool => write!(f, "Bool"),
            Type::Unit => write!(f, "Unit"),
            Type::Any => write!(f, "Any"),
            Type::List(t) => write!(f, "List[{}]", t),
            Type::Record(n, args) | Type::Enum(n, args) if args.is_empty() => write!(f, "{}", n),
            Type::Record(n, args) | Type::Enum(n, args) => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{}[{}]", n, args.join(", "))
            }
            Type::Param(p) => write!(f, "{}", p),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        use BinOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Mod => "%",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "and",
            Or => "or",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
    /// The exact source text, shown in contract and assertion failures.
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Str(String),
    Bool(bool),
    Var(String),
    List(Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    /// `Point { x: 1, y: 2 }`: fields in source order.
    Record(String, Vec<(String, Expr, Pos)>),
    /// `p.x`
    Field(Box<Expr>, String),
    /// An enum value: `Circle { radius: 2 }` or a variant without fields like `Empty`.
    Variant(String, Vec<(String, Expr, Pos)>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    /// `_`
    Wildcard(Pos),
    /// `Circle { radius }` or `Circle { radius: r }`: (field, variable to bind, position).
    Variant(String, Vec<(String, String, Pos)>, Pos),
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub pattern: Pattern,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, Option<Type>, Expr),
    Assign(String, Expr, Pos),
    /// `p.x = e` or `p.a.b = e`: replaces the variable with an updated copy.
    SetField(String, Vec<(String, Pos)>, Expr, Pos),
    Expr(Expr),
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    For(String, Expr, Vec<Stmt>, Pos),
    Return(Option<Expr>, Pos),
    Assert(Expr),
    Match(Expr, Vec<Arm>, Pos),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    /// Declared `pub`: other modules may call it.
    pub public: bool,
    /// Type parameters: `fn first[T](xs: List[T]) -> Option[T]`.
    pub tparams: Vec<String>,
    pub params: Vec<Param>,
    pub ret: Type,
    pub effects: Vec<(String, Pos)>,
    pub requires: Vec<Expr>,
    pub ensures: Vec<Expr>,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub struct Test {
    pub name: String,
    pub effects: Vec<(String, Pos)>,
    pub body: Vec<Stmt>,
    /// Source text of the body, so `glyph fix` can detect edited tests.
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub public: bool,
    pub params: Vec<String>,
    pub fields: Vec<(String, Type, Pos)>,
    pub pos: Pos,
}

impl TypeDecl {
    pub fn field(&self, name: &str) -> Option<(usize, &Type)> {
        self.fields
            .iter()
            .position(|f| f.0 == name)
            .map(|i| (i, &self.fields[i].1))
    }
}

#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<(String, Type, Pos)>,
    pub pos: Pos,
}

impl VariantDecl {
    pub fn field(&self, name: &str) -> Option<(usize, &Type)> {
        self.fields
            .iter()
            .position(|f| f.0 == name)
            .map(|i| (i, &self.fields[i].1))
    }
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub public: bool,
    pub params: Vec<String>,
    pub variants: Vec<VariantDecl>,
    pub pos: Pos,
}

impl Program {
    /// The enum a variant belongs to, and the variant's index (its tag).
    pub fn variant(&self, name: &str) -> Option<(&EnumDecl, usize)> {
        self.enums.iter().find_map(|e| {
            e.variants
                .iter()
                .position(|v| v.name == name)
                .map(|i| (e, i))
        })
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub types: Vec<TypeDecl>,
    pub enums: Vec<EnumDecl>,
    pub funcs: Vec<Func>,
    pub tests: Vec<Test>,
}

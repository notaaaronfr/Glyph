use crate::error::Pos;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Str,
    Bool,
    Unit,
    Any,
    List(Box<Type>),
    /// A user-defined record type, by name.
    Record(String),
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
            Type::Record(n) => write!(f, "{}", n),
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
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
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
pub struct Program {
    pub types: Vec<TypeDecl>,
    pub funcs: Vec<Func>,
    pub tests: Vec<Test>,
}

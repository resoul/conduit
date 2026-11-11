use crate::lexer::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Named(String),
    Generic {
        name: String,
        type_args: Vec<TypeExpr>,
    },
    List(Box<TypeExpr>),
    Optional(Box<TypeExpr>),
    Union(Vec<TypeExpr>),
    Tuple(Vec<TypeExpr>),
    Func {
        params: Vec<TypeExpr>,
        return_type: Box<TypeExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Coalesce,
    Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedArg {
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nil,
    Ident(String),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Try(Box<Expr>),
    Call {
        callee: Box<Expr>,
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        named_args: Vec<NamedArg>,
    },
    Member {
        target: Box<Expr>,
        field: String,
    },
    OptionalMember {
        target: Box<Expr>,
        field: String,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Closure {
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Block,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Type {
        name: String,
        binding: String,
    },
    Tuple(Vec<Pattern>),
    Nil,
    Wildcard,
    Ident(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

impl Pattern {
    pub fn new(kind: PatternKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Define {
        name: String,
        value: Expr,
    },
    DestructureDefine {
        pattern: Pattern,
        value: Expr,
    },
    VarDecl {
        name: String,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    Return(Option<Expr>),
    If {
        condition: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    Guard {
        condition: Expr,
        else_block: Block,
    },
    ForIn {
        pattern: Pattern,
        iterable: Expr,
        body: Block,
    },
    While {
        condition: Expr,
        body: Block,
    },
    Switch {
        expr: Expr,
        cases: Vec<SwitchCase>,
    },
    Break,
    Continue,
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

impl Stmt {
    pub fn new(kind: StmtKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAnnotation {
    pub target: String,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub ty: TypeExpr,
    pub annotations: Vec<FieldAnnotation>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Func {
        name: String,
        type_params: Vec<String>,
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Block,
        is_validator: bool,
    },
    Type {
        name: String,
        type_params: Vec<String>,
        fields: Vec<FieldDef>,
    },
    Error {
        name: String,
        fields: Vec<FieldDef>,
    },
    Import {
        path: String,
        alias: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
    pub is_exported: bool,
}

impl Item {
    pub fn new(kind: ItemKind, span: Span) -> Self {
        Self {
            kind,
            span,
            is_exported: false,
        }
    }

    pub fn new_exported(kind: ItemKind, span: Span, is_exported: bool) -> Self {
        Self {
            kind,
            span,
            is_exported,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}


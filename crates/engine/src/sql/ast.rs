#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ast {
    Col { q: Option<String>, name: String },
    /// Numeric literal text, classified at bind time (integer / decimal / float).
    Num(String),
    Str(String),
    Null,
    Bin(AstOp, Box<Ast>, Box<Ast>),
    Neg(Box<Ast>),
    Not(Box<Ast>),
    IsNull(Box<Ast>, bool),
    /// `arg == None` means `COUNT(*)`.
    Agg { func: AggFunc, arg: Option<Box<Ast>>, distinct: bool },
}

#[derive(Clone, Debug)]
pub struct SelectItem {
    pub expr: Ast,
    pub alias: Option<String>,
    /// Source text, used as the output column name for unnamed expressions.
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    /// Comma join; the equality condition must come from WHERE.
    Comma,
}

#[derive(Clone, Debug)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Join {
    pub kind: JoinKind,
    pub table: TableRef,
    pub on: Option<Ast>,
}

#[derive(Clone, Debug)]
pub struct Select {
    pub star: bool,
    pub items: Vec<SelectItem>,
    pub from: TableRef,
    pub joins: Vec<Join>,
    pub where_: Option<Ast>,
    pub group_by: Vec<Ast>,
    pub having: Option<Ast>,
    pub order_by: Vec<(Ast, bool)>, // (expr, descending)
    pub limit: Option<u64>,
}

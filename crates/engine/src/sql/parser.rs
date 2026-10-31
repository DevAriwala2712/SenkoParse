use super::ast::*;
use super::lexer::{Tok, Token};
use crate::{err, Result};

const RESERVED: &[&str] = &[
    "select", "from", "where", "group", "by", "having", "order", "limit", "join", "inner", "left", "right", "full",
    "cross", "outer", "on", "and", "or", "not", "is", "null", "as", "asc", "desc", "distinct", "union", "intersect",
    "except", "like", "in", "between", "case", "when", "then", "else", "end", "offset", "using", "natural", "with",
    "exists", "all", "any",
];
/// Keywords we recognise only to give a clear "unsupported" error.
const UNSUPPORTED: &[&str] = &[
    "right", "full", "cross", "natural", "union", "intersect", "except", "like", "in", "between", "case", "offset", "using",
    "with", "exists",
];

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    p: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, toks: Vec<Token>) -> Self {
        Parser { src, toks, p: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.p].tok
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.p].tok.clone();
        if self.p + 1 < self.toks.len() {
            self.p += 1;
        }
        t
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &str) -> Result<()> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("'{s}'")))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.unexpected(&kw.to_ascii_uppercase()))
        }
    }
    fn unexpected(&self, want: &str) -> crate::Error {
        let t = &self.toks[self.p];
        if let Tok::Ident(s) = &t.tok {
            if UNSUPPORTED.contains(&s.as_str()) {
                return err!("unsupported SQL feature: {}", s.to_ascii_uppercase());
            }
        }
        let got = if t.tok == Tok::Eof { "end of input".to_string() } else { format!("'{}'", &self.src[t.start..t.end]) };
        err!("syntax error at offset {}: expected {want}, found {got}", t.start)
    }

    fn ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.unexpected("identifier")),
        }
    }

    pub fn parse_select(&mut self) -> Result<Select> {
        self.expect_kw("select")?;
        if self.is_kw("distinct") {
            return Err(err!("unsupported SQL feature: SELECT DISTINCT"));
        }
        let mut star = false;
        let mut items = Vec::new();
        loop {
            if self.is_sym("*") {
                self.bump();
                star = true;
            } else {
                let start = self.toks[self.p].start;
                let expr = self.expr()?;
                let end = self.toks[self.p - 1].end;
                let alias = if self.eat_kw("as") {
                    Some(self.ident()?)
                } else if matches!(self.peek(), Tok::Ident(s) if !RESERVED.contains(&s.as_str())) {
                    Some(self.ident()?)
                } else {
                    None
                };
                items.push(SelectItem { expr, alias, text: self.src[start..end].trim().to_string() });
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        if star && !items.is_empty() {
            return Err(err!("unsupported SQL feature: `*` mixed with other select items"));
        }
        self.expect_kw("from")?;
        let from = self.table_ref()?;
        let mut joins = Vec::new();
        loop {
            if self.eat_sym(",") {
                joins.push(Join { kind: JoinKind::Comma, table: self.table_ref()?, on: None });
                continue;
            }
            let kind = if self.eat_kw("join") {
                JoinKind::Inner
            } else if self.is_kw("inner") {
                self.bump();
                self.expect_kw("join")?;
                JoinKind::Inner
            } else if self.is_kw("left") {
                self.bump();
                self.eat_kw("outer");
                self.expect_kw("join")?;
                JoinKind::Left
            } else if matches!(self.peek(), Tok::Ident(s) if matches!(s.as_str(), "right"|"full"|"cross"|"natural")) {
                return Err(self.unexpected("JOIN"));
            } else {
                break;
            };
            let table = self.table_ref()?;
            self.expect_kw("on")?;
            joins.push(Join { kind, table, on: Some(self.expr()?) });
        }
        let where_ = if self.eat_kw("where") { Some(self.expr()?) } else { None };
        let mut group_by = Vec::new();
        if self.eat_kw("group") {
            self.expect_kw("by")?;
            loop {
                group_by.push(self.expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let having = if self.eat_kw("having") { Some(self.expr()?) } else { None };
        let mut order_by = Vec::new();
        if self.eat_kw("order") {
            self.expect_kw("by")?;
            loop {
                let e = self.expr()?;
                let desc = if self.eat_kw("desc") {
                    true
                } else {
                    self.eat_kw("asc");
                    false
                };
                order_by.push((e, desc));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let limit = if self.eat_kw("limit") {
            match self.bump() {
                Tok::Num(n) => Some(n.parse::<u64>().map_err(|_| err!("LIMIT must be a non-negative integer"))?),
                _ => return Err(err!("LIMIT must be a non-negative integer")),
            }
        } else {
            None
        };
        self.eat_sym(";");
        if *self.peek() != Tok::Eof {
            return Err(self.unexpected("end of statement"));
        }
        Ok(Select { star, items, from, joins, where_, group_by, having, order_by, limit })
    }

    fn table_ref(&mut self) -> Result<TableRef> {
        let name = self.ident()?;
        let alias = if self.eat_kw("as") {
            Some(self.ident()?)
        } else if matches!(self.peek(), Tok::Ident(s) if !RESERVED.contains(&s.as_str())) {
            Some(self.ident()?)
        } else {
            None
        };
        Ok(TableRef { name, alias })
    }

    // precedence: OR < AND < NOT < comparison/IS < + - < * / < unary -
    pub fn expr(&mut self) -> Result<Ast> {
        let mut l = self.and_expr()?;
        while self.eat_kw("or") {
            l = Ast::Bin(AstOp::Or, Box::new(l), Box::new(self.and_expr()?));
        }
        Ok(l)
    }
    fn and_expr(&mut self) -> Result<Ast> {
        let mut l = self.not_expr()?;
        while self.eat_kw("and") {
            l = Ast::Bin(AstOp::And, Box::new(l), Box::new(self.not_expr()?));
        }
        Ok(l)
    }
    fn not_expr(&mut self) -> Result<Ast> {
        if self.eat_kw("not") {
            return Ok(Ast::Not(Box::new(self.not_expr()?)));
        }
        self.cmp_expr()
    }
    fn cmp_expr(&mut self) -> Result<Ast> {
        let l = self.add_expr()?;
        if self.eat_kw("is") {
            let neg = self.eat_kw("not");
            self.expect_kw("null")?;
            return Ok(Ast::IsNull(Box::new(l), neg));
        }
        let op = match self.peek() {
            Tok::Sym("=") => AstOp::Eq,
            Tok::Sym("<>") => AstOp::Ne,
            Tok::Sym("<") => AstOp::Lt,
            Tok::Sym("<=") => AstOp::Le,
            Tok::Sym(">") => AstOp::Gt,
            Tok::Sym(">=") => AstOp::Ge,
            Tok::Ident(s) if UNSUPPORTED.contains(&s.as_str()) || s == "not" => return Err(self.unexpected("operator")),
            _ => return Ok(l),
        };
        self.bump();
        let r = self.add_expr()?;
        Ok(Ast::Bin(op, Box::new(l), Box::new(r)))
    }
    fn add_expr(&mut self) -> Result<Ast> {
        let mut l = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("+") => AstOp::Add,
                Tok::Sym("-") => AstOp::Sub,
                _ => return Ok(l),
            };
            self.bump();
            l = Ast::Bin(op, Box::new(l), Box::new(self.mul_expr()?));
        }
    }
    fn mul_expr(&mut self) -> Result<Ast> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("*") => AstOp::Mul,
                Tok::Sym("/") => AstOp::Div,
                _ => return Ok(l),
            };
            self.bump();
            l = Ast::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
    }
    fn unary(&mut self) -> Result<Ast> {
        if self.eat_sym("-") {
            if let Tok::Num(n) = self.peek().clone() {
                self.bump();
                return Ok(Ast::Num(format!("-{n}")));
            }
            return Ok(Ast::Neg(Box::new(self.unary()?)));
        }
        if self.eat_sym("+") {
            return self.unary();
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<Ast> {
        match self.peek().clone() {
            Tok::Num(n) => {
                self.bump();
                Ok(Ast::Num(n))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Ast::Str(s))
            }
            Tok::Sym("(") => {
                self.bump();
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Ident(s) if s == "null" => {
                self.bump();
                Ok(Ast::Null)
            }
            Tok::Ident(s) => {
                if matches!(self.toks[self.p + 1].tok, Tok::Sym("(")) {
                    return self.func(&s);
                }
                let name = self.ident()?;
                if self.eat_sym(".") {
                    let col = self.ident()?;
                    return Ok(Ast::Col { q: Some(name), name: col });
                }
                Ok(Ast::Col { q: None, name })
            }
            _ => Err(self.unexpected("expression")),
        }
    }
    fn func(&mut self, name: &str) -> Result<Ast> {
        let func = match name {
            "count" => AggFunc::Count,
            "sum" => AggFunc::Sum,
            "avg" => AggFunc::Avg,
            "min" => AggFunc::Min,
            "max" => AggFunc::Max,
            _ => return Err(err!("unsupported function '{}'", name.to_ascii_uppercase())),
        };
        self.bump();
        self.expect_sym("(")?;
        let mut distinct = false;
        let arg = if self.is_sym("*") {
            if func != AggFunc::Count {
                return Err(err!("only COUNT accepts *"));
            }
            self.bump();
            None
        } else {
            if self.eat_kw("distinct") {
                if func != AggFunc::Count {
                    return Err(err!("unsupported SQL feature: {}(DISTINCT ...)", name.to_ascii_uppercase()));
                }
                distinct = true;
            }
            Some(Box::new(self.expr()?))
        };
        self.expect_sym(")")?;
        Ok(Ast::Agg { func, arg, distinct })
    }
}

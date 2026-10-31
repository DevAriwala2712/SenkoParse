pub mod ast;
pub mod lexer;
pub mod parser;

pub use ast::*;

pub fn parse(sql: &str) -> crate::Result<Select> {
    let toks = lexer::lex(sql)?;
    parser::Parser::new(sql, toks).parse_select()
}

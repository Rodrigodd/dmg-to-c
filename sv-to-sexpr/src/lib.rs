pub mod analyze;
pub mod ast;
pub mod cli;
pub mod diagnostic;
pub mod elaborate;
pub mod inventory;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod serialize;
pub mod survey;

pub use cli::run;

pub mod analyze;
pub mod ast;
pub mod cli;
pub mod convert;
pub mod diagnostic;
pub mod elaborate;
pub mod hierarchy;
pub mod inventory;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod serialize;
pub mod survey;

pub use cli::run;

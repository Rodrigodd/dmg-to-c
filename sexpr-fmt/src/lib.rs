mod error;
mod format;
mod lexer;
mod parser;

pub use error::{Location, ParseError, ParseErrorKind};
pub use format::FormatOptions;

/// Parse and canonically format one S-expression source document.
pub fn format_source(source: &str, options: FormatOptions) -> Result<String, ParseError> {
    let document = parser::parse_document(source)?;
    Ok(format::format_document(&document, options))
}

/// Parse and canonically format one S-expression source document with the
/// formatter's default options.
pub fn format_source_default(source: &str) -> Result<String, ParseError> {
    format_source(source, FormatOptions::default())
}

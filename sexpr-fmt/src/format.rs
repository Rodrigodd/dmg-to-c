use crate::parser::{Atom, Document, Expr, Item, List};

#[derive(Debug, Clone, Copy)]
pub struct FormatOptions {
    pub width: usize,
    pub max_inline_items: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            width: 100,
            max_inline_items: 4,
        }
    }
}

pub fn format_document(document: &Document, options: FormatOptions) -> String {
    let mut out = String::new();
    format_items(&document.items, 0, true, &options, &mut out);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn format_items(
    items: &[Item],
    indent: usize,
    top_level: bool,
    options: &FormatOptions,
    out: &mut String,
) {
    let mut previous_emitted_expr = false;
    let mut pending_blank_line = false;

    for item in items {
        match item {
            Item::BlankLine => pending_blank_line = true,
            Item::Comment(comment) => {
                if pending_blank_line && !out.is_empty() {
                    out.push('\n');
                }
                pending_blank_line = false;
                write_indent(out, indent);
                out.push_str(&comment.text);
                out.push('\n');
                previous_emitted_expr = false;
            }
            Item::Expr(expr) => {
                if (pending_blank_line && !out.is_empty()) || (top_level && previous_emitted_expr) {
                    out.push('\n');
                }
                pending_blank_line = false;
                render_expr(expr, indent, options, out);
                previous_emitted_expr = true;
            }
        }
    }
}

fn render_expr(expr: &Expr, indent: usize, options: &FormatOptions, out: &mut String) {
    if let Some(inline) = inline_expr(expr, options)
        && (indent + inline.len() <= options.width || matches!(expr, Expr::Atom(_)))
    {
        write_indent(out, indent);
        out.push_str(&inline);
        out.push('\n');
        return;
    }

    match expr {
        Expr::Atom(atom) => render_atom(atom, indent, out),
        Expr::List(list) => render_list(list, indent, options, out),
    }
}

fn render_atom(atom: &Atom, indent: usize, out: &mut String) {
    write_indent(out, indent);
    out.push_str(&atom.text);
    if let Some(comment) = &atom.trailing_comment {
        out.push(' ');
        out.push_str(&comment.text);
    }
    out.push('\n');
}

fn render_list(list: &List, indent: usize, options: &FormatOptions, out: &mut String) {
    if list.children.is_empty() {
        write_indent(out, indent);
        out.push_str("()");
        if let Some(comment) = &list.trailing_comment {
            out.push(' ');
            out.push_str(&comment.text);
        }
        out.push('\n');
        return;
    }

    write_indent(out, indent);
    out.push('(');

    let mut start_index = 0usize;
    if let Some(Item::Expr(first_expr)) = list.children.first() {
        if let Some(first_inline) = inline_expr(first_expr, options) {
            if indent + 1 + first_inline.len() <= options.width {
                out.push_str(&first_inline);
                out.push('\n');
                start_index = 1;
            } else {
                out.push('\n');
            }
        } else {
            out.push('\n');
        }
    } else {
        out.push('\n');
    }

    if start_index == 0 {
        format_items(&list.children, indent + 2, false, options, out);
    } else {
        format_items(
            &list.children[start_index..],
            indent + 2,
            false,
            options,
            out,
        );
    }

    write_indent(out, indent);
    out.push(')');
    if let Some(comment) = &list.trailing_comment {
        out.push(' ');
        out.push_str(&comment.text);
    }
    out.push('\n');
}

fn inline_expr(expr: &Expr, options: &FormatOptions) -> Option<String> {
    match expr {
        Expr::Atom(atom) => Some(inline_atom(atom)),
        Expr::List(list) => inline_list(list, options),
    }
}

fn inline_atom(atom: &Atom) -> String {
    let mut s = atom.text.clone();
    if let Some(comment) = &atom.trailing_comment {
        s.push(' ');
        s.push_str(&comment.text);
    }
    s
}

fn inline_list(list: &List, options: &FormatOptions) -> Option<String> {
    let mut parts = Vec::new();

    for item in &list.children {
        match item {
            Item::Expr(expr) => parts.push(inline_expr(expr, options)?),
            _ => return None,
        }
    }

    if parts.len() > options.max_inline_items {
        return None;
    }

    let mut s = String::from("(");
    s.push_str(&parts.join(" "));
    s.push(')');
    if let Some(comment) = &list.trailing_comment {
        s.push(' ');
        s.push_str(&comment.text);
    }
    Some(s)
}

fn write_indent(out: &mut String, indent: usize) {
    out.push_str(&" ".repeat(indent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_document;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn inline_short_form_stays_one_line() {
        let doc = parse_document("(a b c)").unwrap();
        assert_eq!(format_document(&doc, FormatOptions::default()), "(a b c)\n");
    }

    #[test]
    fn long_form_breaks_by_width() {
        let doc = parse_document("(alpha beta gamma delta epsilon)").unwrap();
        let formatted = format_document(
            &doc,
            FormatOptions {
                width: 20,
                max_inline_items: 4,
            },
        );
        assert!(formatted.contains("\n  beta\n"));
    }

    #[test]
    fn comment_lines_are_preserved() {
        let doc = parse_document("(a)\n; note\n(b)").unwrap();
        let formatted = format_document(&doc, FormatOptions::default());
        assert!(formatted.contains("; note"));
    }

    #[test]
    fn fixture_round_trips() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell");
        let source = fs::read_to_string(path).unwrap();
        let doc = parse_document(&source).unwrap();
        let formatted = format_document(&doc, FormatOptions::default());
        let reparsed = parse_document(&formatted).unwrap();
        let reformatted = format_document(&reparsed, FormatOptions::default());
        assert_eq!(formatted, reformatted);
    }
}

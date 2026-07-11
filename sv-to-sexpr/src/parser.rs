use crate::ast::*;
use crate::diagnostic::{Diagnostic, Span};
use crate::lexer::{Keyword, Operator, Punct, Token, TokenKind, lex_file};
use std::path::Path;

pub type ParseResult<T> = Result<T, Diagnostic>;

pub fn parse_file(path: &Path, input: &str) -> ParseResult<Design> {
    let tokens = lex_file(path, input)?;
    Parser::new(tokens, eof_span(path, input)).parse_design()
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    eof_span: Span,
}

impl Parser {
    fn new(tokens: Vec<Token>, eof_span: Span) -> Self {
        Self {
            tokens,
            index: 0,
            eof_span,
        }
    }

    fn parse_design(mut self) -> ParseResult<Design> {
        let mut items = Vec::new();
        while !self.is_eof() {
            match self.peek_kind() {
                Some(TokenKind::Directive) => {
                    items.push(DesignItem::Directive(self.parse_directive()?));
                }
                Some(TokenKind::Keyword(Keyword::Module)) => {
                    items.push(DesignItem::Module(self.parse_module()?));
                }
                _ => return Err(self.error_current("expected directive or `module` declaration")),
            }
        }
        Ok(Design { items })
    }

    fn parse_directive(&mut self) -> ParseResult<Directive> {
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current("expected directive"))?;
        let mut parts = token.lexeme.split_whitespace();
        let name = parts
            .next()
            .and_then(|part| part.strip_prefix('`'))
            .filter(|name| !name.is_empty())
            .ok_or_else(|| self.error_at(&token, "expected directive name"))?;
        let arguments = parts.map(str::to_string).collect::<Vec<_>>();

        if name != "default_nettype" {
            return Err(self.error_at(
                &token,
                &format!("unsupported directive `{name}` at design scope"),
            ));
        }
        if arguments.len() != 1 || !matches!(arguments[0].as_str(), "none" | "wire") {
            return Err(self.error_at(
                &token,
                "expected `default_nettype` argument `none` or `wire`",
            ));
        }

        Ok(Directive {
            span: token.span,
            name: name.to_string(),
            arguments,
        })
    }

    fn parse_module(&mut self) -> ParseResult<Module> {
        let start = self.expect_keyword(Keyword::Module)?.span.clone();
        let name = self.expect_identifier("expected module name")?.lexeme;
        let mut parameters = Vec::new();
        if self.matches_operator(Operator::Hash) {
            self.expect_punct(Punct::LParen)?;
            if !self.peek_punct(Punct::RParen) {
                loop {
                    parameters.push(self.parse_module_parameter()?);
                    if self.matches_punct(Punct::Comma) {
                        if self.peek_punct(Punct::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect_punct(Punct::RParen)?;
        }
        let mut ports = Vec::new();
        if self.matches_punct(Punct::LParen) {
            if !self.peek_punct(Punct::RParen) {
                loop {
                    ports.push(self.parse_port_decl()?);
                    if self.matches_punct(Punct::Comma) {
                        if self.peek_punct(Punct::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect_punct(Punct::RParen)?;
        }
        self.expect_punct(Punct::Semicolon)?;

        let mut items = Vec::new();
        loop {
            if self.matches_keyword(Keyword::Endmodule) {
                break;
            }
            if self.is_eof() {
                return Err(self.error_current("unterminated module body"));
            }
            if let Some(item) = self.parse_item()? {
                items.push(item);
            } else {
                break;
            }
        }

        Ok(Module {
            span: start,
            name,
            parameters,
            ports,
            items,
        })
    }

    fn parse_module_parameter(&mut self) -> ParseResult<ParamDecl> {
        let start = self.expect_keyword(Keyword::Parameter)?.span.clone();
        let ty = self.parse_optional_type_name();
        let name = self.expect_identifier("expected parameter name")?.lexeme;
        self.expect_operator(Operator::Equals)?;
        let value = self.parse_expr()?;
        Ok(ParamDecl {
            span: start,
            kind: ParamKind::Parameter,
            ty,
            name,
            value,
        })
    }

    fn parse_port_decl(&mut self) -> ParseResult<PortDecl> {
        let dir_tok = self
            .next_token()
            .ok_or_else(|| self.error_current("expected port direction"))?;
        let direction = match dir_tok.kind {
            TokenKind::Keyword(Keyword::Input) => Direction::Input,
            TokenKind::Keyword(Keyword::Output) => Direction::Output,
            TokenKind::Keyword(Keyword::Inout) => Direction::Inout,
            _ => return Err(self.error_at(&dir_tok, "expected port direction")),
        };
        let mut modifiers = Vec::new();
        while self.matches_any_lexeme(["tri", "wire", "logic", "real", "realtime"]) {
            modifiers.push(self.previous().lexeme.clone());
        }
        let mut names = Vec::new();
        loop {
            names.push(self.expect_identifier("expected port name")?.lexeme);
            if self.peek_punct(Punct::Comma) {
                if self.peek_next_is_direction_or_rparen() {
                    break;
                }
                self.next_token();
                continue;
            }
            break;
        }
        Ok(PortDecl {
            span: dir_tok.span,
            direction,
            modifiers,
            names,
        })
    }

    fn parse_item(&mut self) -> ParseResult<Option<Item>> {
        if self.is_eof() || self.is_terminator() {
            return Ok(None);
        }
        if matches!(self.peek_kind(), Some(TokenKind::Directive)) {
            return Err(self.error_current("directives are only supported at design scope"));
        }
        if self.peek_ident("import") {
            return self.parse_import().map(Some);
        }
        if self.peek_ident("assign") {
            return self.parse_continuous_assign().map(Some);
        }
        if self.peek_ident("initial") {
            return self.parse_initial().map(Some);
        }
        if self.peek_ident("always_latch") {
            return self.parse_always_latch().map(Some);
        }
        if self.peek_ident("always_ff") {
            return self.parse_always(AlwaysKind::Ff).map(Some);
        }
        if self.peek_ident("always_comb") {
            return self.parse_always(AlwaysKind::Comb).map(Some);
        }
        if self.peek_ident("always") {
            return self.parse_always(AlwaysKind::Plain).map(Some);
        }
        if self.peek_ident("specify") {
            return self.parse_specify().map(Some);
        }
        if self.peek_ident("generate") {
            return self.parse_generate().map(Some);
        }
        if self.peek_ident("if") {
            return self.parse_if().map(Some);
        }
        if self.peek_ident("begin") {
            return self.parse_block().map(Some);
        }
        if self.peek_ident("localparam") {
            return self.parse_decl_like(DeclKind::Localparam).map(Some);
        }
        if self.peek_ident("specparam") {
            return self.parse_decl_like(DeclKind::Specparam).map(Some);
        }
        if self.peek_ident("parameter") {
            return self.parse_decl_like(DeclKind::Parameter).map(Some);
        }
        if self.peek_ident("logic") {
            return self.parse_signal_decl(DeclKind::Logic).map(Some);
        }
        if self.peek_ident("tri") {
            return self.parse_signal_decl(DeclKind::Tri).map(Some);
        }
        if self.peek_ident("wire") {
            return self.parse_signal_decl(DeclKind::Wire).map(Some);
        }
        if matches!(self.peek_kind(), Some(TokenKind::Identifier))
            && self.peek_next_assignment_operator()
        {
            return self.parse_proc_assign().map(Some);
        }
        if matches!(self.peek_kind(), Some(TokenKind::Identifier))
            && self.looks_like_instantiation()
        {
            return self.parse_instantiation().map(Some);
        }
        if matches!(self.peek_kind(), Some(TokenKind::Identifier))
            && (self.peek_next_is_strength_or_delay() || self.peek_next_punct(Punct::LParen))
        {
            return self.parse_primitive().map(Some);
        }
        Err(self.error_current("expected a supported module item"))
    }

    fn parse_import(&mut self) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let mut path = Vec::new();
        path.push(self.expect_identifier("expected import path")?.lexeme);
        while self.matches_operator(Operator::ColonColon) {
            if self.matches_operator(Operator::Star) {
                self.expect_punct(Punct::Semicolon)?;
                return Ok(Item {
                    span: start.clone(),
                    kind: ItemKind::Import(ImportDecl {
                        span: start,
                        path,
                        wildcard: true,
                    }),
                });
            }
            path.push(
                self.expect_identifier("expected import path segment")?
                    .lexeme,
            );
        }
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Import(ImportDecl {
                span: start,
                path,
                wildcard: false,
            }),
        })
    }

    fn parse_signal_decl(&mut self, kind: DeclKind) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let ty = self.parse_optional_type_name();
        let names = self.parse_name_list()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Decl(Decl {
                span: start,
                kind,
                ty,
                names,
                value: None,
            }),
        })
    }

    fn parse_decl_like(&mut self, kind: DeclKind) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let ty = self.parse_optional_type_name();
        let name = self.expect_identifier("expected declaration name")?.lexeme;
        self.expect_operator(Operator::Equals)?;
        let value = self.parse_expr()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Decl(Decl {
                span: start,
                kind,
                ty,
                names: vec![name],
                value: Some(value),
            }),
        })
    }

    fn parse_initial(&mut self) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let body = self.parse_assignment_stmt()?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Initial(body),
        })
    }

    fn parse_proc_assign(&mut self) -> ParseResult<Item> {
        let stmt = self.parse_assignment_stmt()?;
        Ok(Item {
            span: stmt.span.clone(),
            kind: ItemKind::ProcAssign(stmt),
        })
    }

    fn parse_always_latch(&mut self) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let condition = if self.peek_ident("if") {
            self.next_token();
            self.expect_punct(Punct::LParen)?;
            let expr = self.parse_expr()?;
            self.expect_punct(Punct::RParen)?;
            Some(expr)
        } else {
            None
        };
        let body = self
            .parse_item()?
            .ok_or_else(|| self.error_current("expected always_latch body"))?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::AlwaysLatch(AlwaysLatch {
                span: start,
                condition,
                body: Box::new(body),
            }),
        })
    }

    fn parse_always(&mut self, kind: AlwaysKind) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let sensitivity = if self.matches_operator(Operator::At) {
            Some(self.parse_sensitivity(self.previous().span.clone())?)
        } else {
            None
        };
        let body = self
            .parse_item()?
            .ok_or_else(|| self.error_current("expected always body"))?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Always(AlwaysBlock {
                span: start,
                kind,
                sensitivity,
                body: Box::new(body),
            }),
        })
    }

    fn parse_continuous_assign(&mut self) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let strength = self.parse_optional_strength()?;
        let delay = self.parse_optional_delay()?;
        let target = self.parse_expr()?;
        self.expect_operator(Operator::Equals)?;
        let value = self.parse_expr()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Assign(AssignDecl {
                span: start,
                strength,
                delay,
                target,
                value,
                op: AssignOp::Blocking,
            }),
        })
    }

    fn parse_assignment_stmt(&mut self) -> ParseResult<AssignStmt> {
        let target = self.parse_expr()?;
        let op = if self.matches_operator(Operator::LessEqual) {
            AssignOp::NonBlocking
        } else {
            self.expect_operator(Operator::Equals)?;
            AssignOp::Blocking
        };
        let value = self.parse_expr()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(AssignStmt {
            span: target.span.clone(),
            target,
            value,
            op,
        })
    }

    fn parse_primitive(&mut self) -> ParseResult<Item> {
        let start = self
            .expect_identifier("expected primitive name")?
            .span
            .clone();
        let name = self.previous().lexeme.clone();
        let strength = self.parse_optional_strength()?;
        let delay = self.parse_optional_delay()?;
        let args = self.parse_paren_arg_list()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Primitive(PrimitiveCall {
                span: start,
                name,
                strength,
                delay,
                args,
            }),
        })
    }

    fn parse_instantiation(&mut self) -> ParseResult<Item> {
        let start = self.expect_identifier("expected module name")?.span.clone();
        let module = self.previous().lexeme.clone();
        let mut parameters = Vec::new();
        if self.matches_operator(Operator::Hash) {
            self.expect_punct(Punct::LParen)?;
            parameters = self.parse_param_override_list()?;
            self.expect_punct(Punct::RParen)?;
        }
        let instance = self.expect_identifier("expected instance name")?.lexeme;
        let connections = self.parse_connection_list()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Instantiation(Instantiation {
                span: start,
                module,
                parameters,
                instance,
                connections,
            }),
        })
    }

    fn parse_generate(&mut self) -> ParseResult<Item> {
        let start = self.expect_ident_exact("generate")?.span.clone();
        let mut items = Vec::new();
        loop {
            if self.peek_ident("endgenerate") {
                self.next_token();
                break;
            }
            if self.is_eof() {
                return Err(self.error_current("unterminated generate block"));
            }
            if let Some(item) = self.parse_item()? {
                items.push(item);
            } else {
                break;
            }
        }
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Generate(Block { span: start, items }),
        })
    }

    fn parse_block(&mut self) -> ParseResult<Item> {
        let start = self.expect_ident_exact("begin")?.span.clone();
        let mut items = Vec::new();
        loop {
            if self.peek_ident("end") {
                self.next_token();
                break;
            }
            if self.is_eof() {
                return Err(self.error_current("unterminated begin/end block"));
            }
            if let Some(item) = self.parse_item()? {
                items.push(item);
            } else {
                break;
            }
        }
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Block(Block { span: start, items }),
        })
    }

    fn parse_if(&mut self) -> ParseResult<Item> {
        let start = self.expect_ident_exact("if")?.span.clone();
        self.expect_punct(Punct::LParen)?;
        let condition = self.parse_expr()?;
        self.expect_punct(Punct::RParen)?;
        let then_branch = self
            .parse_item()?
            .ok_or_else(|| self.error_current("expected `if` body"))?;
        let else_branch = if self.peek_ident("else") {
            self.next_token();
            Some(Box::new(
                self.parse_item()?
                    .ok_or_else(|| self.error_current("expected `else` body"))?,
            ))
        } else {
            None
        };
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::If(IfStmt {
                span: start,
                condition,
                then_branch: Box::new(then_branch),
                else_branch,
            }),
        })
    }

    fn parse_specify(&mut self) -> ParseResult<Item> {
        let start = self.next_token().unwrap().span.clone();
        let mut items = Vec::new();
        loop {
            if self.peek_ident("endspecify") {
                break;
            }
            if self.is_eof() {
                return Err(self.error_current("unterminated specify block"));
            }
            if self.peek_ident("specparam") {
                let decl = self.parse_specparam_decl()?;
                items.push(SpecifyItem::Specparam(decl));
            } else if self.peek_punct(Punct::LParen) {
                items.push(SpecifyItem::Path(self.parse_spec_path()?));
            } else {
                return Err(self.error_current("expected `specparam` or specify path"));
            }
        }
        self.expect_ident_exact("endspecify")?;
        Ok(Item {
            span: start.clone(),
            kind: ItemKind::Specify(SpecifyBlock { span: start, items }),
        })
    }

    fn parse_specparam_decl(&mut self) -> ParseResult<ParamDecl> {
        let start = self.next_token().unwrap().span.clone();
        let ty = self.parse_optional_type_name();
        let name = self.expect_identifier("expected specparam name")?.lexeme;
        self.expect_operator(Operator::Equals)?;
        let value = self.parse_expr()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(ParamDecl {
            span: start,
            kind: ParamKind::Specparam,
            ty,
            name,
            value,
        })
    }

    fn parse_spec_path(&mut self) -> ParseResult<SpecPath> {
        let start = self.expect_punct(Punct::LParen)?.span.clone();
        let mut controls = Vec::new();
        loop {
            controls.push(self.parse_expr()?);
            if self.matches_punct(Punct::Comma) {
                continue;
            }
            self.expect_operator(Operator::Implies)?;
            break;
        }
        let target = self.parse_expr()?;
        self.expect_punct(Punct::RParen)?;
        self.expect_operator(Operator::Equals)?;
        let delays = self.parse_paren_arg_list()?;
        self.expect_punct(Punct::Semicolon)?;
        Ok(SpecPath {
            span: start,
            controls,
            target,
            delays,
        })
    }

    fn parse_sensitivity(&mut self, span: Span) -> ParseResult<Sensitivity> {
        if self.matches_operator(Operator::Star) {
            return Ok(Sensitivity {
                span,
                kind: SensitivityKind::Any,
            });
        }
        self.expect_punct(Punct::LParen)?;
        let mut list = Vec::new();
        if !self.matches_punct(Punct::RParen) {
            loop {
                let span = self.peek_span();
                let edge = if self.peek_ident("posedge") || self.peek_ident("negedge") {
                    Some(self.next_token().unwrap().lexeme.clone())
                } else {
                    None
                };
                let expr = if edge.is_some()
                    || !self.peek_punct(Punct::Comma) && !self.peek_punct(Punct::RParen)
                {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                list.push(EventControl { span, edge, expr });
                if self.matches_punct(Punct::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect_punct(Punct::RParen)?;
        Ok(Sensitivity {
            span,
            kind: SensitivityKind::List(list),
        })
    }

    fn parse_optional_strength(&mut self) -> ParseResult<Option<Strength>> {
        if !self.peek_punct(Punct::LParen) {
            return Ok(None);
        }
        if !self.looks_like_strength_group() {
            return Ok(None);
        }
        let start = self.expect_punct(Punct::LParen)?.span.clone();
        let mut values = Vec::new();
        loop {
            values.push(self.expect_identifier("expected drive strength")?.lexeme);
            if self.matches_punct(Punct::Comma) {
                continue;
            }
            break;
        }
        self.expect_punct(Punct::RParen)?;
        Ok(Some(Strength {
            span: start,
            values,
        }))
    }

    fn parse_optional_delay(&mut self) -> ParseResult<Option<Delay>> {
        if !self.matches_operator(Operator::Hash) {
            return Ok(None);
        }
        let start = self.expect_punct(Punct::LParen)?.span.clone();
        let values = self.parse_optional_expr_list(Punct::RParen)?;
        self.expect_punct(Punct::RParen)?;
        Ok(Some(Delay {
            span: start,
            values,
        }))
    }

    fn parse_paren_arg_list(&mut self) -> ParseResult<Vec<Option<Expr>>> {
        self.expect_punct(Punct::LParen)?;
        let args = self.parse_optional_expr_list(Punct::RParen)?;
        self.expect_punct(Punct::RParen)?;
        Ok(args)
    }

    fn parse_optional_expr_list(&mut self, terminator: Punct) -> ParseResult<Vec<Option<Expr>>> {
        let mut items = Vec::new();
        if self.peek_punct(terminator) {
            return Ok(items);
        }
        loop {
            if self.matches_punct(Punct::Comma) {
                items.push(None);
                continue;
            }
            items.push(Some(self.parse_expr()?));
            if self.matches_punct(Punct::Comma) {
                if self.peek_punct(terminator) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(items)
    }

    fn parse_expr(&mut self) -> ParseResult<Expr> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> ParseResult<Expr> {
        let condition = self.parse_logical_or()?;
        if self.matches_operator(Operator::Question) {
            let then_expr = self.parse_expr()?;
            self.expect_operator(Operator::Colon)?;
            let else_expr = self.parse_expr()?;
            let span = condition.span.clone();
            Ok(Expr {
                span,
                kind: ExprKind::Ternary {
                    condition: Box::new(condition),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
            })
        } else {
            Ok(condition)
        }
    }

    fn parse_logical_or(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_logical_and,
            &[Operator::DoubleOr],
            &[BinaryOp::LogicalOr],
        )
    }

    fn parse_logical_and(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_equality,
            &[Operator::DoubleAnd],
            &[BinaryOp::LogicalAnd],
        )
    }

    fn parse_equality(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_relational,
            &[
                Operator::EqualEqual,
                Operator::TripleEqual,
                Operator::NotEqual,
                Operator::NotCaseEqual,
            ],
            &[
                BinaryOp::Eq,
                BinaryOp::CaseEq,
                BinaryOp::Neq,
                BinaryOp::CaseNeq,
            ],
        )
    }

    fn parse_relational(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_bitwise_or,
            &[Operator::Less, Operator::Greater],
            &[BinaryOp::Less, BinaryOp::Greater],
        )
    }

    fn parse_bitwise_or(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_bitwise_xor,
            &[Operator::Pipe, Operator::TildePipe],
            &[BinaryOp::BitOr, BinaryOp::BitNor],
        )
    }

    fn parse_bitwise_xor(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_bitwise_and,
            &[Operator::Caret, Operator::TildeCaret],
            &[BinaryOp::BitXor, BinaryOp::BitXnor],
        )
    }

    fn parse_bitwise_and(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_additive,
            &[Operator::Ampersand, Operator::TildeAmpersand],
            &[BinaryOp::BitAnd, BinaryOp::BitNand],
        )
    }

    fn parse_additive(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_multiplicative,
            &[Operator::Plus, Operator::Minus],
            &[BinaryOp::Add, BinaryOp::Sub],
        )
    }

    fn parse_multiplicative(&mut self) -> ParseResult<Expr> {
        self.parse_binary_chain(
            Self::parse_unary,
            &[Operator::Star, Operator::Slash],
            &[BinaryOp::Mul, BinaryOp::Div],
        )
    }

    fn parse_unary(&mut self) -> ParseResult<Expr> {
        if self.matches_operator(Operator::Bang) {
            let span = self.previous().span.clone();
            let expr = self.parse_unary()?;
            return Ok(Expr {
                span,
                kind: ExprKind::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                },
            });
        }
        if self.matches_operator(Operator::Tilde) {
            let span = self.previous().span.clone();
            let expr = self.parse_unary()?;
            return Ok(Expr {
                span,
                kind: ExprKind::Unary {
                    op: UnaryOp::BitNot,
                    expr: Box::new(expr),
                },
            });
        }
        if self.matches_operator(Operator::Plus) {
            let span = self.previous().span.clone();
            let expr = self.parse_unary()?;
            return Ok(Expr {
                span,
                kind: ExprKind::Unary {
                    op: UnaryOp::Plus,
                    expr: Box::new(expr),
                },
            });
        }
        if self.matches_operator(Operator::Minus) {
            let span = self.previous().span.clone();
            let expr = self.parse_unary()?;
            return Ok(Expr {
                span,
                kind: ExprKind::Unary {
                    op: UnaryOp::Minus,
                    expr: Box::new(expr),
                },
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> ParseResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.peek_punct(Punct::LParen) {
                let args = self.parse_optional_expr_list_with_leading_punct()?;
                let span = expr.span.clone();
                expr = Expr {
                    span,
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_optional_expr_list_with_leading_punct(&mut self) -> ParseResult<Vec<Option<Expr>>> {
        self.expect_punct(Punct::LParen)?;
        let args = self.parse_optional_expr_list(Punct::RParen)?;
        self.expect_punct(Punct::RParen)?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> ParseResult<Expr> {
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current("expected expression"))?;
        match token.kind {
            TokenKind::Identifier => {
                let mut segments = vec![token.lexeme];
                while self.matches_operator(Operator::ColonColon) {
                    segments.push(self.expect_identifier("expected path segment")?.lexeme);
                }
                let span = token.span.clone();
                Ok(Expr {
                    span,
                    kind: ExprKind::Path(segments),
                })
            }
            TokenKind::Keyword(_) => {
                let span = token.span.clone();
                Ok(Expr {
                    span,
                    kind: ExprKind::Path(vec![token.lexeme]),
                })
            }
            TokenKind::Integer => Ok(Expr {
                span: token.span,
                kind: ExprKind::Integer(token.lexeme),
            }),
            TokenKind::Real => Ok(Expr {
                span: token.span,
                kind: ExprKind::Real(token.lexeme),
            }),
            TokenKind::ConstZero => Ok(Expr {
                span: token.span,
                kind: ExprKind::Constant(ConstKind::Zero),
            }),
            TokenKind::ConstOne => Ok(Expr {
                span: token.span,
                kind: ExprKind::Constant(ConstKind::One),
            }),
            TokenKind::ConstZ => Ok(Expr {
                span: token.span,
                kind: ExprKind::Constant(ConstKind::Z),
            }),
            TokenKind::ConstX => Ok(Expr {
                span: token.span,
                kind: ExprKind::Constant(ConstKind::X),
            }),
            TokenKind::Punct(Punct::LParen) => {
                let expr = self.parse_expr()?;
                self.expect_punct(Punct::RParen)?;
                Ok(Expr {
                    span: token.span,
                    kind: ExprKind::Group(Box::new(expr)),
                })
            }
            _ => Err(self.error_at(&token, "expected expression")),
        }
    }

    fn parse_binary_chain(
        &mut self,
        next: fn(&mut Self) -> ParseResult<Expr>,
        operators: &[Operator],
        kinds: &[BinaryOp],
    ) -> ParseResult<Expr> {
        let mut expr = next(self)?;
        while let Some(op_index) = self.peek_operator_index(operators) {
            let op = kinds[op_index];
            self.next_token();
            let right = next(self)?;
            let span = expr.span.clone();
            expr = Expr {
                span,
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                },
            };
        }
        Ok(expr)
    }

    fn parse_optional_type_name(&mut self) -> Option<String> {
        if self.matches_any_lexeme(["real", "realtime", "logic", "tri", "wire"]) {
            Some(self.previous().lexeme.clone())
        } else {
            None
        }
    }

    fn parse_name_list(&mut self) -> ParseResult<Vec<String>> {
        let mut names = Vec::new();
        loop {
            names.push(self.expect_identifier("expected name")?.lexeme);
            if self.matches_punct(Punct::Comma) {
                if self.peek_punct(Punct::Semicolon) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(names)
    }

    fn looks_like_strength_group(&self) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        if token.kind != TokenKind::Punct(Punct::LParen) {
            return false;
        }
        let mut idx = self.index + 1;
        let mut saw = false;
        while let Some(token) = self.tokens.get(idx) {
            match &token.kind {
                TokenKind::Identifier => {
                    if !matches!(
                        token.lexeme.as_str(),
                        "strong0"
                            | "strong1"
                            | "pull0"
                            | "pull1"
                            | "weak0"
                            | "weak1"
                            | "highz0"
                            | "highz1"
                            | "supply0"
                            | "supply1"
                    ) {
                        return false;
                    }
                    saw = true;
                    idx += 1;
                }
                TokenKind::Punct(Punct::Comma) => {
                    idx += 1;
                }
                TokenKind::Punct(Punct::RParen) => return saw,
                _ => return false,
            }
        }
        false
    }

    fn peek_next_is_direction_or_rparen(&self) -> bool {
        matches!(
            self.tokens
                .get(self.index + 1)
                .map(|token| token.lexeme.as_str()),
            Some("input" | "output" | "inout")
        ) || self.peek_next_punct(Punct::RParen)
    }

    fn peek_next_is_strength_or_delay(&self) -> bool {
        self.peek_punct(Punct::LParen) && self.looks_like_strength_group()
            || self.peek_next_operator(Operator::Hash)
    }

    fn peek_next_punct(&self, punct: Punct) -> bool {
        self.tokens
            .get(self.index + 1)
            .map(|token| token.kind == TokenKind::Punct(punct))
            .unwrap_or(false)
    }

    fn peek_next_operator(&self, operator: Operator) -> bool {
        self.tokens
            .get(self.index + 1)
            .map(|token| token.kind == TokenKind::Operator(operator))
            .unwrap_or(false)
    }

    fn peek_punct(&self, punct: Punct) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Punct(current)) if *current == punct)
    }

    fn looks_like_instantiation(&self) -> bool {
        if !matches!(self.peek_kind(), Some(TokenKind::Identifier)) {
            return false;
        }
        let mut idx = self.index + 1;
        if matches!(
            self.tokens.get(idx).map(|token| &token.kind),
            Some(TokenKind::Operator(Operator::Hash))
        ) {
            idx += 1;
            if !matches!(
                self.tokens.get(idx).map(|token| &token.kind),
                Some(TokenKind::Punct(Punct::LParen))
            ) {
                return false;
            }
            let mut depth = 1usize;
            idx += 1;
            while let Some(token) = self.tokens.get(idx) {
                match token.kind {
                    TokenKind::Punct(Punct::LParen) => depth += 1,
                    TokenKind::Punct(Punct::RParen) => {
                        depth -= 1;
                        if depth == 0 {
                            idx += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
        }
        matches!(
            self.tokens.get(idx).map(|token| &token.kind),
            Some(TokenKind::Identifier)
        ) && matches!(
            self.tokens.get(idx + 1).map(|token| &token.kind),
            Some(TokenKind::Punct(Punct::LParen))
        )
    }

    fn parse_param_override_list(&mut self) -> ParseResult<Vec<ParamOverride>> {
        let mut parameters = Vec::new();
        let mut expecting_slot = true;
        let mut saw_slot = false;
        let mut last_comma_span = None;

        while !self.peek_punct(Punct::RParen) {
            if self.peek_punct(Punct::Comma) {
                let comma = self.next_token().unwrap();
                if expecting_slot {
                    parameters.push(ParamOverride {
                        span: comma.span.clone(),
                        kind: ParamOverrideKind::Positional(None),
                    });
                }
                expecting_slot = true;
                saw_slot = true;
                last_comma_span = Some(comma.span);
                continue;
            }

            parameters.push(self.parse_param_override()?);
            expecting_slot = false;
            saw_slot = true;
            if self.peek_punct(Punct::RParen) {
                break;
            }
            let comma = self.expect_punct(Punct::Comma)?;
            expecting_slot = true;
            last_comma_span = Some(comma.span);
        }

        if expecting_slot && saw_slot {
            parameters.push(ParamOverride {
                span: last_comma_span.expect("an empty trailing slot follows a comma"),
                kind: ParamOverrideKind::Positional(None),
            });
        }

        Ok(parameters)
    }

    fn parse_param_override(&mut self) -> ParseResult<ParamOverride> {
        if self.matches_operator(Operator::Dot) {
            let span = self.previous().span.clone();
            let name = self.expect_identifier("expected parameter name")?.lexeme;
            self.expect_punct(Punct::LParen)?;
            let value = self.parse_expr()?;
            self.expect_punct(Punct::RParen)?;
            Ok(ParamOverride {
                span,
                kind: ParamOverrideKind::Named { name, value },
            })
        } else {
            let value = self.parse_expr()?;
            Ok(ParamOverride {
                span: value.span.clone(),
                kind: ParamOverrideKind::Positional(Some(value)),
            })
        }
    }

    fn parse_connection_list(&mut self) -> ParseResult<Vec<Connection>> {
        self.expect_punct(Punct::LParen)?;
        let mut connections = Vec::new();
        if !self.peek_punct(Punct::RParen) {
            loop {
                connections.push(self.parse_connection()?);
                if self.matches_punct(Punct::Comma) {
                    if self.peek_punct(Punct::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect_punct(Punct::RParen)?;
        Ok(connections)
    }

    fn parse_connection(&mut self) -> ParseResult<Connection> {
        if self.matches_operator(Operator::Dot) {
            let span = self.previous().span.clone();
            let name = self.expect_identifier("expected connection name")?.lexeme;
            self.expect_punct(Punct::LParen)?;
            let value = self.parse_expr()?;
            self.expect_punct(Punct::RParen)?;
            Ok(Connection {
                span,
                kind: ConnectionKind::Named { name, value },
            })
        } else {
            let value = self.parse_expr()?;
            Ok(Connection {
                span: value.span.clone(),
                kind: ConnectionKind::Positional(value),
            })
        }
    }

    fn peek_next_assignment_operator(&self) -> bool {
        matches!(
            self.tokens.get(self.index + 1).map(|token| &token.kind),
            Some(TokenKind::Operator(Operator::Equals | Operator::LessEqual))
        )
    }

    fn is_terminator(&self) -> bool {
        self.peek_ident("end")
            || self.peek_ident("else")
            || self.peek_ident("endmodule")
            || self.peek_ident("endgenerate")
            || self.peek_ident("endspecify")
    }

    fn peek_ident(&self, text: &str) -> bool {
        matches!(self.peek_token().map(|t| t.lexeme.as_str()), Some(lexeme) if lexeme == text)
    }

    fn matches_any_lexeme<const N: usize>(&mut self, items: [&str; N]) -> bool {
        for item in items {
            if self.peek_ident(item) {
                self.next_token();
                return true;
            }
        }
        false
    }

    fn matches_keyword(&mut self, keyword: Keyword) -> bool {
        if matches!(self.peek_kind(), Some(TokenKind::Keyword(k)) if *k == keyword) {
            self.next_token();
            true
        } else {
            false
        }
    }

    fn matches_operator(&mut self, operator: Operator) -> bool {
        if matches!(self.peek_kind(), Some(TokenKind::Operator(op)) if *op == operator) {
            self.next_token();
            true
        } else {
            false
        }
    }

    fn matches_punct(&mut self, punct: Punct) -> bool {
        if self.peek_punct(punct) {
            self.next_token();
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, keyword: Keyword) -> ParseResult<Token> {
        let message = format!("expected keyword `{:?}`", keyword);
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current(&message))?;
        match token.kind {
            TokenKind::Keyword(k) if k == keyword => Ok(token),
            _ => Err(self.error_at(&token, &message)),
        }
    }

    fn expect_operator(&mut self, operator: Operator) -> ParseResult<Token> {
        let message = format!("expected operator `{:?}`", operator);
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current(&message))?;
        match token.kind {
            TokenKind::Operator(op) if op == operator => Ok(token),
            _ => Err(self.error_at(&token, &message)),
        }
    }

    fn expect_punct(&mut self, punct: Punct) -> ParseResult<Token> {
        let message = format!("expected punctuation `{:?}`", punct);
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current(&message))?;
        match token.kind {
            TokenKind::Punct(p) if p == punct => Ok(token),
            _ => Err(self.error_at(&token, &message)),
        }
    }

    fn expect_identifier(&mut self, message: &str) -> ParseResult<Token> {
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current(message))?;
        match token.kind {
            TokenKind::Identifier => Ok(token),
            _ => Err(self.error_at(&token, message)),
        }
    }

    fn expect_ident_exact(&mut self, text: &str) -> ParseResult<Token> {
        let token = self
            .next_token()
            .ok_or_else(|| self.error_current(&format!("expected `{}`", text)))?;
        if token.lexeme == text {
            Ok(token)
        } else {
            Err(self.error_at(&token, &format!("expected `{}`", text)))
        }
    }

    fn peek_operator_index(&self, operators: &[Operator]) -> Option<usize> {
        let op = match self.peek_kind() {
            Some(TokenKind::Operator(op)) => *op,
            _ => return None,
        };
        operators.iter().position(|candidate| *candidate == op)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek_token().map(|token| &token.kind)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.index - 1]
    }

    fn next_token(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    fn peek_span(&self) -> Span {
        self.peek_token()
            .map(|token| token.span.clone())
            .unwrap_or_else(|| self.eof_span.clone())
    }

    fn is_eof(&self) -> bool {
        self.index >= self.tokens.len()
    }

    fn error_current(&self, message: &str) -> Diagnostic {
        Diagnostic::new(self.peek_span(), message.to_string())
    }

    fn error_at(&self, token: &Token, message: &str) -> Diagnostic {
        Diagnostic::new(token.span.clone(), message.to_string())
    }
}

fn eof_span(path: &Path, input: &str) -> Span {
    let mut line = 1;
    let mut column = 1;
    for byte in input.bytes() {
        if byte == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Span::new(path, line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse_snippet(src: &str) -> Design {
        parse_file(Path::new("snippet.sv"), src).unwrap()
    }

    #[test]
    fn parses_combinational_cell_snippet() {
        let design = parse_snippet(
            "module test(input logic a, b, output logic y); assign y = !(a & b); endmodule",
        );
        let module = design.first_module().unwrap();
        assert_eq!(design.modules().count(), 1);
        assert_eq!(module.ports.len(), 2);
        assert_eq!(module.items.len(), 1);
        assert!(matches!(module.items[0].kind, ItemKind::Assign(_)));
    }

    #[test]
    fn parses_latch_cell_snippet() {
        let design = parse_snippet(
            "module test(input logic en, d, output logic q); always_latch if (en) q <= d; endmodule",
        );
        let item = &design.first_module().unwrap().items[0];
        match &item.kind {
            ItemKind::AlwaysLatch(always) => {
                assert!(always.condition.is_some());
                assert!(matches!(always.body.kind, ItemKind::ProcAssign(_)));
            }
            other => panic!("unexpected item: {:?}", other),
        }
    }

    #[test]
    fn parses_tri_state_and_specify_snippet() {
        let design = parse_snippet(
            "module test(input logic a, output tri logic y); localparam realtime T_Z_y = tpd_z(, T_a); assign (highz1, strong0) #(T_Z_y) y = a ? 0 : 'z; specify specparam T_a = 1; (a *> y) = (T_Z_y); endspecify endmodule",
        );
        assert_eq!(design.first_module().unwrap().items.len(), 3);
    }

    #[test]
    fn parses_transistor_and_generate_snippet() {
        let design = parse_snippet(
            "module test(input logic clk, output logic q); generate if (nodelay) begin always @* q <= clk; end else begin nmos #(T_a) (q, clk, clk); end endgenerate endmodule",
        );
        assert_eq!(design.first_module().unwrap().items.len(), 1);
    }

    #[test]
    fn preserves_ordered_top_level_directives_and_arguments() {
        let source = "`default_nettype none\nmodule test; endmodule\n`default_nettype wire\n";
        let design = parse_snippet(source);
        assert_eq!(design.items.len(), 3);
        match &design.items[0] {
            DesignItem::Directive(directive) => {
                assert_eq!(directive.span, Span::new("snippet.sv", 1, 1));
                assert_eq!(directive.name, "default_nettype");
                assert_eq!(directive.arguments, ["none"]);
            }
            other => panic!("unexpected design item: {other:?}"),
        }
        assert!(matches!(&design.items[1], DesignItem::Module(_)));
        match &design.items[2] {
            DesignItem::Directive(directive) => {
                assert_eq!(directive.span, Span::new("snippet.sv", 3, 1));
                assert_eq!(directive.name, "default_nettype");
                assert_eq!(directive.arguments, ["wire"]);
            }
            other => panic!("unexpected design item: {other:?}"),
        }
    }

    #[test]
    fn rejects_directives_outside_design_scope_at_their_location() {
        let source = "module test;\n  `default_nettype none\nendmodule\n";
        let error = parse_file(Path::new("snippet.sv"), source).unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 2, 3));
        assert_eq!(
            error.message,
            "directives are only supported at design scope"
        );
    }

    #[test]
    fn rejects_unknown_and_malformed_directives_at_their_location() {
        let unknown = parse_file(Path::new("snippet.sv"), "`mystery setting\n").unwrap_err();
        assert_eq!(unknown.span, Span::new("snippet.sv", 1, 1));
        assert_eq!(
            unknown.message,
            "unsupported directive `mystery` at design scope"
        );

        let malformed =
            parse_file(Path::new("snippet.sv"), "`default_nettype banana\n").unwrap_err();
        assert_eq!(malformed.span, Span::new("snippet.sv", 1, 1));
        assert_eq!(
            malformed.message,
            "expected `default_nettype` argument `none` or `wire`"
        );
    }

    #[test]
    fn parameter_overrides_and_connections_have_exact_spans() {
        let source = concat!(
            "module test;\n",
            "  child #(.WIDTH(1), 2, , 4) inst(.a(a), b);\n",
            "endmodule\n",
        );
        let design = parse_snippet(source);
        let line = source.lines().nth(1).unwrap();
        let ItemKind::Instantiation(instance) = &design.first_module().unwrap().items[0].kind
        else {
            panic!("expected instantiation");
        };

        assert_eq!(instance.parameters.len(), 4);
        assert_eq!(
            instance.parameters[0].span,
            Span::new("snippet.sv", 2, line.find(".WIDTH").unwrap() + 1)
        );
        assert!(matches!(
            &instance.parameters[0].kind,
            ParamOverrideKind::Named { .. }
        ));
        assert_eq!(instance.parameters[1].span, Span::new("snippet.sv", 2, 22));
        assert!(matches!(
            &instance.parameters[1].kind,
            ParamOverrideKind::Positional(Some(_))
        ));
        assert_eq!(instance.parameters[2].span, Span::new("snippet.sv", 2, 25));
        assert!(matches!(
            &instance.parameters[2].kind,
            ParamOverrideKind::Positional(None)
        ));
        assert_eq!(instance.parameters[3].span, Span::new("snippet.sv", 2, 27));

        assert_eq!(instance.connections.len(), 2);
        assert_eq!(
            instance.connections[0].span,
            Span::new("snippet.sv", 2, line.find(".a(a)").unwrap() + 1)
        );
        assert!(matches!(
            &instance.connections[0].kind,
            ConnectionKind::Named { .. }
        ));
        assert_eq!(
            instance.connections[1].span,
            Span::new("snippet.sv", 2, line.rfind("b)").unwrap() + 1)
        );
        assert!(matches!(
            &instance.connections[1].kind,
            ConnectionKind::Positional(_)
        ));
    }

    #[test]
    fn sensitivities_and_events_have_exact_spans() {
        let source = concat!(
            "module test;\n",
            "  always @* q = d;\n",
            "  always @(posedge clk, negedge rst_n) q = d;\n",
            "endmodule\n",
        );
        let design = parse_snippet(source);
        let module = design.first_module().unwrap();

        let ItemKind::Always(any) = &module.items[0].kind else {
            panic!("expected always block");
        };
        let any = any.sensitivity.as_ref().unwrap();
        assert_eq!(any.span, Span::new("snippet.sv", 2, 10));
        assert!(matches!(&any.kind, SensitivityKind::Any));

        let ItemKind::Always(list) = &module.items[1].kind else {
            panic!("expected always block");
        };
        let list = list.sensitivity.as_ref().unwrap();
        assert_eq!(list.span, Span::new("snippet.sv", 3, 10));
        let SensitivityKind::List(events) = &list.kind else {
            panic!("expected event-list sensitivity");
        };
        assert_eq!(events[0].span, Span::new("snippet.sv", 3, 12));
        assert_eq!(events[1].span, Span::new("snippet.sv", 3, 25));
    }

    #[test]
    fn design_rendering_is_deterministic_and_complete() {
        let design = parse_snippet(
            "`default_nettype none\nmodule test(input logic a); always @* a = a; endmodule\n",
        );
        let first = render_design(&design);
        let second = render_design(&design);
        assert_eq!(first, second);
        assert!(first.ends_with('\n'));
        assert!(first.contains("Directive("));
        assert!(first.contains("arguments: ["));
        assert!(first.contains("path: \"snippet.sv\""));
        assert!(first.contains("kind: Any"));
    }
}

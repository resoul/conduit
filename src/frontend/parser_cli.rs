use crate::frontend::ast::*;
use crate::frontend::lexer::{Span, TokenKind};
use crate::frontend::parser::{Parser, ParserError};

impl Parser {
    pub(super) fn parse_cli_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'cli'
        // Support both `cli App "desc" { ... }` and `cli group App "desc" { ... }`
        if matches!(&self.peek().kind, TokenKind::Ident(s) if s == "group") {
            self.bump();
        }
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_cli_body(&name, is_exported)?;
        self.expect(&TokenKind::RBrace)?;

        let mut item = Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        );
        item.doc = about;
        Ok(item)
    }

    fn try_take_string(&mut self) -> Option<String> {
        if let TokenKind::String(s) = &self.peek().kind {
            let val = s.clone();
            self.bump();
            Some(val)
        } else {
            None
        }
    }

    fn parse_cli_body(
        &mut self,
        parent_name: &str,
        is_exported: bool,
    ) -> Result<Vec<FieldDef>, ParserError> {
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Expected 'flag', 'opt', 'arg', 'rest', 'cmd', or 'group' in cli block, got '{:?}'",
                        tok.kind
                    ),
                    span: tok.span,
                });
            };
            let field = match kw.as_str() {
                "flag" => self.parse_cli_flag()?,
                "opt" => self.parse_cli_opt()?,
                "arg" => self.parse_cli_arg()?,
                "rest" => self.parse_cli_rest()?,
                "cmd" => self.parse_cli_sub(parent_name, is_exported, false)?,
                "group" => self.parse_cli_sub(parent_name, is_exported, true)?,
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown cli member '{}' (expected flag, opt, arg, rest, cmd, or group)",
                            other
                        ),
                        span: tok.span,
                    });
                }
            };
            fields.push(field);
        }
        Ok(fields)
    }

    fn parse_cli_flag(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let (short, help, default) = self.parse_cli_meta_and_default(true, span)?;
        let default = default.or(Some(Expr {
            kind: ExprKind::Bool(false),
            ty: crate::frontend::types::Type::Void,
            span,
        }));
        let ann = make_cli_ann("flag", short, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty: TypeExpr::Named("Bool".to_string()),
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_opt(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let (short, help, default) = self.parse_cli_meta_and_default(true, span)?;
        let ann = make_cli_ann("opt", short, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_arg(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let (_, help, default) = self.parse_cli_meta_and_default(false, span)?;
        let ann = make_cli_ann("arg", None, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_rest(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) {
            self.parse_type_expr()?
        } else {
            TypeExpr::List(Box::new(TypeExpr::Named("String".to_string())))
        };
        let help = self.try_take_string();
        let default = Some(Expr {
            kind: ExprKind::List(Vec::new()),
            ty: crate::frontend::types::Type::Void,
            span,
        });
        let ann = make_cli_ann("rest", None, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_sub(
        &mut self,
        parent_name: &str,
        is_exported: bool,
        is_group: bool,
    ) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        let sub_struct_name = format!("{}_{}", parent_name, name);
        let sub_fields = if self.match_token(&TokenKind::LBrace) {
            let f = self.parse_cli_body(&sub_struct_name, is_exported)?;
            self.expect(&TokenKind::RBrace)?;
            f
        } else {
            Vec::new()
        };

        let mut sub_item = Item::new_exported(
            ItemKind::Struct {
                name: sub_struct_name.clone(),
                type_params: Vec::new(),
                fields: sub_fields,
            },
            span,
            is_exported,
        );
        sub_item.doc = about.clone();
        self.pending_items.push(sub_item);

        let ann_name = if is_group { "group" } else { "cmd" };
        let mut args = Vec::new();
        if let Some(h) = about {
            args.push(str_expr(h, span));
        }
        let ann = FieldAnnotation {
            target: format!("cli.{}", ann_name),
            args,
            span,
        };
        Ok(FieldDef {
            name,
            json_name: None,
            ty: TypeExpr::Optional(Box::new(TypeExpr::Named(sub_struct_name))),
            annotations: vec![ann],
            default: None,
            span,
        })
    }

    fn parse_cli_meta_and_default(
        &mut self,
        allow_short: bool,
        _span: Span,
    ) -> Result<CliMetaAndDefault, ParserError> {
        let mut short: Option<String> = None;
        let mut help: Option<String> = None;
        let mut default: Option<Expr> = None;

        for _ in 0..3 {
            if self.match_token(&TokenKind::Assign) {
                if default.is_some() {
                    return Err(ParserError {
                        message: "Duplicate default value in cli field".to_string(),
                        span: self.peek().span,
                    });
                }
                default = Some(self.parse_expr()?);
            } else if let Some(s) = self.try_take_string() {
                let normalized_short = s.strip_prefix('-').unwrap_or(&s);
                if allow_short && short.is_none() && normalized_short.chars().count() == 1 {
                    short = Some(normalized_short.to_string());
                } else if help.is_none() {
                    help = Some(s);
                } else {
                    return Err(ParserError {
                        message: "Unexpected extra string in cli field".to_string(),
                        span: self.peek().span,
                    });
                }
            } else {
                break;
            }
        }
        Ok((short, help, default))
    }
}

type CliMetaAndDefault = (Option<String>, Option<String>, Option<Expr>);

fn str_expr(s: String, span: Span) -> Expr {
    Expr {
        kind: ExprKind::String(s),
        ty: crate::frontend::types::Type::Void,
        span,
    }
}

fn make_cli_ann(target: &str, short: Option<String>, help: Option<String>, span: Span) -> FieldAnnotation {
    let mut args = Vec::new();
    match (short, help) {
        (Some(s), Some(h)) => {
            args.push(str_expr(s, span));
            args.push(str_expr(h, span));
        }
        (Some(s), None) => args.push(str_expr(s, span)),
        (None, Some(h)) => args.push(str_expr(h, span)),
        (None, None) => {}
    }
    FieldAnnotation {
        target: format!("cli.{}", target),
        args,
        span,
    }
}

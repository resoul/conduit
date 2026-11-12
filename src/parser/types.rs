use super::*;

impl Parser {
    pub(super) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParserError> {
        let mut types = vec![self.parse_primary_type()?];
        while self.match_token(&TokenKind::Bar) {
            types.push(self.parse_primary_type()?);
        }
        if types.len() == 1 {
            Ok(types.pop().unwrap())
        } else {
            Ok(TypeExpr::Union(types))
        }
    }

    fn parse_primary_type(&mut self) -> Result<TypeExpr, ParserError> {
        let mut base_type = if self.match_token(&TokenKind::LBracket) {
            let inner = self.parse_type_expr()?;
            self.expect(&TokenKind::RBracket)?;
            TypeExpr::List(Box::new(inner))
        } else if self.match_token(&TokenKind::LParen) {
            let mut types = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                types.push(self.parse_type_expr()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
            if self.match_token(&TokenKind::Arrow) {
                let ret = self.parse_type_expr()?;
                TypeExpr::Func {
                    params: types,
                    return_type: Box::new(ret),
                }
            } else if types.len() == 1 {
                types.pop().unwrap()
            } else {
                TypeExpr::Tuple(types)
            }
        } else {
            let (name, _) = self.expect_ident()?;
            let full_name = if self.match_token(&TokenKind::Dot) {
                let (sub, _) = self.expect_ident()?;
                format!("{}.{}", name, sub)
            } else {
                name
            };
            if self.check(&TokenKind::Lt) {
                let type_args = self.parse_type_arg_list()?;
                TypeExpr::Generic {
                    name: full_name,
                    type_args,
                }
            } else {
                TypeExpr::Named(full_name)
            }
        };

        while self.match_token(&TokenKind::Question) {
            base_type = TypeExpr::Optional(Box::new(base_type));
        }

        Ok(base_type)
    }

}

use conduit::ast::{ItemKind, Program};
use conduit::lexer::{Lexer, Span};
use conduit::parser::{Parser, ParserError};

fn parse(source: &str) -> Result<Program, ParserError> {
    let tokens = Lexer::new(source).tokenize_all().expect("valid tokens");
    Parser::new(tokens).parse_program()
}

#[test]
fn duplicate_conduit_field_names_are_rejected_even_with_distinct_json_names() {
    let source = "type T {\n  x: Int `json:\"a\"`\n  x: Int `json:\"b\"`\n}";
    let error = parse(source).expect_err("duplicate field names must be rejected");
    assert_eq!(error.message, "Duplicate field name 'x'");
    assert_eq!(error.span, Span::new(3, 3));
}

#[test]
fn effective_json_names_must_be_unique() {
    for source in [
        "type T { x: Int `json:\"same\"` y: Int `json:\"same\"` }",
        "type T { x: Int `json:\"y\"` y: Int }",
        "type T { x: Int y: Int `json:\"x\"` }",
        "type T { x: Int `json:\"\"` y: Int `json:\"x\"` }",
        "type T { x: Int `json:\"a\\\"b\"` y: Int `json:\"a\\\"b\"` }",
    ] {
        let error = parse(source).expect_err(source);
        assert!(
            error.message.contains("Duplicate JSON field name"),
            "{error}"
        );
    }
    parse("type A { x: Int `json:\"key\"` } type B { y: Int `json:\"key\"` }")
        .expect("names may be reused in different types");
}

#[test]
fn json_options_are_rejected() {
    for tag in ["json:\"x,omitempty\"", "json:\",omitempty\""] {
        let source = format!("type T {{ x: Int `{tag}` }}");
        let error = parse(&source).expect_err(&source);
        assert!(
            error.message.contains("options are not supported"),
            "{error}"
        );
    }
}

#[test]
fn json_tags_and_validation_annotations_are_independent() {
    let program = parse(
        "type T { name: String `json:\"display_name\"` @validation.min(2) \
         other: String @validation.min(3) `json:\"other_name\"` }",
    )
    .expect("field tags and annotations should parse");
    let ItemKind::Type { fields, .. } = &program.items[0].kind else {
        panic!("expected a type");
    };
    assert_eq!(fields[0].json_name.as_deref(), Some("display_name"));
    assert_eq!(fields[0].annotations[0].target, "validation.min");
    assert_eq!(fields[0].annotations[0].args.len(), 1);
    assert_eq!(fields[1].json_name.as_deref(), Some("other_name"));
    assert_eq!(fields[1].annotations[0].target, "validation.min");
}

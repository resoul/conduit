use std::collections::HashSet;
use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::json::{
    decode_node_to_conduit, encode_conduit_value, parse_json, stringify_json_node,
};
use conduit::reachability::ReachabilityReport;
use conduit::types::{Type, TypeId};
use conduit::value::Value;

fn check_source(source: &str) -> Result<(), String> {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&main, source);
    let root = loader.load_project(&main).map_err(|error| error.message)?;
    TypeChecker::new()
        .check_project(&root, &loader)
        .map_err(|error| error.message)
}

fn run_source(source: &str) -> Result<(), String> {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&main, source);
    let root = loader.load_project(&main).map_err(|error| error.message)?;
    TypeChecker::new()
        .check_project(&root, &loader)
        .map_err(|error| error.message)?;
    Interpreter::new()
        .run_project(&root, &loader)
        .map_err(|error| error.message)
}

#[test]
fn closures_cannot_mutate_captured_variables_but_can_mutate_locals() {
    let cases = [
        r#"
        func main() {
            var value = 1
            change := func() { value = 2 }
        }
        "#,
        r#"
        type Box { value: Int }
        func main() {
            var box = Box(value: 1)
            change := func() { box.value = 2 }
        }
        "#,
        r#"
        func main() {
            var values = [1]
            change := func() { values.append(2) }
        }
        "#,
        r#"
        func main() {
            var values: Map<String, Int> = Map<String, Int>()
            change := func() { values.set("key", 1) }
        }
        "#,
        r#"
        func main() {
            var values: Map<String, Int> = Map<String, Int>()
            change := func() { values.remove("key") }
        }
        "#,
    ];

    for source in cases {
        let error = check_source(source).unwrap_err();
        assert!(error.contains("Cannot mutate captured variable"), "{error}");
    }

    run_source(
        r#"
        func main() {
            change := func() -> Int {
                var values = [1]
                values.append(2)
                return values.len()
            }
            if change() != 2 { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn local_function_variables_shadow_module_functions_in_checking_and_runtime() {
    run_source(
        r#"
        func calculate(value: Int) -> Int { return value + 100 }

        func main() {
            calculate := func(value: Int) -> Int { return value + 1 }
            if calculate(4) != 5 { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();

    run_source(
        r#"
        func identify<T>(value: T) -> T { return value }
        func main() {
            identify := func(value: Int) -> Int { return value + 100 }
            if identify<Int>(4) != 4 { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn generic_union_instantiations_must_match() {
    for union in ["Box<Int> | Box<String>", "Box<Int>? | Box<String>"] {
        let source = format!(
            r#"
        type Box<T> {{ value: T }}
        func consume(value: {union}) {{}}
        "#
        );
        let error = check_source(&source).unwrap_err();
        assert!(
            error.contains("Cannot combine distinct generic instantiations of 'Box'"),
            "{error}"
        );
    }

    let substituted_error = check_source(
        r#"
        type Box<T> { value: T }
        func pick<A, B>(a: A, b: B) -> A | B { return a }
        func main() {
            res := pick(Box(value: 1), Box(value: "a"))
            print(res)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        substituted_error.contains("Cannot combine distinct generic instantiations of 'Box'"),
        "{substituted_error}"
    );
}

#[test]
fn empty_lists_need_a_type_unless_a_context_supplies_one() {
    for declaration in ["xs := []", "var xs = []"] {
        let source = format!("func main() {{ {declaration} }}");
        let error = check_source(&source).unwrap_err();
        assert!(
            error.contains("Cannot infer element type for empty list '[]'"),
            "{error}"
        );
    }

    let tuple_error = check_source("func main() { value := ([],) }").unwrap_err();
    assert!(
        tuple_error.contains("Cannot infer element type for empty list '[]'"),
        "{tuple_error}"
    );

    run_source(
        r#"
        type User { tags: [String] }
        func accept(values: [String]) {}
        func empty_values() -> [Int] { return [] }
        func main() {
            var values: [Int] = []
            values = []
            accept([])
            user := User(tags: [])
            pairs := Map.from<String, Int>([])
            values = empty_values()

            var matrix: [[Int]] = []
            matrix.append([])
            if !matrix.contains([]) { fail := 1 / 0 }

            var groups: Map<String, [Int]> = Map<String, [Int]>()
            groups.set("empty", [])
            nested := [[1, 2], []]
            if nested.len() != 2 { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn interpolation_is_checked_and_escaped_braces_stay_literal() {
    let error = check_source(r#"func main() { value := "{missing_var}" }"#).unwrap_err();
    assert!(error.contains("Unknown variable 'missing_var' in string interpolation"));
    let escaped_backslash =
        check_source(r#"func main() { value := "\\{missing_var}" }"#).unwrap_err();
    assert!(
        escaped_backslash.contains("Unknown variable 'missing_var' in string interpolation"),
        "{escaped_backslash}"
    );

    let bad_field = check_source(
        r#"
        type User { name: String }
        func main() {
            u := User(name: "Alice")
            value := "{u.missing}"
            print(value)
        }
        "#,
    )
    .unwrap_err();
    assert!(bad_field.contains("has no field 'missing'"), "{bad_field}");

    run_source(
        r#"
        type User { name: String }
        func main() {
            literal := "\{literal\}"
            if literal != "{literal}" { fail := 1 / 0 }
            mixed := "{raw\}"
            if mixed != "\{raw\}" { fail := 1 / 0 }
            unclosed := "{foo\{bar"
            if unclosed != "{foo{bar" { fail := 1 / 0 }
            user := User(name: "Alice")
            greeting := "Hello, {user.name}!"
            if greeting != "Hello, Alice!" { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn json_parser_is_utf8_safe_and_limited_to_128_nested_containers() {
    let malformed_unicode = "\"\\u123é\"";
    assert!(parse_json(malformed_unicode).is_err());

    let deeply_nested = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    let error = parse_json(&deeply_nested).unwrap_err();
    assert_eq!(error.1, "Maximum JSON nesting depth (128) exceeded");

    let malformed_program = r#"
        import cdt.json as json
        func expect_decode_error(value: Int | json.DecodeError) {
            switch value {
                case json.DecodeError(_error): return
                case Int(_value): fail := 1 / 0
            }
        }
        func main() {
            expect_decode_error(json.decode<Int>("\"\\u123é\""))
        }
    "#;
    run_source(malformed_program).unwrap();

    let deeply_nested_program = format!(
        r#"
        import cdt.json as json
        func expect_decode_error(value: Int | json.DecodeError) {{
            switch value {{
                case json.DecodeError(_error): return
                case Int(_value): fail := 1 / 0
            }}
        }}
        func main() {{ expect_decode_error(json.decode<Int>("{}")) }}
        "#,
        deeply_nested
    );
    run_source(&deeply_nested_program).unwrap();
}

#[test]
fn json_float_encoding_preserves_float_syntax_on_round_trip() {
    let mut encoded = String::new();
    encode_conduit_value(&Value::Float(3.0), "", &mut encoded, &|_| None).unwrap();
    assert_eq!(encoded, "3.0");

    let node = parse_json(&encoded).unwrap();
    assert_eq!(stringify_json_node(&node), "3.0");
    let decoded = decode_node_to_conduit(
        &node,
        &Type::union(vec![Type::Int, Type::Float]),
        "",
        &|_, _| None,
    )
    .unwrap();
    assert_eq!(decoded, Value::Float(3.0));
}

#[test]
fn coalescing_binds_tighter_than_comparisons_and_return_can_precede_case() {
    run_source(
        r#"
        type A { value: Int }
        type B { value: Int }

        func handle(value: A | B) {
            switch value {
                case A(a): return
                case B(b): return
            }
        }

        func main() {
            var value: Int? = nil
            if value ?? 0 == 0 && value ?? 0 < 1 || false {
                handle(A(value: 1))
            } else {
                fail := 1 / 0
            }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn integer_overflow_and_recursion_return_runtime_errors() {
    let error = run_source(r#"func main() { value := 9223372036854775807 + 1 }"#).unwrap_err();
    assert!(error.contains("Integer overflow"), "{error}");

    let error = run_source(r#"func main() { value := -9223372036854775808 / -1 }"#).unwrap_err();
    assert!(error.contains("Integer overflow"), "{error}");

    let error = run_source(
        r#"
        func recurse() { recurse() }
        func main() { recurse() }
        "#,
    )
    .unwrap_err();
    assert!(
        error.contains("Maximum call stack depth (256) exceeded"),
        "{error}"
    );
}

#[test]
fn float_to_int_methods_reject_out_of_range_values() {
    let error = run_source(r#"func main() { value := 9223372036854775807.to_float().floor() }"#)
        .unwrap_err();
    assert!(
        error.contains("Cannot convert non-finite or out-of-range Float to Int"),
        "{error}"
    );
}

#[test]
fn reachability_and_struct_display_distinguish_paths_and_sort_fields() {
    let report = ReachabilityReport {
        root_module: PathBuf::from("/project/main.cdt"),
        reachable_functions: HashSet::from([(
            PathBuf::from("/project/a/util.cdt"),
            "helper".to_string(),
        )]),
        reachable_native_functions: HashSet::new(),
        reachable_native_methods: HashSet::new(),
        total_functions: 1,
        reachable_functions_count: 1,
        unused_functions_count: 0,
    };
    assert!(!report.is_function_reachable(PathBuf::new().join("b/util.cdt").as_path(), "helper"));

    let mut fields = std::collections::HashMap::new();
    fields.insert("z".to_string(), Value::Int(2));
    fields.insert("a".to_string(), Value::Int(1));
    let value = Value::Struct {
        type_id: TypeId::program("Pair"),
        fields,
    };
    assert_eq!(value.to_string(), "Pair(a: 1, z: 2)");

    run_source(
        r#"
        type Pair { z: Int, a: Int }
        func main() {
            pair := Pair(z: 2, a: 1)
            if "{pair}" != "Pair(a: 1, z: 2)" { fail := 1 / 0 }
        }
        "#,
    )
    .unwrap();
}

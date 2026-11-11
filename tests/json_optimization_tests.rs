use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::loader::ModuleLoader;
use conduit::native::{NativeFuncDef, NativeModule};
use conduit::parser::Parser;
use conduit::types::Type;
use conduit::value::Value;

fn load_project(files: &[(&str, &str)]) -> Result<(ModuleLoader, PathBuf), String> {
    let mut loader = ModuleLoader::new();
    for (path, source) in files {
        loader.with_virtual_file(PathBuf::from(path), *source);
    }
    let root = loader
        .load_project(&PathBuf::from(files[0].0))
        .map_err(|e| e.to_string())?;
    Ok((loader, root))
}

fn run_project(files: &[(&str, &str)]) -> Result<(), String> {
    let (loader, root) = load_project(files)?;
    TypeChecker::new()
        .check_project(&root, &loader)
        .map_err(|e| e.to_string())?;
    Interpreter::new()
        .run_project(&root, &loader)
        .map_err(|e| e.to_string())
}

#[test]
fn cached_schemas_distinguish_module_identity_and_generic_arguments() {
    run_project(&[
        (
            "/virtual/main.cdt",
            r#"
            import cdt.json as json
            import "./left" as left
            import "./right" as right

            type Box<T> { item: T `json:"payload"` }
            func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }

            func main() {
                for i in 0..4 {
                    switch json.decode<Box<left.Item>>("{\"payload\":{\"left_name\":7}}") {
                        case Box(box):
                            assert_true(box.item.value == 7)
                            switch json.encode(box) {
                                case String(s): assert_true(s == "{\"payload\":{\"left_name\":7}}")
                                case json.EncodeError(e): boom := 1 / 0
                            }
                        case json.DecodeError(e): boom := 1 / 0
                    }
                    switch json.decode<Box<right.Item>>("{\"payload\":{\"right_name\":\"ok\"}}") {
                        case Box(box):
                            assert_true(box.item.value == "ok")
                            switch json.encode(box) {
                                case String(s): assert_true(s == "{\"payload\":{\"right_name\":\"ok\"}}")
                                case json.EncodeError(e): boom := 1 / 0
                            }
                        case json.DecodeError(e): boom := 1 / 0
                    }
                    switch json.decode<Box<Int>>("{\"payload\":3}") {
                        case Box(box): assert_true(box.item == 3)
                        case json.DecodeError(e): boom := 1 / 0
                    }
                    switch json.decode<Box<String>>("{\"payload\":\"text\"}") {
                        case Box(box): assert_true(box.item == "text")
                        case json.DecodeError(e): boom := 1 / 0
                    }
                }
            }
            "#,
        ),
        (
            "/virtual/left.cdt",
            "export type Item { value: Int `json:\"left_name\"` }",
        ),
        (
            "/virtual/right.cdt",
            "export type Item { value: String `json:\"right_name\"` }",
        ),
    ])
    .expect("cached schemas must use canonical module identity and concrete generic arguments");
}

#[test]
fn json_calls_preserve_mutable_variables_and_nested_members() {
    run_project(&[(
        "/virtual/main.cdt",
        r#"
        import cdt.json as json
        type Item { name: String `json:"display_name"` values: [Int] }
        type Holder { item: Item raw: String }
        func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }

        func main() {
            var raw = "{\"display_name\":\"first\",\"values\":[1]}"
            var item = Item(name: "first", values: [1])
            var holder = Holder(item: item, raw: raw)
            var snapshot = ""
            switch json.encode(item) {
                case String(s): snapshot = s
                case json.EncodeError(e): boom := 1 / 0
            }
            switch json.encode(holder.item) {
                case String(s): assert_true(s == raw)
                case json.EncodeError(e): boom := 1 / 0
            }
            encoder := json.encode
            switch encoder(holder.item) {
                case String(s): assert_true(s == raw)
                case json.EncodeError(e): boom := 1 / 0
            }
            switch json.decode<Item>(raw) {
                case Item(decoded):
                    raw = "{}"
                    assert_true(decoded.name == "first" && decoded.values.len() == 1)
                case json.DecodeError(e): boom := 1 / 0
            }
            switch json.decode<Item>(holder.raw) {
                case Item(decoded):
                    holder.raw = "{}"
                    assert_true(decoded.name == "first" && decoded.values.len() == 1)
                case json.DecodeError(e): boom := 1 / 0
            }
            item.name = "second"
            item.values.append(2)
            holder.item.name = "nested"
            holder.item.values.append(3)
            assert_true(snapshot == "{\"display_name\":\"first\",\"values\":[1]}")
            assert_true(raw == "{}" && holder.raw == "{}")
            assert_true(item.name == "second" && item.values.len() == 2)
            switch json.encode(holder.item) {
                case String(s): assert_true(s == "{\"display_name\":\"nested\",\"values\":[1,3]}")
                case json.EncodeError(e): boom := 1 / 0
            }
        }
        "#,
    )])
    .expect("JSON calls must leave variables and member inputs available for subsequent mutations");
}

#[test]
fn expression_arguments_are_evaluated_once_and_argument_errors_propagate() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn next_value(_: Vec<Value>) -> Result<Value, Value> {
        Ok(Value::Int(CALLS.fetch_add(1, Ordering::SeqCst) as i64 + 1))
    }
    CALLS.store(0, Ordering::SeqCst);
    let mut probe = NativeModule::new("cdt.time", "time", "Argument evaluation probe");
    probe.add_function(NativeFuncDef {
        name: "next",
        doc: "Return the next integer",
        params: vec![],
        return_type: Type::Int,
        is_validator: false,
        implementation: next_value,
    });
    let (loader, root) = load_project(&[(
        "/virtual/main.cdt",
        r#"
        import cdt.json as json
        import cdt.time as probe
        type Item { value: Int `json:"json_value"` }
        error InputError { message: String }
        func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }
        func make_item() -> Item { return Item(value: probe.next()) }
        func make_raw() -> String { return probe.next().to_string() }
        func encoder() -> (Int) -> String | json.EncodeError {
            order := probe.next()
            if order != 5 { boom := 1 / 0 }
            return json.encode
        }
        func fail_raw() -> String | InputError { return InputError(message: "raw failed") }
        func fail_value() -> Int | InputError { return InputError(message: "value failed") }
        func decode_failed_argument() -> Int | InputError | json.DecodeError {
            return json.decode<Int>(fail_raw()?)?
        }
        func encode_failed_argument() -> String | InputError | json.EncodeError {
            return json.encode(fail_value()?)?
        }
        func main() {
            switch json.encode(probe.next() + 10) {
                case String(s): assert_true(s == "11")
                case json.EncodeError(e): boom := 1 / 0
            }
            switch json.encode(make_item()) {
                case String(s): assert_true(s == "{\"json_value\":2}")
                case json.EncodeError(e): boom := 1 / 0
            }
            switch json.decode<Int>(probe.next().to_string()) {
                case Int(n): assert_true(n == 3)
                case json.DecodeError(e): boom := 1 / 0
            }
            switch json.decode<Int>(make_raw()) {
                case Int(n): assert_true(n == 4)
                case json.DecodeError(e): boom := 1 / 0
            }
            switch encoder()(probe.next()) {
                case String(s): assert_true(s == "6")
                case json.EncodeError(e): boom := 1 / 0
            }
            switch decode_failed_argument() {
                case Int(n): boom := 1 / 0
                case InputError(e): assert_true(e.message == "raw failed")
                case json.DecodeError(e): boom := 1 / 0
            }
            switch encode_failed_argument() {
                case String(s): boom := 1 / 0
                case InputError(e): assert_true(e.message == "value failed")
                case json.EncodeError(e): boom := 1 / 0
            }
        }
        "#,
    )])
    .expect("load expression argument project");
    let mut checker = TypeChecker::new();
    checker.native_registry.register_module(probe.clone());
    checker.check_project(&root, &loader).unwrap();
    let mut interp = Interpreter::new();
    interp.native_registry.register_module(probe);
    interp.run_project(&root, &loader).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 6);

    for source in [
        "import cdt.json as json func main() { switch json.encode(1 / 0) { case String(s): print(s) case json.EncodeError(e): print(e.path) } }",
        "import cdt.json as json func raw() -> String { boom := 1 / 0 return \"0\" } func main() { switch json.decode<Int>(raw()) { case Int(n): print(n) case json.DecodeError(e): print(e.path) } }",
    ] {
        let error = run_project(&[("/virtual/main.cdt", source)]).unwrap_err();
        assert!(error.contains("Division by zero"), "{error}");
    }
}

#[test]
fn cached_tagged_schemas_preserve_paths_optional_fields_and_duplicate_keys() {
    run_project(&[(
        "/virtual/main.cdt",
        r#"
        import cdt.json as json
        type Item { value: Int `json:"a.b"` note: String? `json:"memo"` }
        type Envelope { items: [Item] `json:"entries"` }
        type Callback { callback: () -> Int `json:"a.b"` }
        func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }
        func expect_path(raw: String, path: String) {
            switch json.decode<Envelope>(raw) {
                case Envelope(e): boom := 1 / 0
                case json.DecodeError(e): assert_true(e.path == path)
            }
        }
        func main() {
            for i in 0..4 {
                switch json.decode<Item>("{\"a.b\":\"overwritten\",\"a.b\":7}") {
                    case Item(item):
                        assert_true(item.value == 7 && item.note == nil)
                        switch json.encode(item) {
                            case String(s): assert_true(s == "{\"a.b\":7,\"memo\":null}")
                            case json.EncodeError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }
                expect_path("{\"entries\":[{}]}", "entries[0][\"a.b\"]")
                expect_path("{\"entries\":[{\"a.b\":null}]}", "entries[0][\"a.b\"]")
                expect_path("{\"entries\":[{\"a.b\":\"wrong\"}]}", "entries[0][\"a.b\"]")
                expect_path("{\"entries\":[{\"a.b\":1,\"extra.key\":0}]}", "entries[0][\"extra.key\"]")
                expect_path("{\"entries\":[{\"value\":1}]}", "entries[0].value")
                switch json.encode(Callback(callback: func() -> Int { return 1 })) {
                    case String(s): boom := 1 / 0
                    case json.EncodeError(e): assert_true(e.path == "[\"a.b\"]")
                }
            }
        }
        "#,
    )])
    .expect("schema caching must preserve strict decoding and JSON-name error paths");
}

#[test]
fn reusing_an_interpreter_invalidates_schemas_when_definitions_change() {
    let sources = [
        r#"
        import cdt.json as json
        type Item { value: Int `json:"first"` }
        func main() {
            switch json.decode<Item>("{\"first\":7}") {
                case Item(item):
                    if item.value != 7 { boom := 1 / 0 }
                    switch json.encode(item) {
                        case String(s): if s != "{\"first\":7}" { boom := 1 / 0 }
                        case json.EncodeError(e): boom := 1 / 0
                    }
                case json.DecodeError(e): boom := 1 / 0
            }
        }
        "#,
        r#"
        import cdt.json as json
        type Item { value: String `json:"second"` }
        func main() {
            switch json.decode<Item>("{\"second\":\"ok\"}") {
                case Item(item):
                    if item.value != "ok" { boom := 1 / 0 }
                    switch json.encode(item) {
                        case String(s): if s != "{\"second\":\"ok\"}" { boom := 1 / 0 }
                        case json.EncodeError(e): boom := 1 / 0
                    }
                case json.DecodeError(e): boom := 1 / 0
            }
        }
        "#,
    ];
    let mut project_interp = Interpreter::new();
    let mut program_interp = Interpreter::new();
    for source in sources {
        let (loader, root) = load_project(&[("/virtual/main.cdt", source)]).unwrap();
        TypeChecker::new().check_project(&root, &loader).unwrap();
        project_interp.run_project(&root, &loader).unwrap();

        let program = Parser::new(Lexer::new(source).tokenize_all().unwrap())
            .parse_program()
            .unwrap();
        TypeChecker::new().check_program(&program).unwrap();
        program_interp.run_program(&program).unwrap();
    }
}

#[test]
fn compiled_plans_support_recursive_and_expanding_generic_structures() {
    run_project(&[("/virtual/main.cdt", r#"
        import cdt.json as json
        type Node { value: Int next: Node? }
        type Left { right: Right? }
        type Right { left: Left? }
        type Spiral<T> { value: T next: Spiral<[T]>? }
        func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }
        func main() {
            for i in 0..3 {
                raw_node := "{\"value\":1,\"next\":{\"value\":2,\"next\":null}}"
                switch json.decode<Node>(raw_node) {
                    case Node(node):
                        assert_true(node.value == 1 && (node.next?.value ?? 0) == 2)
                        switch json.encode(node) {
                            case String(s): assert_true(s == raw_node)
                            case json.EncodeError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }
                raw_left := "{\"right\":{\"left\":{\"right\":null}}}"
                switch json.decode<Left>(raw_left) {
                    case Left(left):
                        switch json.encode(left) {
                            case String(s): assert_true(s == raw_left)
                            case json.EncodeError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }
                raw_spiral := "{\"value\":1,\"next\":{\"value\":[2],\"next\":{\"value\":[[3]],\"next\":null}}}"
                switch json.decode<Spiral<Int>>(raw_spiral) {
                    case Spiral(spiral):
                        assert_true(spiral.value == 1)
                        switch json.encode(spiral) {
                            case String(s): assert_true(s == raw_spiral)
                            case json.EncodeError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }
            }
        }
    "#)]).expect("prepared schemas must support cycles without eager infinite expansion");
}

#[test]
fn compiled_plans_handle_both_stack_and_heap_field_slots() {
    for count in [0, 16, 17, 40] {
        let fields = (0..count).map(|i| format!("f{i}: Int ")).collect::<String>();
        let entries = (0..count).map(|i| format!("\\\"f{i}\\\":{i}"))
            .collect::<Vec<_>>().join(",");
        let raw = format!("{{{entries}}}");
        let source = format!(r#"
            import cdt.json as json
            type Wide {{ {fields} }}
            func main() {{
                for i in 0..3 {{
                    switch json.decode<Wide>("{raw}") {{
                        case Wide(value):
                            switch json.encode(value) {{
                                case String(s): if s != "{raw}" {{ boom := 1 / 0 }}
                                case json.EncodeError(e): boom := 1 / 0
                            }}
                        case json.DecodeError(e): boom := 1 / 0
                    }}
                }}
            }}
        "#);
        run_project(&[("/virtual/main.cdt", &source)])
            .unwrap_or_else(|error| panic!("{count} fields: {error}"));
    }
}

#[test]
fn borrowed_decode_keeps_syntax_errors_ahead_of_schema_errors() {
    run_project(&[("/virtual/main.cdt", r#"
        import cdt.json as json
        type Item { value: Int `json:"a.b"` }
        func assert_true(ok: Bool) { if !ok { boom := 1 / 0 } }
        func main() {
            switch json.decode<Item>("{\"unknown\":true,\"a.b\":\"bad\",\"tail\":!}") {
                case Item(item): boom := 1 / 0
                case json.DecodeError(e):
                    assert_true(e.path == "tail")
                    assert_true(e.message == "Unexpected character '!' in JSON input")
            }
            switch json.decode<Item>("{\"unknown\":true,\"a.b\":\"bad\"}") {
                case Item(item): boom := 1 / 0
                case json.DecodeError(e):
                    assert_true(e.path == "unknown" && e.message.contains("Unknown field"))
            }
            switch json.decode<Item>("{\"a.b\":!,\"a.b\":3}") {
                case Item(item): boom := 1 / 0
                case json.DecodeError(e):
                    assert_true(e.path == "[\"a.b\"]")
                    assert_true(e.message == "Unexpected character '!' in JSON input")
            }
        }
    "#)]).expect("parsing must finish before any schema errors or duplicate-key selection");
}

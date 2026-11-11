use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::value::Value;

fn load_and_check(files: &[(&str, &str)]) -> Result<(ModuleLoader, PathBuf), String> {
    let mut loader = ModuleLoader::new();
    for (path, src) in files {
        loader.with_virtual_file(&PathBuf::from(path), *src);
    }
    let root = loader
        .load_project(&PathBuf::from(files[0].0))
        .map_err(|e| e.message)?;
    let mut checker = TypeChecker::new();
    checker
        .check_project(&root, &loader)
        .map_err(|e| e.message)?;
    Ok((loader, root))
}

fn check_single(src: &str) -> Result<(), String> {
    load_and_check(&[("/virtual/main.cdt", src)]).map(|_| ())
}

fn run_project(files: &[(&str, &str)]) -> Result<(), String> {
    let (loader, root) = load_and_check(files)?;
    let mut interp = Interpreter::new();
    interp.run_project(&root, &loader).map_err(|e| e.message)
}

fn run_single(src: &str) -> Result<(), String> {
    run_project(&[("/virtual/main.cdt", src)])
}

#[test]
fn test_criterion_1_float_operations_conversions_and_strict_typing() {
    run_single(
        r#"
        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func main() {
            var x = 3.5
            y := -1.25
            sum := x + y
            diff := x - y
            prod := x * 2.0
            quot := 9.0 / 2.0

            assert_true(sum == 2.25)
            assert_true(diff == 4.75)
            assert_true(prod == 7.0)
            assert_true(quot == 4.5)
            assert_true(0.0 < 0.1 && 2.5 >= 2.5 && -10.5 <= -1.0 && 5.0 > 4.9 && 1.0 != 2.0)

            // Explicit Int <-> Float conversions & rounding
            as_f := 42.to_float()
            assert_true(as_f == 42.0)
            assert_true((3.7).floor() == 3)
            assert_true((3.2).ceil() == 4)
            assert_true((3.6).round() == 4)
            assert_true((-3.7).trunc() == -3)
            assert_true((2.5).to_string() == "2.5")
            assert_true(!(2.5).is_nan() && !(2.5).is_infinite() && (2.5).is_finite())

            // Ensure integer ranges still lex cleanly alongside float literals
            var total = 0
            for i in 1..4 {
                total = total + i
            }
            assert_true(total == 6)
        }
    "#,
    )
    .expect("Float arithmetic, comparisons, and methods should execute cleanly");

    // Division by 0.0 raises runtime error rather than leaking Infinity/NaN
    let div_zero = r#"
        func main() {
            x := 5.0 / 0.0
            print(x)
        }
    "#;
    assert!(run_single(div_zero).unwrap_err().contains("Division by zero"));

    // No implicit mixing of Int and Float
    for bad_src in [
        "func main() { x := 1 + 2.5 }",
        "func main() { x := 1 == 1.0 }",
        "func take_float(f: Float) {}\n func main() { take_float(5) }",
        "func take_int(n: Int) {}\n func main() { take_int(5.0) }",
    ] {
        assert!(check_single(bad_src).is_err(), "Expected type error for: {}", bad_src);
    }
}

#[test]
fn test_criterion_2_float_validators_and_nan_rejection() {
    run_single(
        r#"
        import cdt.validation as validation

        [#validator]
        func min_temp(val: Float, limit: Float) -> Void | validation.Issue {
            guard val >= limit else {
                return validation.Issue(code: "too_cold", message: "below minimum")
            }
            return
        }

        type Measurement {
            temp_c: Float @validation.min(-273.15) @validation.max(100.5)
            delta: Float @min_temp(-0.5)
        }

        func main() {
            good := Measurement(temp_c: -273.15, delta: -0.5)
            switch validation.check(good) {
                case Measurement(m):
                case validation.ValidationError(e):
                    boom := 1 / 0
            }

            bad := Measurement(temp_c: 100.6, delta: -0.6)
            switch validation.check(bad) {
                case Measurement(m):
                    boom := 1 / 0
                case validation.ValidationError(errs):
                    if errs.issues.len() != 2 {
                        boom := 1 / 0
                    }
                    var paths: [String] = []
                    for iss in errs.issues {
                        paths.append(iss.path)
                    }
                    if (paths.get(0) ?? "") != "temp_c" || (paths.get(1) ?? "") != "delta" {
                        boom := 1 / 0
                    }
            }
        }
    "#,
    )
    .expect("Float validation bounds and custom validators should run accurately");

    // Wrong bound type rejected at compile time
    let int_bound_on_float = r#"
        import cdt.validation as validation
        type P { x: Float @validation.min(0) }
        func main() {}
    "#;
    assert!(check_single(int_bound_on_float).is_err());

    let float_bound_on_int = r#"
        import cdt.validation as validation
        type P { x: Int @validation.min(0.5) }
        func main() {}
    "#;
    assert!(check_single(float_bound_on_int).is_err());

    // Direct native NaN / Infinity safety check
    let registry = NativeRegistry::new();
    let val_mod = registry.get_module("cdt.validation").unwrap();
    let min_fn = val_mod.functions.get("min").unwrap();
    let nan_res = (min_fn.implementation)(vec![Value::Float(f64::NAN), Value::Float(0.0)]).unwrap();
    assert!(nan_res.is_error(), "NaN must never pass min validator");
    let inf_res = (min_fn.implementation)(vec![Value::Float(f64::INFINITY), Value::Float(0.0)]).unwrap();
    assert!(inf_res.is_error(), "Infinity must never pass min validator");
}

#[test]
fn test_criterion_3_json_decode_validate_encode_roundtrip() {
    run_project(&[
        (
            "/virtual/main.cdt",
            r#"
            import cdt.json as json
            import cdt.validation as validation
            import "./models" as models

            type Wrapper<T> {
                page: Int
                data: T
            }

            func assert_eq(a: String, b: String) {
                if a != b {
                    boom := 1 / 0
                }
            }

            func pipeline(raw: String) -> String | json.DecodeError | validation.ValidationError | json.EncodeError {
                decoded := json.decode<Wrapper<models.Order>>(raw)?
                checked := validation.check(decoded)?
                return json.encode(checked)?
            }

            func main() {
                raw := "{\"page\": 1, \"data\": {\"id\": 42, \"note\": null, \"total\": 19.5, \"tags\": [\"rush\", \"vip\"], \"attrs\": {\"env\": \"prod\", \"tier\": \"gold\"}, \"coord\": [10, 20]}}"
                res := pipeline(raw)
                switch res {
                    case String(encoded):
                        // Deterministic field order (page, data) & Map insertion order (env, tier)
                        expected := "{\"page\":1,\"data\":{\"id\":42,\"note\":null,\"total\":19.5,\"tags\":[\"rush\",\"vip\"],\"attrs\":{\"env\":\"prod\",\"tier\":\"gold\"},\"coord\":[10,20]}}"
                        assert_eq(encoded, expected)
                    case json.DecodeError(e): boom := 1 / 0
                    case validation.ValidationError(e): boom := 1 / 0
                    case json.EncodeError(e): boom := 1 / 0
                }

                // Omitted optional field 'note' decodes to nil cleanly
                raw_omitted := "{\"page\": 2, \"data\": {\"id\": 7, \"total\": 5, \"tags\": [], \"attrs\": {}, \"coord\": [1, 2]}}"
                switch json.decode<Wrapper<models.Order>>(raw_omitted) {
                    case Wrapper(w):
                        assert_eq((w.data.note ?? "none"), "none")
                        assert_eq(w.data.total.to_string(), "5")
                    case json.DecodeError(e): boom := 1 / 0
                }

                // String escaping round-trip + Unicode escape \u00E9 & surrogate pair \uD83D\uDE00
                esc_raw := "\"line1\\n\\t\\\"hi\\\"\\\\end \\u00E9 \\uD83D\\uDE00\""
                switch json.decode<String>(esc_raw) {
                    case String(s):
                        switch json.encode(s) {
                            case String(js):
                                switch json.decode<String>(js) {
                                    case String(s2): assert_eq(s, s2)
                                    case json.DecodeError(e): boom := 1 / 0
                                }
                            case json.EncodeError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }
            }
        "#,
        ),
        (
            "/virtual/models.cdt",
            r#"
            import cdt.validation as validation

            export type Order {
                id: Int @validation.min(1)
                note: String?
                total: Float @validation.min(0.01)
                tags: [String]
                attrs: Map<String, String>
                coord: (Int, Int)
            }
        "#,
        ),
    ])
    .expect("Typed json.decode -> validation.check -> json.encode round-trip should succeed");
}

#[test]
fn test_criterion_4_json_decode_and_encode_errors_with_exact_paths() {
    run_single(
        r#"
        import cdt.json as json

        type Item {
            name: String
            price: Float
            quantity: Int
        }

        type Profile {
            bio: String
        }

        type Payload {
            profile: Profile
            items: [Item]
        }

        func assert_err_path(res: Payload | json.DecodeError, expected_path: String) {
            switch res {
                case Payload(p):
                    boom := 1 / 0
                case json.DecodeError(e):
                    if e.path != expected_path {
                        boom := 1 / 0
                    }
            }
        }

        func main() {
            // 1. Invalid JSON syntax & non-RFC literals NaN/Infinity
            for bad_raw in ["{broken", "123 trailing", "NaN", "Infinity", "01"] {
                switch json.decode<Int>(bad_raw) {
                    case Int(n): boom := 1 / 0
                    case json.DecodeError(e):
                }
            }

            // 2. Missing required field: items[1].name
            missing := "{\"profile\": {\"bio\": \"ok\"}, \"items\": [{\"name\": \"a\", \"price\": 1.5, \"quantity\": 1}, {\"price\": 2.5, \"quantity\": 2}]}"
            assert_err_path(json.decode<Payload>(missing), "items[1].name")

            // 3. null in non-optional field: items[0].price
            null_req := "{\"profile\": {\"bio\": \"ok\"}, \"items\": [{\"name\": \"a\", \"price\": null, \"quantity\": 1}]}"
            assert_err_path(json.decode<Payload>(null_req), "items[0].price")

            // 4. Unknown extra field: profile.extra
            extra := "{\"profile\": {\"bio\": \"ok\", \"extra\": 123}, \"items\": []}"
            assert_err_path(json.decode<Payload>(extra), "profile.extra")

            // 5. Fractional 19.99 & exponent 1e5 in Int field: items[2].quantity
            frac := "{\"profile\": {\"bio\": \"ok\"}, \"items\": [{\"name\": \"a\", \"price\": 1.0, \"quantity\": 1}, {\"name\": \"b\", \"price\": 2.0, \"quantity\": 2}, {\"name\": \"c\", \"price\": 3.0, \"quantity\": 19.99}]}"
            assert_err_path(json.decode<Payload>(frac), "items[2].quantity")

            exp := "{\"profile\": {\"bio\": \"ok\"}, \"items\": [{\"name\": \"a\", \"price\": 1.0, \"quantity\": 1e2}]}"
            assert_err_path(json.decode<Payload>(exp), "items[0].quantity")

            // 6. i64 overflow in Int field
            overflow := "{\"profile\": {\"bio\": \"ok\"}, \"items\": [{\"name\": \"a\", \"price\": 1.0, \"quantity\": 999999999999999999999}]}"
            assert_err_path(json.decode<Payload>(overflow), "items[0].quantity")

            // Disjoint primitive union decodes cleanly
            switch json.decode<Int | String>("42") {
                case Int(n):
                    if n != 42 { boom := 1 / 0 }
                case String(s): boom := 1 / 0
                case json.DecodeError(e): boom := 1 / 0
            }

            // EncodeError for closure and non-String Map key
            fn := func(x: Int) -> Int { return x }
            switch json.encode(fn) {
                case String(s): boom := 1 / 0
                case json.EncodeError(e):
            }
            int_map := Map.from([(1, "one")])
            switch json.encode(int_map) {
                case String(s): boom := 1 / 0
                case json.EncodeError(e):
                    if e.path != "[1]" {
                        boom := 1 / 0
                    }
            }
        }
    "#,
    )
    .expect("json.DecodeError paths and EncodeError checks should pass");

    // Ambiguous struct union in json.decode is rejected at compile time
    let ambig_union = r#"
        import cdt.json as json
        type Cat { name: String }
        type Dog { name: String }
        func main() {
            res := json.decode<Cat | Dog>("{\"name\": \"Rex\"}")
        }
    "#;
    assert!(check_single(ambig_union)
        .unwrap_err()
        .contains("Ambiguous union type"));
}

#[test]
fn test_criterion_5_decode_and_validation_are_explicitly_separate() {
    run_single(
        r#"
        import cdt.json as json
        import cdt.validation as validation

        type User {
            email: String @validation.email
        }

        func main() {
            // Structurally valid JSON even though @validation.email fails
            raw := "{\"email\": \"not-an-email\"}"
            switch json.decode<User>(raw) {
                case User(u):
                    switch validation.check(u) {
                        case User(valid):
                            boom := 1 / 0
                        case validation.ValidationError(errs):
                            var path = ""
                            for iss in errs.issues {
                                path = iss.path
                            }
                            if errs.issues.len() != 1 || path != "email" {
                                boom := 1 / 0
                            }
                    }
                case json.DecodeError(e):
                    boom := 1 / 0
            }
        }
    "#,
    )
    .expect("json.decode must succeed independently before validation.check catches @ rules");
}

#[test]
fn test_criterion_6_unstructured_json_value_parse_and_stringify() {
    run_single(
        r#"
        import cdt.json as json

        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func read_port(raw: String) -> Int | json.DecodeError {
            root := json.parse(raw)?
            obj := root.as_map()
            if obj == nil {
                return -1
            }
            switch obj {
                case Map(m):
                    srv_node := m.get("server")
                    switch srv_node {
                        case json.Value(srv):
                            srv_map := srv.as_map()
                            switch srv_map {
                                case Map(sm):
                                    port_node := sm.get("port")
                                    switch port_node {
                                        case json.Value(pn):
                                            return pn.as_int() ?? -1
                                        case nil:
                                            return -1
                                    }
                                case nil:
                                    return -1
                            }
                        case nil:
                            return -1
                    }
                case nil:
                    return -1
            }
        }

        func main() {
            raw := "{\"server\": {\"port\": 8080, \"tls\": true, \"rate\": 1.5, \"tag\": \"v1\", \"extra\": null}, \"nums\": [1, 2.5]}"
            switch read_port(raw) {
                case Int(p): assert_true(p == 8080)
                case json.DecodeError(e): boom := 1 / 0
            }

            switch json.parse(raw) {
                case json.Value(root):
                    switch root.as_map() {
                        case Map(m):
                            switch m.get("nums") {
                                case json.Value(nums_v):
                                    list := nums_v.as_list() ?? []
                                    assert_true(list.len() == 2)
                                    n0 := list.get(0)
                                    n1 := list.get(1)
                                    switch n0 {
                                        case json.Value(v0):
                                            assert_true((v0.as_int() ?? 0) == 1)
                                            assert_true((v0.as_float() ?? 0.0) == 1.0)
                                        case nil: boom := 1 / 0
                                    }
                                    switch n1 {
                                        case json.Value(v1):
                                            assert_true(v1.as_int() == nil)
                                            assert_true((v1.as_float() ?? 0.0) == 2.5)
                                        case nil: boom := 1 / 0
                                    }
                                case nil: boom := 1 / 0
                            }
                            switch m.get("server") {
                                case json.Value(sv):
                                    switch sv.as_map() {
                                        case Map(sm):
                                            switch sm.get("tls") {
                                                case json.Value(tv): assert_true((tv.as_bool() ?? false) == true)
                                                case nil: boom := 1 / 0
                                            }
                                            switch sm.get("tag") {
                                                case json.Value(st): assert_true((st.as_string() ?? "") == "v1")
                                                case nil: boom := 1 / 0
                                            }
                                            switch sm.get("extra") {
                                                case json.Value(nv): assert_true(nv.is_null())
                                                case nil: boom := 1 / 0
                                            }
                                        case nil: boom := 1 / 0
                                    }
                                case nil: boom := 1 / 0
                            }
                        case nil: boom := 1 / 0
                    }
                    re := json.stringify(root)
                    assert_true(re == "{\"server\":{\"port\":8080,\"tls\":true,\"rate\":1.5,\"tag\":\"v1\",\"extra\":null},\"nums\":[1,2.5]}")
                case json.DecodeError(e):
                    boom := 1 / 0
            }
        }
    "#,
    )
    .expect("json.parse, json.Value accessors, and json.stringify should work cleanly");
}

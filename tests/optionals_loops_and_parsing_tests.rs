use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::reachability::ReachabilityAnalyzer;

fn load_and_check(files: &[(&str, &str)]) -> Result<(ModuleLoader, PathBuf), String> {
    let mut loader = ModuleLoader::new();
    for (path, src) in files {
        loader.with_virtual_file(PathBuf::from(path), *src);
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

fn run_single(src: &str) -> Result<(), String> {
    let (loader, root) = load_and_check(&[("/virtual/main.cdt", src)])?;
    let mut interp = Interpreter::new();
    interp.run_project(&root, &loader).map_err(|e| e.message)
}

#[test]
fn test_criterion_1_optional_chaining_and_short_circuiting() {
    run_single(
        r#"
        import cdt.json as json

        type City {
            name: String
        }
        type Address {
            city: City?
        }
        type Profile {
            address: Address?
            tag: String?
        }

        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func main() {
            p1 := Profile(address: Address(city: City(name: "Kyiv")), tag: "primary")
            p2 := Profile(address: Address(city: nil), tag: nil)
            p3 := nil

            // Chained field and method access flattens to U?, never U??
            c1 := p1.address?.city?.name
            c1_upper := p1.address?.city?.name?.upper()
            c2 := p2.address?.city?.name?.upper()
            var maybe_p: Profile? = p3
            c3 := maybe_p?.address?.city?.name

            assert_true((c1 ?? "") == "Kyiv")
            assert_true((c1_upper ?? "") == "KYIV")
            assert_true(c2 == nil)
            assert_true(c3 == nil)

            // Json Value chaining
            raw := "{\"server\": {\"port\": 8080, \"host\": \"localhost\"}}"
            parsed := json.parse(raw)
            switch parsed {
                case json.Value(root):
                    port := root.as_map()?.get("server")?.as_map()?.get("port")?.as_int() ?? -1
                    missing_port := root.as_map()?.get("missing")?.as_map()?.get("port")?.as_int() ?? -1
                    assert_true(port == 8080)
                    assert_true(missing_port == -1)
                case json.DecodeError(err):
                    boom := 1 / 0
            }

            // Short-circuit: side effects / errors in arguments and chain tail do not run when target is nil
            fail_if_called := func() -> String {
                boom := 1 / 0
                return "unreachable"
            }
            var nil_str: String? = nil
            res := nil_str?.replace("a", fail_if_called())
            assert_true(res == nil)

            var some_str: String? = "ab"
            res2 := some_str?.replace("a", "c")
            assert_true((res2 ?? "") == "cb")
        }
        "#,
    )
    .unwrap();

    // Reject ?. on non-optional types
    let err_non_opt = check_single(
        r#"
        type User { name: String }
        func main() {
            u := User(name: "A")
            n := u?.name
            print(n)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_non_opt.contains("Operator '?.' requires an optional type 'T?'"),
        "Unexpected error: {}",
        err_non_opt
    );

    // Reject ?. with Void-returning mutating method
    let err_void = check_single(
        r#"
        func main() {
            var xs: [Int]? = [1, 2]
            xs?.append(3)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_void.contains("Void-returning"),
        "Unexpected error: {}",
        err_void
    );

    // Reject ?. with error-union method
    let err_union = check_single(
        r#"
        func main() {
            var s: String? = "42"
            x := s?.to_int()
            print(x)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_union.contains("error-returning method"),
        "Unexpected error: {}",
        err_union
    );
}

#[test]
fn test_criterion_2_flow_sensitive_type_narrowing() {
    run_single(
        r#"
        type Profile {
            score: Int
            label: String
        }

        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func check_guard(p: Profile?) -> Int {
            guard p != nil else {
                return -1
            }
            return p.score + p.label.len()
        }

        func opt_profile(ok: Bool) -> Profile? {
            if ok {
                return Profile(score: 10, label: "abc")
            }
            return nil
        }

        func main() {
            assert_true(check_guard(nil) == -1)
            assert_true(check_guard(Profile(score: 40, label: "hi")) == 42)

            // if x != nil and if x == nil else narrowing
            maybe_p := opt_profile(true)
            if maybe_p != nil {
                assert_true(maybe_p.score == 10)
            } else {
                boom := 1 / 0
            }

            var other: Profile? = nil
            if other == nil {
                other = Profile(score: 7, label: "ok")
            } else {
                boom := 1 / 0
            }

            // Reassignment resets narrowing cleanly
            if other != nil {
                assert_true(other.score == 7)
                other = nil
                assert_true((other?.score ?? -99) == -99)
            } else {
                boom := 1 / 0
            }

            // Short-circuit narrowing with && and ||
            var p3: Profile? = Profile(score: 25, label: "yes")
            if p3 != nil && p3.score > 20 && p3.label == "yes" {
                assert_true(p3.score == 25)
            } else {
                boom := 1 / 0
            }

            var p4: Profile? = nil
            if p4 == nil || p4.score == 0 {
                assert_true((p4?.score ?? 0) == 0)
            } else {
                boom := 1 / 0
            }

            // while loop narrowing + mutation to nil in body
            var cur: Profile? = Profile(score: 3, label: "loop")
            var sum = 0
            while cur != nil {
                sum = sum + cur.score
                cur = nil
            }
            assert_true(sum == 3)
            assert_true(cur == nil)

            // guard in loop with break/continue
            items := [opt_profile(false), Profile(score: 5, label: "a"), nil, Profile(score: 8, label: "b")]
            var total = 0
            for it in items {
                guard it != nil else {
                    continue
                }
                total = total + it.score
            }
            assert_true(total == 13)
        }
        "#,
    )
    .unwrap();

    // After resetting var x = nil, narrowed field access must fail typechecking
    let err_widened = check_single(
        r#"
        type Box { v: Int }
        func main() {
            var b: Box? = Box(v: 1)
            if b != nil {
                b = nil
                x := b.v
                print(x)
            }
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_widened.contains("has no field 'v'") || err_widened.contains("without narrowing"),
        "Unexpected error: {}",
        err_widened
    );
}

#[test]
fn test_criterion_3_loops_map_iteration_destructuring_and_while() {
    run_single(
        r#"
        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func main() {
            // Map insertion-order iteration via (k, v) and single entry tuple
            m := Map.from([("b", 2), ("a", 1), ("c", 3)])
            var keys: [String] = []
            var sum = 0
            for (k, v) in m {
                keys.append(k)
                sum = sum + v
            }
            assert_true(keys.join(",") == "b,a,c")
            assert_true(sum == 6)

            var pairs_text: [String] = []
            for entry in m {
                (ek, ev) := entry
                pairs_text.append("{ek}:{ev}")
            }
            assert_true(pairs_text.join("|") == "b:2|a:1|c:3")

            // for (idx, item) in list.enumerate()
            var enum_out: [String] = []
            for (idx, word) in ["alpha", "beta", "gamma"].enumerate() {
                enum_out.append("{idx}={word}")
            }
            assert_true(enum_out.join(" ") == "0=alpha 1=beta 2=gamma")

            // Wildcard skipping non-error values in tuple loop pattern
            var only_vals = 0
            for (_, v) in m {
                only_vals = only_vals + v
            }
            assert_true(only_vals == 6)

            // while with break and continue
            var i = 0
            var acc = 0
            while i < 10 {
                i = i + 1
                if i % 2 == 0 {
                    continue
                }
                if i > 7 {
                    break
                }
                acc = acc + i
            }
            assert_true(acc == 1 + 3 + 5 + 7)
        }
        "#,
    )
    .unwrap();

    // Wildcard cannot silently discard error-union component in for pattern
    let err_discard = check_single(
        r#"
        error Boom { msg: String }
        func get_pairs() -> [(Int, String | Boom)] {
            return [(1, "ok")]
        }
        func main() {
            for (id, _) in get_pairs() {
                print(id)
            }
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_discard.contains("Cannot discard potential error type"),
        "Unexpected error: {}",
        err_discard
    );

    // while requires strictly Bool condition
    let err_non_bool = check_single(
        r#"
        func main() {
            while 1 {
                break
            }
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_non_bool.contains("must be Bool"),
        "Unexpected error: {}",
        err_non_bool
    );

    // break inside closure cannot escape outer while loop
    let err_closure_break = check_single(
        r#"
        func main() {
            while true {
                f := func() {
                    break
                }
                f()
                break
            }
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_closure_break.contains("'break' outside of loop"),
        "Unexpected error: {}",
        err_closure_break
    );
}

#[test]
fn test_criterion_4_index_assignment_nested_mutation_and_immutability() {
    run_single(
        r#"
        type Meta {
            updated: Bool
        }

        type Order {
            total: Float
            meta: Meta
            items: [String]
            tags: Map<String, String>
        }

        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func main() {
            // Map m[k] = v inserts and overwrites preserving key order
            var scores = Map.from([("alice", 10), ("bob", 20)])
            scores["alice"] = 15
            scores["carol"] = 30
            assert_true(scores.keys().join(",") == "alice,bob,carol")
            assert_true((scores.get("alice") ?? 0) == 15)
            assert_true((scores.get("carol") ?? 0) == 30)

            // Nested struct field + collection mutations on var root
            var order = Order(
                total: 10.0,
                meta: Meta(updated: false),
                items: ["pen"],
                tags: Map.from([("tier", "basic")])
            )
            order.total = 99.5
            order.meta.updated = true
            order.items.append("notebook")
            order.tags.set("env", "prod")
            order.tags["tier"] = "pro"

            assert_true(order.total == 99.5)
            assert_true(order.meta.updated)
            assert_true(order.items.join(",") == "pen,notebook")
            assert_true((order.tags.get("env") ?? "") == "prod")
            assert_true((order.tags.get("tier") ?? "") == "pro")
        }
        "#,
    )
    .unwrap();

    // Immutable map index assignment rejected
    let err_imm_map = check_single(
        r#"
        func main() {
            m := Map.from([("k", 1)])
            m["k"] = 2
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_imm_map.contains("Cannot mutate immutable variable 'm'"),
        "Unexpected error: {}",
        err_imm_map
    );

    // Immutable struct field assignment rejected
    let err_imm_field = check_single(
        r#"
        type Box { val: Int }
        func main() {
            order := Box(val: 1)
            order.val = 2
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_imm_field.contains("Cannot mutate immutable variable 'order'"),
        "Unexpected error: {}",
        err_imm_field
    );

    // Mutating collection method on field of immutable binding rejected
    let err_imm_method = check_single(
        r#"
        type Bag { items: [Int] }
        func main() {
            order := Bag(items: [1])
            order.items.append(2)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_imm_method.contains("Cannot mutate immutable variable 'order'"),
        "Unexpected error: {}",
        err_imm_method
    );

    // List index assignment remains unsupported
    let err_list_idx = check_single(
        r#"
        func main() {
            var xs = [1, 2, 3]
            xs[0] = 9
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_list_idx.contains("not supported"),
        "Unexpected error: {}",
        err_list_idx
    );
}

#[test]
fn test_criterion_5_list_and_string_methods_unicode() {
    run_single(
        r#"
        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func main() {
            nums := [10, 20, 30, 40]
            assert_true(nums.slice(1, 3).len() == 2)
            assert_true((nums.slice(1, 3).get(0) ?? -1) == 20)
            assert_true((nums.slice(1, 3).get(1) ?? -1) == 30)
            assert_true(nums.slice(-5, 2).len() == 2)
            assert_true(nums.slice(3, 1).len() == 0)
            assert_true(nums.slice(2, 99).len() == 2)

            rev := nums.reverse()
            assert_true((rev.get(0) ?? -1) == 40)
            assert_true((rev.get(3) ?? -1) == 10)
            // reverse does not mutate original list
            assert_true((nums.get(0) ?? -1) == 10)

            assert_true(["a", "b", "c"].join("-") == "a-b-c")
            var empty_strs: [String] = []
            assert_true(empty_strs.join(",") == "")

            // Unicode scalar indexing and slicing
            text := "Привет 🚀 мир!"
            idx_rocket := text.index_of("🚀")
            idx_world := text.index_of("мир")
            idx_missing := text.index_of("xyz")
            assert_true((idx_rocket ?? -1) == 7)
            assert_true((idx_world ?? -1) == 9)
            assert_true(idx_missing == nil)
            assert_true(text.slice(0, 6) == "Привет")
            assert_true(text.slice(7, 8) == "🚀")
            assert_true(text.slice(9, 100) == "мир!")
            assert_true(text.slice(5, 2) == "")
            assert_true(text.slice(-10, 2) == "Пр")
        }
        "#,
    )
    .unwrap();

    // join rejected on non-[String] list
    let err_join = check_single(
        r#"
        func main() {
            s := [1, 2, 3].join(",")
            print(s)
        }
        "#,
    )
    .unwrap_err();
    assert!(
        err_join.contains("[String]"),
        "Unexpected error: {}",
        err_join
    );
}

#[test]
fn test_criterion_6_string_number_parsing_and_parse_error() {
    run_single(
        r#"
        func assert_true(ok: Bool) {
            if !ok {
                boom := 1 / 0
            }
        }

        func parse_sum(a: String, b: String) -> Int | ParseError {
            x := a.to_int()?
            y := b.to_int()?
            return x + y
        }

        func expect_int_err(raw: String) {
            switch raw.to_int() {
                case Int(v):
                    boom := 1 / 0
                case ParseError(e):
                    assert_true(e.value == raw)
                    assert_true(e.message.len() > 0)
            }
        }

        func expect_float_err(raw: String) {
            switch raw.to_float() {
                case Float(v):
                    boom := 1 / 0
                case ParseError(e):
                    assert_true(e.value == raw)
                    assert_true(e.message.len() > 0)
            }
        }

        func main() {
            assert_true((parse_sum("40", "-2") ?? 0) == 38)

            switch " 42 ".trim().to_int() {
                case Int(v):
                    assert_true(v == 42)
                case ParseError(e):
                    boom := 1 / 0
            }

            switch "-3.5".to_float() {
                case Float(v):
                    assert_true(v == -3.5)
                case ParseError(e):
                    boom := 1 / 0
            }

            switch "1.5e2".to_float() {
                case Float(v):
                    assert_true(v == 150.0)
                case ParseError(e):
                    boom := 1 / 0
            }

            // Reject whitespace without trim, floats in to_int, overflow, empty, and non-finite floats
            expect_int_err("")
            expect_int_err(" 42")
            expect_int_err("12.34")
            expect_int_err("9223372036854775808")
            expect_int_err("abc")

            expect_float_err("")
            expect_float_err(" 1.5")
            expect_float_err("NaN")
            expect_float_err("inf")
            expect_float_err("Infinity")
            expect_float_err("-Infinity")
            expect_float_err("not-a-float")
        }
        "#,
    )
    .unwrap();
}

#[test]
fn test_reachability_handles_while_and_optional_chaining() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(
        &main,
        r#"
        type Node { val: Int }
        func helper() -> Node? { return Node(val: 1) }
        func unused_fn() -> Int { return 99 }

        func main() {
            var count = 0
            while count < 1 {
                v := helper()?.val ?? 0
                count = count + v
            }
        }
        "#,
    );
    let root = loader.load_project(&main).unwrap();
    let registry = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &registry);
    let report = analyzer.analyze(&root, &[]);
    assert!(!report.is_function_reachable(&root, "unused_fn"));
    assert!(report.is_function_reachable(&root, "helper"));
}

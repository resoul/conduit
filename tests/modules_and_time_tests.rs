use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration as StdDuration, UNIX_EPOCH};

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::time::Timestamp;
use conduit::native::NativeRegistry;
use conduit::reachability::ReachabilityAnalyzer;
use conduit::value::Value;

// =========================================================================
// Criterion 1: Export and privacy of functions, types, and errors
// =========================================================================
#[test]
fn test_criterion_1_export_privacy_and_case_independence() {
    let mut loader = ModuleLoader::new();
    let helper = PathBuf::from("/criterion1/helper.cdt");
    let main = PathBuf::from("/criterion1/main.cdt");

    loader.with_virtual_file(
        &helper,
        r#"
        // Private by default regardless of casing
        func PrivateHelper() -> Int {
            return 10
        }
        func lowercase_private() -> Int {
            return 20
        }

        // Explicitly exported
        export func PublicFunc() -> Int {
            return PrivateHelper() + lowercase_private()
        }

        export type PublicUser {
            id: Int
        }

        type PrivateUser {
            id: Int
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./helper" as helper

        func main() {
            x := helper.PublicFunc()
            u := helper.PublicUser(id: x)
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());

    let mut bad_loader = ModuleLoader::new();
    bad_loader.with_virtual_file(
        &helper,
        r#"
        func private_fn() -> Int { return 1 }
    "#,
    );
    bad_loader.with_virtual_file(
        &main,
        r#"
        import "./helper" as helper
        func main() {
            helper.private_fn()
        }
    "#,
    );
    let bad_root = bad_loader.load_project(&main).unwrap();
    let mut bad_checker = TypeChecker::new();
    let err = bad_checker.check_project(&bad_root, &bad_loader).unwrap_err();
    assert!(err.to_string().contains("is private in module 'helper'"));
}

// =========================================================================
// Criterion 2: Prohibition of leaking private types
// =========================================================================
#[test]
fn test_criterion_2_private_type_leakage_prohibition() {
    let mut loader = ModuleLoader::new();
    let lib = PathBuf::from("/criterion2/lib.cdt");

    // Leakage in return type
    loader.with_virtual_file(
        &lib,
        r#"
        type InternalData { val: Int }
        export func get_data() -> InternalData {
            return InternalData(val: 1)
        }
    "#,
    );
    let root = loader.load_project(&lib).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root, &loader).unwrap_err();
    assert!(err.to_string().contains("exposes private type 'InternalData'"));

    let mut loader2 = ModuleLoader::new();
    loader2.with_virtual_file(
        &lib,
        r#"
        type Secret { key: String }
        export func use_secret(s: Secret) {}
    "#,
    );
    let root2 = loader2.load_project(&lib).unwrap();
    let mut checker2 = TypeChecker::new();
    let err2 = checker2.check_project(&root2, &loader2).unwrap_err();
    assert!(err2.to_string().contains("exposes private type 'Secret'"));

    let mut loader3 = ModuleLoader::new();
    loader3.with_virtual_file(
        &lib,
        r#"
        type Hidden { code: Int }
        export type PublicWrapper {
            inner: Hidden
        }
    "#,
    );
    let root3 = loader3.load_project(&lib).unwrap();
    let mut checker3 = TypeChecker::new();
    let err3 = checker3.check_project(&root3, &loader3).unwrap_err();
    assert!(err3.to_string().contains("exposes private type 'Hidden'"));
}

// =========================================================================
// Criterion 3: Imports, aliases, relative paths, duplicate loading, cycles
// =========================================================================
#[test]
fn test_criterion_3_imports_aliases_and_cycles() {
    let mut loader = ModuleLoader::new();
    let a = PathBuf::from("/criterion3/a.cdt");
    let b = PathBuf::from("/criterion3/b.cdt");

    loader.with_virtual_file(
        &a,
        r#"
        import "./b" as b
        export func fa() {}
    "#,
    );
    loader.with_virtual_file(
        &b,
        r#"
        import "./a" as a
        export func fb() {}
    "#,
    );

    let err = loader.load_project(&a).unwrap_err();
    assert!(err.to_string().contains("Cyclic import detected"));

    let mut loader2 = ModuleLoader::new();
    let main = PathBuf::from("/criterion3/main.cdt");
    loader2.with_virtual_file(
        &main,
        r#"
        import cdt.time as clock

        func main() {
            t := clock.monotonic()
        }
    "#,
    );
    let root2 = loader2.load_project(&main).unwrap();
    let mut checker2 = TypeChecker::new();
    assert!(checker2.check_project(&root2, &loader2).is_ok());
}

// =========================================================================
// Criterion 4: Qualified types, constructors, and switch pattern matching
// =========================================================================
#[test]
fn test_criterion_4_qualified_types_constructors_patterns() {
    let mut loader = ModuleLoader::new();
    let users = PathBuf::from("/criterion4/users.cdt");
    let main = PathBuf::from("/criterion4/main.cdt");

    loader.with_virtual_file(
        &users,
        r#"
        export type User {
            id: Int
            name: String
        }

        export error UserNotFound {
            id: Int
        }

        export func find(id: Int) -> User | UserNotFound {
            guard id > 0 else {
                return UserNotFound(id: id)
            }
            return User(id: id, name: "Alice")
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./users" as users

        func main() {
            res := users.find(1)
            switch res {
                case users.User(u):
                    print(u.name)
                case users.UserNotFound(e):
                    print(e.id)
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

// =========================================================================
// Criterion 5: Imported and native functions as first-class values and callbacks
// =========================================================================
#[test]
fn test_criterion_5_imported_and_native_first_class_functions_callbacks() {
    let mut loader = ModuleLoader::new();
    let math_lib = PathBuf::from("/criterion5/math.cdt");
    let main = PathBuf::from("/criterion5/main.cdt");

    loader.with_virtual_file(
        &math_lib,
        r#"
        export func double(x: Int) -> Int {
            return x * 2
        }

        export func is_positive(x: Int) -> Bool {
            return x > 0
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./math" as math

        func main() {
            // Function value assignment
            op := math.double
            val := op(21)
            print(val)

            // As map and filter callbacks
            nums := [-2, 0, 5]
            positives := filter(nums, math.is_positive)
            doubled := map(positives, math.double)
            print(doubled)
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

// =========================================================================
// Criterion 6: Time signatures & prohibition of directly constructing native types
// =========================================================================
#[test]
fn test_criterion_6_time_signatures_and_opaque_type_construction_prohibited() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/criterion6/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.time

        func main() {
            // Directly constructing opaque Timestamp via struct syntax must be rejected
            t := time.Timestamp()
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root, &loader).unwrap_err();
    assert!(err.to_string().contains("cannot be constructed directly"));
}

// =========================================================================
// Criterion 7: Handling TimeError via switch and propagation via '?'
// =========================================================================
#[test]
fn test_criterion_7_time_error_handling_switch_and_try() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/criterion7/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.time

        func get_unix_now() -> Int | time.TimeError {
            now := time.now()?
            secs := now.unix_seconds()?
            return secs
        }

        func main() {
            res := get_unix_now()
            switch res {
                case Int(s):
                    print(s)
                case time.TimeError(e):
                    print(e.message)
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

// =========================================================================
// Criterion 8: Duration constructors, negative arguments, unit conversions
// =========================================================================
#[test]
fn test_criterion_8_duration_constructors_negative_units() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/criterion8/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.time

        func run_durations() -> (Int, Int) | time.TimeError {
            d1 := time.seconds(5)?
            d2 := time.milliseconds(500)?

            s := d1.seconds()?
            ms := d2.milliseconds()?
            return (s, ms)
        }

        func main() {
            switch run_durations() {
                case (s, ms):
                    print(s)
                    print(ms)
                case time.TimeError(e):
                    print(e.message)
            }

            // Negative duration returns TimeError
            neg := time.seconds(-10)
            switch neg {
                case time.Duration(d):
                    print("unexpected")
                case time.TimeError(e):
                    print(e.message)
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

// =========================================================================
// Criterion 9: Unix timestamp before/after epoch and negative flooring
// =========================================================================
#[test]
fn test_criterion_9_unix_timestamp_negative_flooring() {
    use conduit::native::NativeValue;

    let registry = NativeRegistry::new();
    let time_mod = registry.get_module("time").unwrap();

    let unix_sec_method = time_mod
        .methods
        .get(&("Timestamp", "unix_seconds"))
        .unwrap();

    // 1. Exactly at epoch
    let ts_epoch: Arc<dyn NativeValue> = Arc::new(Timestamp(UNIX_EPOCH));
    let val_epoch = (unix_sec_method.implementation)(&ts_epoch, vec![]).unwrap();
    assert_eq!(val_epoch, Value::Int(0));

    // 2. 500ms after epoch -> 0 whole seconds
    let ts_after: Arc<dyn NativeValue> = Arc::new(Timestamp(UNIX_EPOCH + StdDuration::from_millis(500)));
    let val_after = (unix_sec_method.implementation)(&ts_after, vec![]).unwrap();
    assert_eq!(val_after, Value::Int(0));

    // 3. 500ms BEFORE epoch -> floored towards -infinity -> -1 second!
    let ts_before_half: Arc<dyn NativeValue> = Arc::new(Timestamp(UNIX_EPOCH - StdDuration::from_millis(500)));
    let val_before_half = (unix_sec_method.implementation)(&ts_before_half, vec![]).unwrap();
    assert_eq!(val_before_half, Value::Int(-1));

    // 4. Exactly 1 second BEFORE epoch -> -1 second
    let ts_before_1s: Arc<dyn NativeValue> = Arc::new(Timestamp(UNIX_EPOCH - StdDuration::from_secs(1)));
    let val_before_1s = (unix_sec_method.implementation)(&ts_before_1s, vec![]).unwrap();
    assert_eq!(val_before_1s, Value::Int(-1));

    // 5. 1.2 seconds BEFORE epoch -> -2 seconds
    let ts_before_1_2s: Arc<dyn NativeValue> = Arc::new(Timestamp(UNIX_EPOCH - StdDuration::from_millis(1200)));
    let val_before_1_2s = (unix_sec_method.implementation)(&ts_before_1_2s, vec![]).unwrap();
    assert_eq!(val_before_1_2s, Value::Int(-2));
}

// =========================================================================
// Criterion 10: elapsed with zero duration and invalid clock order
// =========================================================================
#[test]
fn test_criterion_10_elapsed_zero_and_invalid_order() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/criterion10/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.time

        func test_elapsed() -> Int | time.TimeError {
            m1 := time.monotonic()
            d := time.elapsed(m1)?
            return d.nanoseconds()?
        }

        func main() {
            switch test_elapsed() {
                case Int(nanos):
                    print(nanos)
                case time.TimeError(e):
                    print(e.message)
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

// =========================================================================
// Criterion 11: Reachability analysis report
// =========================================================================
#[test]
fn test_criterion_11_reachability_analysis_unused_code() {
    let mut loader = ModuleLoader::new();
    let helper = PathBuf::from("/criterion11/helper.cdt");
    let main = PathBuf::from("/criterion11/main.cdt");

    loader.with_virtual_file(
        &helper,
        r#"
        export func used_function() -> Int {
            return helper_internal()
        }

        func helper_internal() -> Int {
            return 42
        }

        export func unused_exported_function() -> Int {
            return 99
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./helper" as helper
        import cdt.time

        func main() {
            helper.used_function()
            t := time.now()
        }

        func unused_local_function() {}
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let registry = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &registry);
    let report = analyzer.analyze(&root, &[]);

    assert!(report.is_function_reachable(&main, "main"));
    assert!(report.is_function_reachable(&helper, "used_function"));
    assert!(report.is_function_reachable(&helper, "helper_internal"));

    // Unused functions are NOT reachable
    assert!(!report.is_function_reachable(&helper, "unused_exported_function"));
    assert!(!report.is_function_reachable(&main, "unused_local_function"));

    // Native dependencies
    assert!(report.is_native_function_reachable("cdt.time", "now"));
    assert!(!report.is_native_function_reachable("cdt.time", "monotonic"));

    assert_eq!(report.unused_functions_count, 2);
}

// =========================================================================
// Criterion 12: Preserving existing language features
// =========================================================================
#[test]
fn test_criterion_12_language_features_preserved() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/criterion12/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        func inc(x: Int) -> Int { return x + 1 }

        func test_all_features() -> Int {
            // Guard
            var opt: Int? = 10
            guard opt != nil else {
                return 0
            }

            // Coalesce ??
            val := opt ?? 5

            // Pipe |>
            piped := val |> inc()

            // Loop with break & continue
            var sum = 0
            for i in [1, 2, 3, 4, 5] {
                if i == 2 {
                    continue
                }
                if i == 4 {
                    break
                }
                sum = sum + i
            }

            // Tuples
            (a, b) := (sum, piped)
            return a + b
        }

        func main() {
            res := test_all_features()
            print(res)
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    if let Err(e) = checker.check_project(&root, &loader) {
        panic!("check_project error: {e}");
    }

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

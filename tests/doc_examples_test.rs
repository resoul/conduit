use std::path::Path;
use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;

#[test]
fn test_all_positive_documentation_examples_pass() {
    let positive_files = [
        "examples/docs/getting_started/hello.cdt",
        "examples/docs/language/variables.cdt",
        "examples/docs/language/types.cdt",
        "examples/docs/language/functions.cdt",
        "examples/docs/language/control_flow.cdt",
        "examples/docs/language/collections.cdt",
        "examples/docs/language/errors.cdt",
        "examples/docs/language/tuples.cdt",
        "examples/docs/language/modules_main.cdt",
        "examples/docs/library/json.cdt",
        "examples/docs/library/time.cdt",
        "examples/docs/library/validation.cdt",
        "examples/docs/guides/error_recipe.cdt",
        "examples/docs/guides/module_recipe_main.cdt",
    ];

    for file_path in &positive_files {
        let path = Path::new(file_path);
        assert!(path.exists(), "Example file not found: {}", file_path);

        let mut loader = ModuleLoader::new();
        let root = loader
            .load_project(path)
            .unwrap_or_else(|e| panic!("Failed to load {}: {}", file_path, e));

        let mut checker = TypeChecker::new();
        checker
            .check_project(&root, &loader)
            .unwrap_or_else(|e| panic!("Type check failed for {}: {}", file_path, e));

        let mut interpreter = Interpreter::new();
        interpreter
            .run_project(&root, &loader)
            .unwrap_or_else(|e| panic!("Execution failed for {}: {}", file_path, e));
    }
}

#[test]
fn test_negative_documentation_examples_fail_as_expected() {
    let negative_cases = [
        (
            "examples/docs/negative/discard_error.err.cdt",
            "contains an error and must not be discarded",
        ),
        (
            "examples/docs/negative/private_leak.err.cdt",
            "exposes private type 'PrivateData' in return type",
        ),
        (
            "examples/docs/negative/try_optional.err.cdt",
            "Operator '?' is only allowed on types containing error variants, got 'Int?'",
        ),
    ];

    for (file_path, expected_substring) in &negative_cases {
        let path = Path::new(file_path);
        assert!(path.exists(), "Negative file not found: {}", file_path);

        let mut loader = ModuleLoader::new();
        let root = match loader.load_project(path) {
            Ok(r) => r,
            Err(e) => {
                assert!(
                    e.to_string().contains(expected_substring),
                    "Expected substring '{}' in loader error, got: {}",
                    expected_substring,
                    e
                );
                continue;
            }
        };

        let mut checker = TypeChecker::new();
        let check_res = checker.check_project(&root, &loader);
        assert!(check_res.is_err(), "Expected type check failure for {}", file_path);
        let err_msg = check_res.unwrap_err().to_string();
        assert!(
            err_msg.contains(expected_substring),
            "Expected substring '{}' in checker error for {}, got: {}",
            expected_substring,
            file_path,
            err_msg
        );
    }
}

#[test]
fn test_deterministic_library_docs_generation() {
    let registry = NativeRegistry::new();
    let time_mod = registry.get_module("cdt.time").expect("cdt.time module exists");
    let gen1 = time_mod.generate_markdown_docs();
    let gen2 = time_mod.generate_markdown_docs();

    assert_eq!(gen1, gen2, "Doc generation must be strictly deterministic and idempotent");
    assert!(gen1.contains("# cdt.time"));
    assert!(gen1.contains("### Timestamp"));
    assert!(gen1.contains("### Instant"));
    assert!(gen1.contains("### Duration"));
    assert!(gen1.contains("### TimeError"));
    assert!(gen1.contains("| `now` |"));
    assert!(gen1.contains("| `monotonic` |"));
    assert!(gen1.contains("| `elapsed` |"));
    assert!(gen1.contains("| `unix_seconds` |"));
}


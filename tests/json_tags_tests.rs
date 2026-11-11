use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;

fn run_project(files: &[(&str, &str)]) -> Result<(), String> {
    let mut loader = ModuleLoader::new();
    for (path, source) in files {
        loader.with_virtual_file(&PathBuf::from(path), *source);
    }
    let root = loader
        .load_project(&PathBuf::from(files[0].0))
        .map_err(|e| e.message)?;
    TypeChecker::new()
        .check_project(&root, &loader)
        .map_err(|e| e.message)?;
    Interpreter::new()
        .run_project(&root, &loader)
        .map_err(|e| e.message)
}

#[test]
fn tagged_fields_round_trip_with_generics_modules_and_validation() {
    run_project(&[
        ("/virtual/main.cdt", r#"
            import cdt.json as json
            import cdt.validation as validation
            import "./models" as models

            type Envelope<T> {
                data: T `json:"payload"`
                count: Int
            }

            func assert_true(ok: Bool) {
                if !ok { boom := 1 / 0 }
            }

            func main() {
                raw := "{\"payload\":{\"display_name\":\"Alice\",\"coordinates\":[{\"x_coord\":3},{\"x_coord\":7}],\"note\":null},\"count\":2}"
                switch json.decode<Envelope<models.User>>(raw) {
                    case Envelope(user):
                        assert_true(user.data.name == "Alice")
                        assert_true((user.data.coordinates.get(0) ?? models.Point(x: 0)).x == 3)
                        switch validation.check(user) {
                            case Envelope(valid):
                                switch json.encode(valid) {
                                    case String(encoded): assert_true(encoded == raw)
                                    case json.EncodeError(e): boom := 1 / 0
                                }
                            case validation.ValidationError(e): boom := 1 / 0
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }

                bad := "{\"payload\":{\"display_name\":\"A\",\"coordinates\":[],\"note\":null},\"count\":0}"
                switch json.decode<Envelope<models.User>>(bad) {
                    case Envelope(user):
                        switch validation.check(user) {
                            case Envelope(valid): boom := 1 / 0
                            case validation.ValidationError(e):
                                assert_true(e.issues.len() == 1)
                                assert_true((e.issues.get(0) ?? validation.Violation(path: "", code: "", message: "")).path == "data.name")
                        }
                    case json.DecodeError(e): boom := 1 / 0
                }

                // The old Conduit name is no longer accepted as a JSON key.
                switch json.decode<models.Point>("{\"x\":3}") {
                    case models.Point(p): boom := 1 / 0
                    case json.DecodeError(e): assert_true(e.path == "x")
                }
            }
        "#),
        ("/virtual/models.cdt", r#"
            import cdt.validation as validation
            export type Point { x: Int `json:"x_coord"` }
            export type User {
                name: String `json:"display_name"` @validation.min(2)
                coordinates: [Point] `json:"coordinates"`
                note: String?
            }
        "#),
    ]).expect("JSON tags should work through imported generic structures and independently of validation");
}

#[test]
fn tagged_special_key_round_trips_and_reports_its_path() {
    run_project(&[(
        "/virtual/main.cdt",
        r#"
        import cdt.json as json
        type Point { x: Int `json:"a.b"` }
        func main() {
            switch json.decode<Point>("{\"a.b\":7}") {
                case Point(p):
                    if p.x != 7 { boom := 1 / 0 }
                    switch json.encode(p) {
                        case String(s): if s != "{\"a.b\":7}" { boom := 1 / 0 }
                        case json.EncodeError(e): boom := 1 / 0
                    }
                case json.DecodeError(e): boom := 1 / 0
            }
            switch json.decode<Point>("{\"a.b\":\"wrong\"}") {
                case Point(p): boom := 1 / 0
                case json.DecodeError(e):
                    if e.path != "[\"a.b\"]" { boom := 1 / 0 }
            }
        }
    "#,
    )])
    .expect("special JSON names should round-trip and produce unambiguous paths");
}

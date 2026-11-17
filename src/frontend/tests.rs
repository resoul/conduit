use std::collections::HashMap;
use std::sync::Arc;

use crate::frontend::{
    check_source, compile_source, compile_source_roots, compile_with_modules, parse_source,
};
use crate::ir::{CallableRef, PipelineStage};
use crate::vm::Vm;
use crate::vm::opcode::Op;
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj, OrderedMap};

#[test]
fn parse_and_check_errors_are_rejected() {
    assert!(parse_source("func broken(x: Int -> Int { return x }").is_err());
    assert!(parse_source("type LegacyStruct { id: Int }").is_err());
    assert!(check_source("func bad_ret() -> Int { return \"hello\" }").is_err());
    assert!(check_source("func undef_var() -> Int { return missing + 1 }").is_err());
    assert!(
        check_source(
            "import cdt.json\n\
             func bad_json(s: String) -> Int {\n\
                 _ := json.decode<(Int) -> Int>(s)?\n\
                 return 0\n\
             }"
        )
        .is_err()
    );
    assert!(
        check_source(
            "type A = B\n\
             type B = A\n\
             func f(x: A) -> Int { return 0 }"
        )
        .is_err()
    );
}

#[test]
fn closure_unused_capture_elision() {
    let src = "struct Row {\n\
                   id: Int\n\
                   name: String\n\
               }\n\
               func make_and_call(base: Int, unused_rows: [Row]) -> Int {\n\
                   cb := func(x: Int) -> Int { return base + x }\n\
                   return cb(1)\n\
               }";
    let module = compile_source(src).expect("should compile");
    assert_eq!(module.closures.len(), 1);
    assert_eq!(module.closures[0].capture_scalars.len(), 1);
    assert!(module.closures[0].capture_objs.is_empty());

    let fid = module.func_id("make_and_call").unwrap();
    let empty_rows = Obj::List(ListStorage::Obj(Arc::new(vec![])));
    let res = Vm::new()
        .call_scalar(&module, fid, &[41], &[empty_rows])
        .expect("should execute");
    assert_eq!(res, 42);
}

#[test]
fn counting_while_lowers_to_for_range_step() {
    let src = "func sum_while(n: Int) -> Int {\n\
                   var i = 0\n\
                   var s = 0\n\
                   while i < n {\n\
                       s = s + i\n\
                       i = i + 1\n\
                   }\n\
                   return s\n\
               }";
    let module = compile_source(src).expect("should compile");
    let fid = module.func_id("sum_while").unwrap();
    assert!(
        module.functions[fid as usize]
            .ops
            .iter()
            .any(|op| matches!(op, Op::ForRangeStep { .. }))
    );

    let res = Vm::new()
        .call_scalar(&module, fid, &[10], &[])
        .expect("should execute");
    assert_eq!(res, 45);
}

#[test]
fn chained_pipe_with_arrow_lambdas_compiles_to_direct_fused_pipeline() {
    let src = "func pipe_len(items: [Int]) -> Int {\n\
                   out := items |> filter(x => x % 2 == 0) |> map(x => x + 1)\n\
                   return out.len()\n\
               }";
    let module = compile_source(src).expect("should compile");
    let fid = module.func_id("pipe_len").unwrap();
    let run_count = module.functions[fid as usize]
        .ops
        .iter()
        .filter(|op| matches!(op, Op::RunPipeline { .. }))
        .count();
    assert_eq!(run_count, 1);
    assert_eq!(module.pipelines.len(), 1);
    assert_eq!(module.pipelines[0].stages.len(), 2);
    assert!(matches!(
        module.pipelines[0].stages[0],
        PipelineStage::FilterScalar(CallableRef::DirectFunc(_))
    ));
    assert!(matches!(
        module.pipelines[0].stages[1],
        PipelineStage::MapScalarToScalar(CallableRef::DirectFunc(_))
    ));

    let items = Obj::List(ListStorage::Int(Arc::new(vec![0, 1, 2, 3, 4])));
    let res = Vm::new()
        .call_scalar(&module, fid, &[], &[items])
        .expect("should execute");
    assert_eq!(res, 3);
}

#[test]
fn arrow_lambdas_with_multiple_params_and_typed_vars() {
    let src = "func run_lambdas(k: Int) -> Int {\n\
                   var inc: (Int) -> Int = x => x + 1\n\
                   var add: (Int, Int) -> Int = (a, b) => a + b + k\n\
                   return add(inc(9), 2)\n\
               }";
    let module = compile_source(src).expect("should compile");
    let fid = module.func_id("run_lambdas").unwrap();
    let res = Vm::new()
        .call_scalar(&module, fid, &[30], &[])
        .expect("should execute");
    assert_eq!(res, 42);
}

#[test]
fn cow_list_and_map_shorthand_isolation_from_cdt() {
    let src = "type Counts = [Int: Int]\n\
               func mutate_copies(items: [Int], m: Counts) -> Int {\n\
                   var out_list = items\n\
                   out_list.append(99)\n\
                   var out_map = m\n\
                   out_map.set(1, 42)\n\
                   var fresh = [Int: Int]()\n\
                   fresh.set(7, 10)\n\
                   got := out_map.get(1) ?? -1\n\
                   fval := fresh.get(7) ?? 0\n\
                   return out_list.len() + got + fval\n\
               }";
    let module = compile_source(src).expect("should compile");
    let fid = module.func_id("mutate_copies").unwrap();

    let orig_list = Obj::List(ListStorage::Int(Arc::new(vec![1, 2, 3])));
    let mut map = OrderedMap::new();
    map.set(MapKey::Int(1), MapVal::Scalar(7));
    let orig_map = Obj::Map(Arc::new(map));

    let res = Vm::new()
        .call_scalar(&module, fid, &[], &[orig_list.clone(), orig_map.clone()])
        .expect("should execute");
    assert_eq!(res, 56);

    match orig_list {
        Obj::List(ListStorage::Int(v)) => assert_eq!(&*v, &[1, 2, 3]),
        _ => panic!("expected Int list"),
    }
    match orig_map {
        Obj::Map(arc) => assert_eq!(arc.get(&MapKey::Int(1)), Some(&MapVal::Scalar(7))),
        _ => panic!("expected Map"),
    }
}

#[test]
fn record_spread_json_attr_and_type_alias_roundtrip() {
    let src = "import cdt.json\n\
               type UserId = Int\n\
               struct Item {\n\
                   id: UserId\n\
                   name: String @json(\"display_name\")\n\
                   active: Bool\n\
                   score: Float\n\
               }\n\
               type ItemBatch = [Item]\n\
               func json_etl(raw: String) -> String {\n\
                   items := json.decode<ItemBatch>(raw)?\n\
                   out := items |> filter(it => it.active) |> map(it => Item(..it, score: it.score + 5.0))\n\
                   return json.encode(out)?\n\
               }";
    let module = compile_source(src).expect("should compile");
    let fid = module.func_id("json_etl").unwrap();
    let input = Obj::str(
        r#"[{"id":1,"display_name":"Ada","active":true,"score":10.5},{"id":2,"display_name":"Bob","active":false,"score":20.0}]"#,
    );
    let out = Vm::new()
        .call_obj(&module, fid, &[], &[input])
        .expect("should execute");
    let Obj::Str(s) = out else {
        panic!("expected String output");
    };
    assert_eq!(
        s.as_str(),
        r#"[{"id":1,"display_name":"Ada","active":true,"score":15.5}]"#
    );
}

#[test]
fn ts_style_json_named_and_namespace_imports() {
    let src = "import { decode, encode as to_json, DecodeError } from \"cdt.json\"\n\
               import * as jns from \"cdt.json\"\n\
               struct User { id: Int, name: String }\n\
               export func roundtrip(raw: String) -> String {\n\
                   u := decode<User>(raw)?\n\
                   s2 := jns.encode(u)?\n\
                   if s2 == \"\" {\n\
                       return \"\"\n\
                   }\n\
                   return to_json(u)?\n\
               }";
    let module = compile_source(src).expect("TS-style cdt.json imports should compile");
    let fid = module.func_id("roundtrip").unwrap();
    let input = Obj::str(r#"{"id":7,"name":"Grace"}"#);
    let Obj::Str(out) = Vm::new()
        .call_obj(&module, fid, &[], &[input])
        .expect("should execute")
    else {
        panic!("expected String");
    };
    assert_eq!(out.as_str(), r#"{"id":7,"name":"Grace"}"#);

    let default_src = "import jdef from \"cdt.json\"\n\
                       struct Boxed { v: Int }\n\
                       export func enc(v: Int) -> String {\n\
                           return jdef.encode(Boxed(v: v))?\n\
                       }";
    let def_mod = compile_source(default_src).expect("default import from cdt.json should work");
    let dfid = def_mod.func_id("enc").unwrap();
    let Obj::Str(dout) = Vm::new().call_obj(&def_mod, dfid, &[9], &[]).unwrap() else {
        panic!("expected String");
    };
    assert_eq!(dout.as_str(), r#"{"v":9}"#);

    assert!(compile_source("import { missing } from \"cdt.json\"\nfunc f() -> Int { return 0 }").is_err());
    assert!(compile_source("import { bad } from \"./unknown\"\nfunc f() -> Int { return 0 }").is_err());
}

#[test]
fn single_file_tree_shaking_eliminates_unreachable_functions() {
    let src = "func used_helper(x: Int) -> Int {\n\
                   return x * 2\n\
               }\n\
               func unused_dead_fn(x: Int) -> Int {\n\
                   return unused_chain(x) + 1\n\
               }\n\
               func unused_chain(x: Int) -> Int {\n\
                   return x + 100\n\
               }\n\
               func via_export_list(x: Int) -> Int {\n\
                   return used_helper(x) + 3\n\
               }\n\
               export { via_export_list }\n\
               export func main_entry(x: Int) -> Int {\n\
                   return used_helper(x)\n\
               }";
    let module = compile_source(src).expect("should compile and tree-shake");
    assert!(module.func_id("main_entry").is_some());
    assert!(module.func_id("via_export_list").is_some());
    assert!(module.func_id("used_helper").is_some());
    assert!(module.func_id("unused_dead_fn").is_none());
    assert!(module.func_id("unused_chain").is_none());
    assert_eq!(module.functions.len(), 3);

    let fid = module.func_id("main_entry").unwrap();
    assert_eq!(Vm::new().call_scalar(&module, fid, &[21], &[]).unwrap(), 42);

    let root_only =
        compile_source_roots(src, &["via_export_list"]).expect("explicit root should compile");
    assert!(root_only.func_id("via_export_list").is_some());
    assert!(root_only.func_id("used_helper").is_some());
    assert!(root_only.func_id("main_entry").is_none());
    assert!(root_only.func_id("unused_dead_fn").is_none());
    assert_eq!(root_only.functions.len(), 2);
}

#[test]
fn multi_module_ts_imports_and_tree_shaking() {
    let mut modules = HashMap::new();
    modules.insert(
        "./core",
        "export func double_val(x: Int) -> Int {\n\
             return x * 2\n\
         }\n\
         export func unused_triple(x: Int) -> Int {\n\
             return x * 3\n\
         }",
    );
    modules.insert(
        "./math",
        "import { double_val } from \"./core\"\n\
         export struct Pair { a: Int, b: Int }\n\
         func private_step(x: Int) -> Int {\n\
             return double_val(x) + 1\n\
         }\n\
         export func add(a: Int, b: Int) -> Int {\n\
             return private_step(a) + b\n\
         }\n\
         export func unused_mul(a: Int, b: Int) -> Int {\n\
             return a * b\n\
         }",
    );

    let named_main = "import { add } from \"./math\"\n\
                      export func run(x: Int, y: Int) -> Int {\n\
                          return add(x, y)\n\
                      }";
    let module = compile_with_modules(named_main, &modules).expect("named import should compile");
    assert!(module.func_id("run").is_some());
    assert!(module.func_id("add").is_some());
    assert!(module.func_id("double_val").is_none()); // transitive helper gets module-scoped name
    assert!(module.functions.iter().any(|f| f.name.contains("double_val")));
    assert!(module.functions.iter().any(|f| f.name.contains("private_step")));
    assert!(module.func_id("unused_mul").is_none());
    assert!(!module.functions.iter().any(|f| f.name.contains("unused_mul")));
    assert!(!module.functions.iter().any(|f| f.name.contains("unused_triple")));
    assert_eq!(module.functions.len(), 4);

    let fid = module.func_id("run").unwrap();
    assert_eq!(Vm::new().call_scalar(&module, fid, &[10, 5], &[]).unwrap(), 26);

    let ns_main = "import * as math from \"./math\"\n\
                   export func sum_pair(x: Int, y: Int) -> Int {\n\
                       p := math.Pair(x, y)\n\
                       return math.add(p.a, p.b)\n\
                   }";
    let ns_mod = compile_with_modules(ns_main, &modules).expect("namespace import should compile");
    assert!(!ns_mod.functions.iter().any(|f| f.name.contains("unused_mul")));
    let sfid = ns_mod.func_id("sum_pair").unwrap();
    assert_eq!(Vm::new().call_scalar(&ns_mod, sfid, &[4, 7], &[]).unwrap(), 16);

    let bad_priv = "import { private_step } from \"./math\"\n\
                    export func run() -> Int { return private_step(1) }";
    assert!(compile_with_modules(bad_priv, &modules).is_err());

    let bad_ns = "import * as math from \"./math\"\n\
                  export func run() -> Int { return math.private_step(1) }";
    assert!(compile_with_modules(bad_ns, &modules).is_err());
}

#[test]
fn module_consts_and_cross_module_exports() {
    let cfg_src = r#"
        const BASE_PORT = 8000
        export const PORT: Int = BASE_PORT + 80
        export const NEG_OFFSET = -5
        export const RATIO: Float = 1.5
        export const PREFIX = "/v1"
        export const ENABLED = true
    "#;
    let main_src = r#"
        import * as cfg from "./cfg"
        import { PREFIX, NEG_OFFSET } from "./cfg"

        const LOCAL_STEP = 2

        func with_default(p: Int = cfg.PORT) -> Int {
            return p + NEG_OFFSET + LOCAL_STEP
        }

        export func run() -> String {
            if cfg.ENABLED {
                v := with_default()
                return PREFIX + ":" + v.to_string() + ":" + cfg.RATIO.to_string()
            }
            return ""
        }
    "#;
    let mut modules = HashMap::new();
    modules.insert("./cfg", cfg_src);
    let module = compile_with_modules(main_src, &modules).expect("compile consts");
    let fid = module.func_id("run").unwrap();
    let res = Vm::new().call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("/v1:8077:1.5"));

    let bad_assign = "const X = 1\nexport func f() -> Int { X = 2\n return X }";
    let err = compile_source(bad_assign).unwrap_err();
    assert!(err.contains("Cannot assign to constant"), "got {err}");

    let bad_nonconst = "func g() -> Int { return 1 }\nconst X = g()\nexport func f() -> Int { return X }";
    assert!(compile_source(bad_nonconst).is_err());
}

#[test]
fn enum_and_const_interp_cross_module() {
    let mod_a = r#"
        export enum Role {
            Admin,
            User,
            Guest
        }
        export const TAG = "mod_a"
    "#;
    let main_src = r#"
        import { Role } from "./a"
        import * as a from "./a"

        const TAG = "local"

        func describe(r: Role) -> String {
            switch r {
                case Role.Admin:
                    return "admin"
                case Role.User:
                    return "user"
                case _:
                    return "guest"
            }
        }

        export func run() -> String {
            r := Role.Admin
            eq := r == Role.Admin && r != Role.Guest
            roles := [Role.Admin, Role.User]
            return "{TAG}:{a.TAG}:" + describe(roles.get(1) ?? Role.Guest) + ":" + eq.to_string()
        }
    "#;
    let mut modules = HashMap::new();
    modules.insert("./a", mod_a);
    let module = compile_with_modules(main_src, &modules).expect("compile enum");
    let fid = module.func_id("run").unwrap();
    let res = Vm::new().call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("local:mod_a:user:true"));
}

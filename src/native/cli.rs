use std::sync::Arc;

use crate::vm::value::{ListStorage, Obj, SCHEMA_CLI_ERROR, StructData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliKind {
    Flag,
    Opt,
    Arg,
    Rest,
    Cmd,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliValKind {
    String,
    Int,
    Float,
    Bool,
    ListStr,
    SubStruct,
}

#[derive(Debug, Clone)]
pub enum CliDefault {
    Int(i64),
    Float(u64),
    Bool(bool),
    Str(String),
    Nil,
    EmptyList,
}

#[derive(Debug, Clone)]
pub struct CliFieldPlan {
    pub field_name: String,
    pub cli_name: String,
    pub short: Option<char>,
    pub help: String,
    pub kind: CliKind,
    pub val_kind: CliValKind,
    pub is_optional: bool,
    pub is_scalar: bool,
    pub slot_idx: u16,
    pub default: Option<CliDefault>,
    pub sub_node: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct CliNodePlan {
    pub name: String,
    pub about: String,
    pub schema_id: u32,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub fields: Box<[CliFieldPlan]>,
}

#[derive(Debug, Clone)]
pub struct CliPlan {
    pub nodes: Box<[CliNodePlan]>,
}

pub fn make_cli_error(kind: impl Into<String>, message: impl Into<String>) -> Obj {
    let mut rec = StructData::new(SCHEMA_CLI_ERROR, 0, 2);
    rec.set_obj(0, Obj::str(kind));
    rec.set_obj(1, Obj::str(message));
    Obj::Record(Arc::new(rec))
}

pub fn format_cli_help(plan: &CliPlan, node_idx: usize, cmd_path: &[String]) -> String {
    let node = &plan.nodes[node_idx];
    let mut out = String::new();
    let title = if cmd_path.is_empty() {
        node.name.clone()
    } else {
        cmd_path.join(" ")
    };
    if node.about.is_empty() {
        out.push_str(&title);
    } else {
        out.push_str(&format!("{} — {}", title, node.about));
    }
    out.push('\n');

    let mut usage = format!("\nUsage: {}", title);
    let has_opts = node
        .fields
        .iter()
        .any(|f| matches!(f.kind, CliKind::Flag | CliKind::Opt));
    let has_cmds = node
        .fields
        .iter()
        .any(|f| matches!(f.kind, CliKind::Cmd | CliKind::Group));
    if has_opts {
        usage.push_str(" [OPTIONS]");
    }
    if has_cmds {
        usage.push_str(" [COMMAND]");
    }
    for f in node.fields.iter() {
        match f.kind {
            CliKind::Arg => {
                if f.is_optional || f.default.is_some() {
                    usage.push_str(&format!(" [{}]", f.cli_name));
                } else {
                    usage.push_str(&format!(" <{}>", f.cli_name));
                }
            }
            CliKind::Rest => usage.push_str(&format!(" [{}...]", f.cli_name)),
            _ => {}
        }
    }
    out.push_str(&usage);
    out.push('\n');

    let cmds: Vec<&CliFieldPlan> = node
        .fields
        .iter()
        .filter(|f| matches!(f.kind, CliKind::Cmd | CliKind::Group))
        .collect();
    if !cmds.is_empty() {
        out.push_str("\nCommands:\n");
        for f in cmds {
            if f.help.is_empty() {
                out.push_str(&format!("  {}\n", f.cli_name));
            } else {
                out.push_str(&format!("  {:<18} {}\n", f.cli_name, f.help));
            }
        }
    }

    let args: Vec<&CliFieldPlan> = node
        .fields
        .iter()
        .filter(|f| matches!(f.kind, CliKind::Arg | CliKind::Rest))
        .collect();
    if !args.is_empty() {
        out.push_str("\nArguments:\n");
        for f in args {
            let label = if f.kind == CliKind::Rest {
                format!("{}...", f.cli_name)
            } else {
                f.cli_name.clone()
            };
            if f.help.is_empty() {
                out.push_str(&format!("  {}\n", label));
            } else {
                out.push_str(&format!("  {:<18} {}\n", label, f.help));
            }
        }
    }

    let opts: Vec<&CliFieldPlan> = node
        .fields
        .iter()
        .filter(|f| matches!(f.kind, CliKind::Flag | CliKind::Opt))
        .collect();
    out.push_str("\nOptions:\n");
    for f in opts {
        let short_part = f
            .short
            .map(|c| format!("-{}, ", c))
            .unwrap_or_else(|| "    ".to_string());
        let val_part = if f.kind == CliKind::Opt {
            " <VAL>"
        } else {
            ""
        };
        let lhs = format!("{}--{}{}", short_part, f.cli_name, val_part);
        if f.help.is_empty() {
            out.push_str(&format!("  {}\n", lhs));
        } else {
            out.push_str(&format!("  {:<24} {}\n", lhs, f.help));
        }
    }
    out.push_str("  -h, --help               Show help\n");
    out
}

pub fn exec_cli_parse(plan: &CliPlan, args: &[String]) -> Obj {
    let root_name = plan
        .nodes
        .first()
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "app".to_string());
    let mut path = vec![root_name];
    match parse_node(plan, 0, args, &mut path) {
        Ok(obj) => obj,
        Err(err_obj) => err_obj,
    }
}

fn parse_node(
    plan: &CliPlan,
    node_idx: usize,
    args: &[String],
    cmd_path: &mut Vec<String>,
) -> Result<Obj, Obj> {
    let node = &plan.nodes[node_idx];
    let mut rec = StructData::new(node.schema_id, node.scalar_len, node.obj_len);
    let mut set_flags = vec![false; node.fields.len()];
    let mut rest_vals: Vec<String> = Vec::new();
    let mut only_pos = false;
    let mut i = 0usize;

    while i < args.len() {
        let arg = &args[i];
        if !only_pos && arg == "--" {
            only_pos = true;
            i += 1;
            continue;
        }
        if !only_pos && (arg == "--help" || arg == "-h") {
            return Err(make_cli_error("help", format_cli_help(plan, node_idx, cmd_path)));
        }
        if !only_pos && let Some(long_body) = arg.strip_prefix("--") {
            let (name, inline_val) = match long_body.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (long_body, None),
            };
            if let Some((idx, f)) = find_long_field(&node.fields, name) {
                match f.kind {
                    CliKind::Flag => {
                        if inline_val.is_some() {
                            return Err(make_cli_error(
                                "invalid",
                                format!("Flag '--{}' does not take a value", f.cli_name),
                            ));
                        }
                        store_bool(&mut rec, f, true);
                        set_flags[idx] = true;
                    }
                    CliKind::Opt => {
                        let raw = if let Some(v) = inline_val {
                            v.to_string()
                        } else {
                            i += 1;
                            if i >= args.len() {
                                return Err(make_cli_error(
                                    "missing",
                                    format!("Option '--{}' requires a value", f.cli_name),
                                ));
                            }
                            args[i].clone()
                        };
                        store_parsed_value(&mut rec, f, &raw)?;
                        set_flags[idx] = true;
                    }
                    _ => unreachable!(),
                }
            } else if let Some(stripped) = name.strip_prefix("no-")
                && let Some((idx, f)) = find_long_field(&node.fields, stripped)
                && f.kind == CliKind::Flag
            {
                store_bool(&mut rec, f, false);
                set_flags[idx] = true;
            } else {
                return Err(make_cli_error(
                    "unknown",
                    format!("Unknown option '--{}'", name),
                ));
            }
            i += 1;
            continue;
        }
        if !only_pos && arg.starts_with('-') && arg.len() > 1 {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut ci = 0usize;
            while ci < chars.len() {
                let ch = chars[ci];
                let Some((idx, f)) = node.fields.iter().enumerate().find(|(_, f)| {
                    matches!(f.kind, CliKind::Flag | CliKind::Opt) && f.short == Some(ch)
                }) else {
                    return Err(make_cli_error("unknown", format!("Unknown flag '-{}'", ch)));
                };
                match f.kind {
                    CliKind::Flag => {
                        store_bool(&mut rec, f, true);
                        set_flags[idx] = true;
                        ci += 1;
                    }
                    CliKind::Opt => {
                        let raw = if ci + 1 < chars.len() {
                            let rest: String = chars[ci + 1..].iter().collect();
                            ci = chars.len();
                            rest
                        } else {
                            i += 1;
                            if i >= args.len() {
                                return Err(make_cli_error(
                                    "missing",
                                    format!("Option '-{}' requires a value", ch),
                                ));
                            }
                            ci = chars.len();
                            args[i].clone()
                        };
                        store_parsed_value(&mut rec, f, &raw)?;
                        set_flags[idx] = true;
                    }
                    _ => unreachable!(),
                }
            }
            i += 1;
            continue;
        }

        // Positional: check subcommands first if any exist on this node.
        if let Some((idx, f)) = node.fields.iter().enumerate().find(|(_, f)| {
            matches!(f.kind, CliKind::Cmd | CliKind::Group) && f.cli_name == *arg
        }) {
            let sub_idx = f.sub_node.unwrap() as usize;
            cmd_path.push(f.cli_name.clone());
            let sub_obj = parse_node(plan, sub_idx, &args[i + 1..], cmd_path)?;
            cmd_path.pop();
            rec.set_obj(f.slot_idx as usize, sub_obj);
            set_flags[idx] = true;
            break;
        }

        if let Some((idx, f)) = node
            .fields
            .iter()
            .enumerate()
            .find(|(idx, f)| f.kind == CliKind::Arg && !set_flags[*idx])
        {
            store_parsed_value(&mut rec, f, arg)?;
            set_flags[idx] = true;
            i += 1;
            continue;
        }

        if let Some((idx, _)) = node
            .fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.kind == CliKind::Rest)
        {
            rest_vals.push(arg.clone());
            set_flags[idx] = true;
            i += 1;
            continue;
        }

        return Err(make_cli_error(
            "unknown",
            format!("Unexpected argument '{}'", arg),
        ));
    }

    for (idx, f) in node.fields.iter().enumerate() {
        if set_flags[idx] {
            if f.kind == CliKind::Rest {
                let objs = rest_vals.iter().map(Obj::str).collect();
                rec.set_obj(f.slot_idx as usize, Obj::List(ListStorage::Obj(Arc::new(objs))));
            }
            continue;
        }
        match f.kind {
            CliKind::Rest => {
                rec.set_obj(
                    f.slot_idx as usize,
                    Obj::List(ListStorage::Obj(Arc::new(Vec::new()))),
                );
            }
            CliKind::Cmd | CliKind::Group => {
                rec.set_obj(f.slot_idx as usize, Obj::Empty);
            }
            CliKind::Flag => {
                let b = match f.default {
                    Some(CliDefault::Bool(v)) => v,
                    _ => false,
                };
                store_bool(&mut rec, f, b);
            }
            CliKind::Opt | CliKind::Arg => {
                if let Some(def) = &f.default {
                    store_default(&mut rec, f, def);
                } else if f.is_optional {
                    rec.set_obj(f.slot_idx as usize, Obj::Empty);
                } else {
                    let kind_label = if f.kind == CliKind::Arg {
                        format!("argument '<{}>'", f.cli_name)
                    } else {
                        format!("option '--{}'", f.cli_name)
                    };
                    return Err(make_cli_error(
                        "missing",
                        format!("Missing required {}", kind_label),
                    ));
                }
            }
        }
    }

    Ok(Obj::Record(Arc::new(rec)))
}

fn find_long_field<'a>(fields: &'a [CliFieldPlan], name: &str) -> Option<(usize, &'a CliFieldPlan)> {
    fields.iter().enumerate().find(|(_, f)| {
        matches!(f.kind, CliKind::Flag | CliKind::Opt) && f.cli_name == name
    })
}

fn store_bool(rec: &mut StructData, f: &CliFieldPlan, val: bool) {
    if f.is_optional {
        rec.set_obj(f.slot_idx as usize, Obj::boxed_bool(val));
    } else {
        rec.set_scalar(f.slot_idx as usize, u64::from(val));
    }
}

fn store_default(rec: &mut StructData, f: &CliFieldPlan, def: &CliDefault) {
    let slot = f.slot_idx as usize;
    match def {
        CliDefault::Int(v) => {
            if f.is_optional { rec.set_obj(slot, Obj::boxed_int(*v)); } else { rec.set_scalar(slot, *v as u64); }
        }
        CliDefault::Float(bits) => {
            if f.is_optional { rec.set_obj(slot, Obj::boxed_float(*bits)); } else { rec.set_scalar(slot, *bits); }
        }
        CliDefault::Bool(b) => store_bool(rec, f, *b),
        CliDefault::Str(s) => rec.set_obj(slot, Obj::str(s.clone())),
        CliDefault::Nil => rec.set_obj(slot, Obj::Empty),
        CliDefault::EmptyList => rec.set_obj(slot, Obj::List(ListStorage::Obj(Arc::new(Vec::new())))),
    }
}

fn store_parsed_value(rec: &mut StructData, f: &CliFieldPlan, raw: &str) -> Result<(), Obj> {
    let slot = f.slot_idx as usize;
    match f.val_kind {
        CliValKind::String => rec.set_obj(slot, Obj::str(raw)),
        CliValKind::Int => {
            let v = raw.trim().parse::<i64>().map_err(|_| {
                make_cli_error("invalid", format!("Invalid Int for '{}': '{}'", f.cli_name, raw))
            })?;
            if f.is_optional { rec.set_obj(slot, Obj::boxed_int(v)); } else { rec.set_scalar(slot, v as u64); }
        }
        CliValKind::Float => {
            let v = raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| {
                    make_cli_error("invalid", format!("Invalid Float for '{}': '{}'", f.cli_name, raw))
                })?;
            if f.is_optional { rec.set_obj(slot, Obj::boxed_float(v.to_bits())); } else { rec.set_scalar(slot, v.to_bits()); }
        }
        CliValKind::Bool => {
            let b = match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => {
                    return Err(make_cli_error(
                        "invalid",
                        format!("Invalid Bool for '{}': '{}'", f.cli_name, raw),
                    ));
                }
            };
            store_bool(rec, f, b);
        }
        CliValKind::ListStr | CliValKind::SubStruct => {
            return Err(make_cli_error(
                "invalid",
                format!("Unsupported CLI target type for '{}'", f.cli_name),
            ));
        }
    }
    Ok(())
}

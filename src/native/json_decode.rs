use std::sync::Arc;

use crate::native::{
    CompiledStructDecodePlan, FieldDecodeKind, JsonDecodePlan, JsonError, MAX_JSON_DEPTH,
    PlanNode, err_at_field, err_at_index, join_field_path, join_index_path,
};
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj, OrderedMap, StructData};

pub fn decode_json_with_plan(raw: &str, plan: &JsonDecodePlan) -> Result<Obj, JsonError> {
    let mut decoder = JsonDecoder::new(raw);
    decoder.skip_ws();
    let val = decoder.decode_node(plan, plan.root, 0)?;
    decoder.skip_ws();
    if decoder.pos < decoder.bytes.len() {
        return Err(JsonError::root(
            "Unexpected trailing characters after JSON value",
        ));
    }
    Ok(val)
}

pub(crate) struct JsonDecoder<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) src: &'a str,
    pub(crate) pos: usize,
}

impl<'a> JsonDecoder<'a> {
    #[inline]
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            src,
            pos: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    #[inline(always)]
    pub(crate) fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn decode_node(
        &mut self,
        plan: &JsonDecodePlan,
        index: usize,
        depth: usize,
    ) -> Result<Obj, JsonError> {
        match &plan.nodes[index] {
            PlanNode::Pending => unreachable!("decode plan compilation left an unfinished node"),
            PlanNode::String => self.parse_string_obj(depth),
            PlanNode::Optional(inner) => {
                self.skip_ws();
                if self.bytes[self.pos..].starts_with(b"null") {
                    self.pos += 4;
                    Ok(Obj::Empty)
                } else {
                    self.decode_node(plan, *inner, depth)
                }
            }
            PlanNode::ListInt => self.decode_list_int(depth),
            PlanNode::ListFloat => self.decode_list_float(depth),
            PlanNode::ListStr => self.decode_list_str(depth),
            PlanNode::ListObj(child) => self.decode_list_obj(plan, *child, depth),
            PlanNode::Map { val_plan, val_name } => {
                self.decode_map(plan, *val_plan, val_name, depth)
            }
            PlanNode::Struct(schema) => self.decode_struct(schema, plan, depth),
            PlanNode::TaggedEnum {
                type_name,
                variants,
            } => self.decode_tagged_enum(type_name, variants, plan, depth),
            PlanNode::Unavailable(id) => Err(JsonError::root(format!(
                "Type 'Struct#{}' cannot be decoded from JSON",
                id
            ))),
            PlanNode::Int => self.parse_i64(depth).map(Obj::boxed_int),
            PlanNode::Float => self.parse_f64(depth).map(|v| Obj::boxed_float(v.to_bits())),
            PlanNode::Bool => self.parse_bool(depth).map(Obj::boxed_bool),
        }
    }

    #[inline]
    fn begin_array(&mut self, expected: &str, depth: usize) -> Result<bool, JsonError> {
        self.skip_ws();
        if self.peek() != Some(b'[') {
            return Err(self.wrong_kind(expected, depth));
        }
        if depth >= MAX_JSON_DEPTH {
            return Err(JsonError::root("Maximum JSON nesting depth (128) exceeded"));
        }
        self.pos += 1;
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(false);
        }
        Ok(true)
    }

    fn decode_list_int(&mut self, depth: usize) -> Result<Obj, JsonError> {
        if !self.begin_array("List", depth)? {
            return Ok(Obj::List(ListStorage::new_int(0)));
        }
        let cap = if depth == 0 && self.bytes.len() > 4096 {
            (self.bytes.len() / 8).clamp(16, 4096)
        } else {
            4
        };
        let mut items = Vec::with_capacity(cap);
        loop {
            let idx = items.len();
            let val = self
                .parse_i64(depth + 1)
                .map_err(|e| err_at_index(idx, e))?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Obj::List(ListStorage::Int(Arc::new(items))));
                }
                _ => {
                    return Err(JsonError::new(
                        join_index_path("", idx),
                        "Expected ',' or ']' in JSON array",
                    ));
                }
            }
        }
    }

    fn decode_list_float(&mut self, depth: usize) -> Result<Obj, JsonError> {
        if !self.begin_array("List", depth)? {
            return Ok(Obj::List(ListStorage::new_float(0)));
        }
        let cap = if depth == 0 && self.bytes.len() > 4096 {
            (self.bytes.len() / 12).clamp(16, 4096)
        } else {
            4
        };
        let mut items = Vec::with_capacity(cap);
        loop {
            let idx = items.len();
            let val = self
                .parse_f64(depth + 1)
                .map_err(|e| err_at_index(idx, e))?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Obj::List(ListStorage::Float(Arc::new(items))));
                }
                _ => {
                    return Err(JsonError::new(
                        join_index_path("", idx),
                        "Expected ',' or ']' in JSON array",
                    ));
                }
            }
        }
    }

    fn decode_list_str(&mut self, depth: usize) -> Result<Obj, JsonError> {
        if !self.begin_array("List", depth)? {
            return Ok(Obj::List(ListStorage::new_obj(0)));
        }
        let mut items = Vec::with_capacity(4);
        loop {
            let idx = items.len();
            let val = self
                .parse_string_obj(depth + 1)
                .map_err(|e| err_at_index(idx, e))?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Obj::List(ListStorage::Obj(Arc::new(items))));
                }
                _ => {
                    return Err(JsonError::new(
                        join_index_path("", idx),
                        "Expected ',' or ']' in JSON array",
                    ));
                }
            }
        }
    }

    fn decode_list_obj(
        &mut self,
        plan: &JsonDecodePlan,
        child: usize,
        depth: usize,
    ) -> Result<Obj, JsonError> {
        if !self.begin_array("List", depth)? {
            return Ok(Obj::List(ListStorage::new_obj(0)));
        }
        let cap = if depth == 0 && self.bytes.len() > 4096 {
            (self.bytes.len() / 180).clamp(16, 2048)
        } else {
            4
        };
        let mut items = Vec::with_capacity(cap);
        loop {
            let idx = items.len();
            let item = self
                .decode_node(plan, child, depth + 1)
                .map_err(|e| err_at_index(idx, e))?;
            items.push(item);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Obj::List(ListStorage::Obj(Arc::new(items))));
                }
                _ => {
                    return Err(JsonError::new(
                        join_index_path("", idx),
                        "Expected ',' or ']' in JSON array",
                    ));
                }
            }
        }
    }

    fn decode_map(
        &mut self,
        plan: &JsonDecodePlan,
        val_plan: usize,
        val_name: &str,
        depth: usize,
    ) -> Result<Obj, JsonError> {
        self.skip_ws();
        if self.peek() != Some(b'{') {
            return Err(self.wrong_kind(&format!("JSON object for Map<String, {}>", val_name), depth));
        }
        if depth >= MAX_JSON_DEPTH {
            return Err(JsonError::root("Maximum JSON nesting depth (128) exceeded"));
        }
        self.pos += 1;
        self.skip_ws();
        let mut map = OrderedMap::with_capacity(4);
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Obj::Map(Arc::new(map)));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(JsonError::root("Expected string key in JSON object"));
            }
            let key = self.parse_string_cow()?.into_owned();
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(JsonError::new(
                    join_field_path("", &key),
                    "Expected ':' after JSON object key",
                ));
            }
            self.pos += 1;
            let decoded_res = match &plan.nodes[val_plan] {
                PlanNode::Int => self.parse_i64(depth + 1).map(|v| MapVal::Scalar(v as u64)),
                PlanNode::Float => self.parse_f64(depth + 1).map(MapVal::Float),
                PlanNode::Bool => self.parse_bool(depth + 1).map(MapVal::Bool),
                _ => self.decode_node(plan, val_plan, depth + 1).map(MapVal::Obj),
            };
            let decoded = decoded_res.map_err(|e| err_at_field(&key, e))?;
            map.set(MapKey::Str(Arc::new(key.clone().into())), decoded);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Obj::Map(Arc::new(map)));
                }
                _ => {
                    return Err(JsonError::new(
                        join_field_path("", &key),
                        "Expected ',' or '}' in JSON object",
                    ));
                }
            }
        }
    }

    fn decode_tagged_enum(
        &mut self,
        type_name: &str,
        variants: &[(String, CompiledStructDecodePlan)],
        plan: &JsonDecodePlan,
        depth: usize,
    ) -> Result<Obj, JsonError> {
        self.skip_ws();
        if self.peek() != Some(b'{') {
            return Err(self.wrong_kind(&format!("JSON object for '{}'", type_name), depth));
        }
        if depth >= MAX_JSON_DEPTH {
            return Err(JsonError::root("Maximum JSON nesting depth (128) exceeded"));
        }
        let obj_start = self.pos;
        self.pos += 1;

        self.skip_ws();
        if self.peek() == Some(b'}') {
            return Err(JsonError::new(
                join_field_path("", "type"),
                format!("Missing discriminator field 'type' for enum '{}'", type_name),
            ));
        }

        let tag: String;
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(JsonError::root("Expected string key in JSON object"));
            }
            let key = self.parse_string_cow()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(JsonError::new(
                    join_field_path("", key.as_ref()),
                    "Expected ':' after JSON object key",
                ));
            }
            self.pos += 1;
            if key.as_ref() == "type" {
                self.skip_ws();
                if self.peek() != Some(b'"') {
                    return Err(err_at_field(
                        "type",
                        self.wrong_kind("JSON string", depth + 1),
                    ));
                }
                tag = self
                    .parse_string_cow()
                    .map_err(|e| err_at_field("type", e))?
                    .into_owned();
                break;
            }
            self.skip_value(depth + 1)
                .map_err(|e| err_at_field(key.as_ref(), e))?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    return Err(JsonError::new(
                        join_field_path("", "type"),
                        format!("Missing discriminator field 'type' for enum '{}'", type_name),
                    ));
                }
                _ => {
                    return Err(JsonError::new(
                        join_field_path("", key.as_ref()),
                        "Expected ',' or '}' in JSON object",
                    ));
                }
            }
        }

        let Some((_, variant_schema)) = variants.iter().find(|(vname, _)| vname == &tag) else {
            return Err(JsonError::new(
                join_field_path("", "type"),
                format!("Unknown variant '{}' for enum '{}'", tag, type_name),
            ));
        };

        self.pos = obj_start;
        self.decode_struct_inner(variant_schema, plan, depth, true)
    }

    fn decode_struct(
        &mut self,
        schema: &CompiledStructDecodePlan,
        plan: &JsonDecodePlan,
        depth: usize,
    ) -> Result<Obj, JsonError> {
        self.decode_struct_inner(schema, plan, depth, false)
    }

    fn decode_struct_inner(
        &mut self,
        schema: &CompiledStructDecodePlan,
        plan: &JsonDecodePlan,
        depth: usize,
        allow_type_skip: bool,
    ) -> Result<Obj, JsonError> {
        self.skip_ws();
        if self.peek() != Some(b'{') {
            return Err(self.wrong_kind(&format!("JSON object for '{}'", schema.type_name), depth));
        }
        if depth >= MAX_JSON_DEPTH {
            return Err(JsonError::root("Maximum JSON nesting depth (128) exceeded"));
        }
        self.pos += 1;

        let mut rec = StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
        let mut seen_mask: u64 = 0;
        let mut seen_extra = if schema.fields.len() > 64 {
            vec![false; schema.fields.len()]
        } else {
            Vec::new()
        };

        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
        } else {
            let mut cursor = 0usize;
            loop {
                self.skip_ws();
                let field_idx_opt =
                    if !schema.has_duplicate_json_names && cursor < schema.fields.len() {
                        let qkc = &schema.fields[cursor].quoted_key_colon;
                        if self.bytes[self.pos..].starts_with(qkc) {
                            self.pos += qkc.len();
                            let idx = cursor;
                            cursor += 1;
                            Some(idx)
                        } else {
                            self.parse_struct_key_slow(
                                schema,
                                &mut cursor,
                                depth + 1,
                                allow_type_skip,
                            )?
                        }
                    } else {
                        self.parse_struct_key_slow(
                            schema,
                            &mut cursor,
                            depth + 1,
                            allow_type_skip,
                        )?
                    };

                let err_field_name: &str = if let Some(field_idx) = field_idx_opt {
                    if field_idx < 64 {
                        seen_mask |= 1u64 << field_idx;
                    } else {
                        seen_extra[field_idx] = true;
                    }

                    let field = &schema.fields[field_idx];
                    let field_res = match field.kind {
                        FieldDecodeKind::Int(slot) => self.parse_i64(depth + 1).map(|v| {
                            rec.set_scalar(slot as usize, v as u64);
                        }),
                        FieldDecodeKind::Float(slot) => self.parse_f64(depth + 1).map(|v| {
                            rec.set_scalar(slot as usize, v.to_bits());
                        }),
                        FieldDecodeKind::Bool(slot) => self.parse_bool(depth + 1).map(|v| {
                            rec.set_scalar(slot as usize, v as u64);
                        }),
                        FieldDecodeKind::Str(slot) => {
                            self.parse_string_obj(depth + 1).map(|s| {
                                rec.set_obj(slot as usize, s);
                            })
                        }
                        FieldDecodeKind::OptStr(slot) => {
                            self.skip_ws();
                            if self.bytes[self.pos..].starts_with(b"null") {
                                self.pos += 4;
                                rec.set_obj(slot as usize, Obj::Empty);
                                Ok(())
                            } else {
                                self.parse_string_obj(depth + 1).map(|s| {
                                    rec.set_obj(slot as usize, s);
                                })
                            }
                        }
                        FieldDecodeKind::Obj { slot, plan_idx } => {
                            self.decode_node(plan, plan_idx, depth + 1).map(|obj| {
                                rec.set_obj(slot as usize, obj);
                            })
                        }
                    };
                    if let Err(err) = field_res {
                        return Err(err_at_field(&field.json_name, err));
                    }
                    field.json_name.as_str()
                } else {
                    "type"
                };

                self.skip_ws();
                match self.peek() {
                    Some(b',') => {
                        self.pos += 1;
                    }
                    Some(b'}') => {
                        self.pos += 1;
                        break;
                    }
                    _ => {
                        return Err(JsonError::new(
                            join_field_path("", err_field_name),
                            "Expected ',' or '}' in JSON object",
                        ));
                    }
                }
            }
        }

        if (seen_mask & schema.required_mask) != schema.required_mask {
            let missing_idx = (schema.required_mask & !seen_mask).trailing_zeros() as usize;
            let name = &schema.fields[missing_idx].json_name;
            return Err(JsonError::new(
                join_field_path("", name),
                format!("Missing required field '{}'", name),
            ));
        }
        if schema.fields.len() > 64 {
            for (f, seen) in schema.fields[64..].iter().zip(&seen_extra[64..]) {
                if !f.is_optional && !seen {
                    let name = &f.json_name;
                    return Err(JsonError::new(
                        join_field_path("", name),
                        format!("Missing required field '{}'", name),
                    ));
                }
            }
        }
        Ok(Obj::Record(Arc::new(rec)))
    }

    #[cold]
    #[inline(never)]
    fn parse_struct_key_slow(
        &mut self,
        schema: &CompiledStructDecodePlan,
        cursor: &mut usize,
        depth: usize,
        allow_type_skip: bool,
    ) -> Result<Option<usize>, JsonError> {
        if self.peek() != Some(b'"') {
            return Err(JsonError::root("Expected string key in JSON object"));
        }
        let key = self.parse_string_cow()?;
        self.skip_ws();
        if self.peek() != Some(b':') {
            return Err(JsonError::new(
                join_field_path("", key.as_ref()),
                "Expected ':' after JSON object key",
            ));
        }
        self.pos += 1;

        if allow_type_skip && key.as_ref() == "type" {
            self.skip_value(depth)
                .map_err(|e| err_at_field("type", e))?;
            return Ok(None);
        }

        let found = find_field_index(schema, key.as_ref(), cursor);
        if let Some(idx) = found {
            return Ok(Some(idx));
        }
        self.skip_value(depth)
            .map_err(|e| err_at_field(key.as_ref(), e))?;
        Err(JsonError::new(
            join_field_path("", key.as_ref()),
            format!("Unknown field '{}' for type '{}'", key, schema.type_name),
        ))
    }
}

#[cold]
#[inline(never)]
fn find_field_index(
    schema: &CompiledStructDecodePlan,
    key: &str,
    cursor: &mut usize,
) -> Option<usize> {
    if !schema.has_duplicate_json_names && *cursor < schema.fields.len() {
        let field = &schema.fields[*cursor];
        if field.json_name == key {
            let idx = *cursor;
            *cursor += 1;
            return Some(idx);
        }
    }
    let found = if schema.fields.len() <= 8 {
        if schema.has_duplicate_json_names {
            schema.fields.iter().rposition(|f| f.json_name == key)
        } else {
            schema.fields.iter().position(|f| f.json_name == key)
        }
    } else {
        schema.field_indexes.get(key).copied()
    };
    if let Some(idx) = found {
        *cursor = idx + 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_json_decoder_never_panics() {
        let src = r#"
            import * as json from "cdt.json"

            enum Kind { OkVal(code: Int), ErrVal }
            struct Inner { tag: String?, score: Float }
            struct Payload {
                id: Int
                name: String
                flags: [Bool]
                nums: [Int]
                inner: Inner?
                meta: Map<String, String>
                kind: Kind
            }
            export func decode_one(raw: String) -> Payload | json.DecodeError {
                return json.decode<Payload>(raw)
            }
        "#;
        let module = crate::frontend::compile_source(src).unwrap();
        let plan = &module.json_plans[0];

        let seeds: &[&str] = &[
            r#"{"id":1,"name":"a","flags":[true,false],"nums":[1,-2,3],"inner":{"tag":null,"score":1.5},"meta":{"k":"v"},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":9223372036854775807,"name":"x\n\t\"\\","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"ErrVal"}}"#,
            r#"{"id":0,"name":"\u0041\uD83D\uDE00","flags":[false],"nums":[0],"inner":{"tag":"t","score":-0.0},"meta":{},"kind":{"type":"OkVal","code":0}}"#,
        ];

        for &seed in seeds {
            let res = decode_json_with_plan(seed, plan);
            assert!(res.is_ok(), "seed failed `{seed}`: {:?}", res.err());
        }

        // Explicit adversarial inputs.
        let deep_arr = "[".repeat(150) + &"]".repeat(150);
        let deep_obj = "{\"a\":".repeat(150) + "1" + &"}".repeat(150);
        let adversarial = [
            "",
            "{}",
            "null",
            &deep_arr,
            &deep_obj,
            r#"{"id":9223372036854775808,"name":"a","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":-9223372036854775809,"name":"a","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":1,"name":"\uD800","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":1,"name":"\uDC00","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":1,"name":"\uD83D","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":1,"name":"\u","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            "{\"id\":1,\"name\":\"a\0b\",\"flags\":[],\"nums\":[],\"inner\":null,\"meta\":{},\"kind\":{\"type\":\"OkVal\",\"code\":1}}",
            r#"{"id":1,"name":"a","flags":[],"nums":[],"inner":{"tag":"x","score":1e9999999},"meta":{},"kind":{"type":"OkVal","code":1}}"#,
            r#"{"id":1,"name":"a","flags":[],"nums":[],"inner":null,"meta":{},"kind":{"type":"OkVal","code":1}} trailing"#,
        ];
        for raw in adversarial {
            assert!(std::panic::catch_unwind(|| {
                let _ = decode_json_with_plan(raw, plan);
            })
            .is_ok());
        }

        // 2,000 deterministic mutations across bytes/UTF-8 slices of valid JSON seeds.
        let mut rng = 0x517c_c1b7_2722_0a95u64;
        let mut next_u64 = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let alphabet: &[u8] = b"{}[]\":,0123456789-eE.+truefalsenull\\uD800 \n\t";
        for iter in 0..2000usize {
            let mut raw = seeds[iter % seeds.len()].as_bytes().to_vec();
            let op = next_u64() % 4;
            match op {
                0 => {
                    let cut = (next_u64() as usize) % (raw.len() + 1);
                    raw.truncate(cut);
                }
                1 => {
                    let pos = (next_u64() as usize) % raw.len().max(1);
                    raw[pos] = alphabet[(next_u64() as usize) % alphabet.len()];
                }
                2 => {
                    let pos = (next_u64() as usize) % (raw.len() + 1);
                    let extra = alphabet[(next_u64() as usize) % alphabet.len()];
                    raw.insert(pos, extra);
                }
                _ => {
                    let mut out = Vec::with_capacity(96);
                    for _ in 0..((next_u64() % 96) as usize) {
                        out.push(alphabet[(next_u64() as usize) % alphabet.len()]);
                    }
                    raw = out;
                }
            }
            if let Ok(s) = std::str::from_utf8(&raw) {
                let res = std::panic::catch_unwind(|| {
                    let _ = decode_json_with_plan(s, plan);
                });
                assert!(res.is_ok(), "decode_json_with_plan panicked at iter {iter}");
            }
        }
    }
}

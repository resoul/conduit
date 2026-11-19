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

    fn decode_struct(
        &mut self,
        schema: &CompiledStructDecodePlan,
        plan: &JsonDecodePlan,
        depth: usize,
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
                let field_idx = if !schema.has_duplicate_json_names && cursor < schema.fields.len()
                {
                    let qkc = &schema.fields[cursor].quoted_key_colon;
                    if self.bytes[self.pos..].starts_with(qkc) {
                        self.pos += qkc.len();
                        let idx = cursor;
                        cursor += 1;
                        idx
                    } else {
                        self.parse_struct_key_slow(schema, &mut cursor, depth + 1)?
                    }
                } else {
                    self.parse_struct_key_slow(schema, &mut cursor, depth + 1)?
                };

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
                            join_field_path("", &field.json_name),
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
    ) -> Result<usize, JsonError> {
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

        let found = find_field_index(schema, key.as_ref(), cursor);
        if let Some(idx) = found {
            return Ok(idx);
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

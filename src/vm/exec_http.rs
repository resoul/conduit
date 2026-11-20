use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::ir::Module;
use crate::native::http::{
    MatchedParams, ReadHttpError, RouteEntry, RouterState, http_client_bytes_request,
    http_client_request, http_client_request_with_timeout, lookup_route, make_http_error,
    make_request_obj, make_response_obj, make_response_with_body_obj, make_router_obj,
    read_http_request, reader_writer_pair, route_prefix_matches, try_serve_static,
    write_error_response, write_http_response_ext,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_HTTP_REQUEST, SCHEMA_HTTP_RESPONSE,
    SCHEMA_HTTP_ROUTER, SMALL_MAP_LIMIT, StructData,
};
use crate::vm::{ExecResult, RuntimeError, Vm};

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub const HTTP_TEXT: u8 = 0;
pub const HTTP_HTML: u8 = 1;
pub const HTTP_JSON: u8 = 2;
pub const HTTP_RESPONSE: u8 = 3;
pub const HTTP_ROUTER: u8 = 4;
pub const HTTP_ROUTE_GET: u8 = 5;
pub const HTTP_ROUTE_POST: u8 = 6;
pub const HTTP_ROUTE_PUT: u8 = 7;
pub const HTTP_ROUTE_DELETE: u8 = 8;
pub const HTTP_DISPATCH: u8 = 9;
pub const HTTP_SERVE: u8 = 10;
pub const HTTP_GET: u8 = 11;
pub const HTTP_POST: u8 = 12;
pub const HTTP_REQUEST: u8 = 13;
pub const HTTP_REQUEST_TIMEOUT: u8 = 14;
pub const HTTP_ROUTE_PATCH: u8 = 15;
pub const HTTP_ROUTE_OPTIONS: u8 = 16;
pub const HTTP_ROUTE_HEAD: u8 = 17;
pub const HTTP_ROUTE_ANY: u8 = 18;
pub const HTTP_ROUTE_BEFORE: u8 = 19;
pub const HTTP_ROUTE_AFTER: u8 = 20;
pub const HTTP_GET_BYTES: u8 = 21;
pub const HTTP_REQUEST_BYTES: u8 = 22;
pub const HTTP_STOP: u8 = 23;
pub const HTTP_ROUTE_TYPED_BODY: u8 = 24;
pub const HTTP_ROUTE_GROUP_BEFORE: u8 = 25;
pub const HTTP_ROUTE_GROUP_AFTER: u8 = 26;
pub const HTTP_ROUTE_STATIC: u8 = 27;
pub const HTTP_ROUTE_CORS: u8 = 28;
pub const HTTP_OK_JSON: u8 = 29;
pub const HTTP_STATUS_JSON: u8 = 30;
pub const HTTP_REDIRECT: u8 = 31;
pub const HTTP_NO_CONTENT: u8 = 32;
pub const HTTP_REQ_HEADER: u8 = 33;
pub const HTTP_REQ_PARAM: u8 = 34;
pub const HTTP_REQ_QUERY_PARAM: u8 = 35;
pub const HTTP_GET_JSON: u8 = 36;
pub const HTTP_POST_JSON: u8 = 37;

#[inline]
fn populate_ordered_map_from_params(map: &mut OrderedMap, params: MatchedParams) {
    map.entries.clear();
    if !map.index.is_empty() {
        map.index.clear();
    }
    map.dense_ints = false;
    match params {
        MatchedParams::None => {}
        MatchedParams::One(k, v) => {
            map.entries.push((MapKey::Str(k), MapVal::Obj(v)));
        }
        MatchedParams::Many(vec) => {
            map.entries.extend(vec);
            if map.entries.len() > SMALL_MAP_LIMIT {
                map.materialize_index();
            }
        }
    }
}

#[inline]
fn set_matched_params_on_record(req_mut: &mut StructData, params: MatchedParams) {
    if let Obj::Map(m_arc) = req_mut.get_obj_mut(5)
        && let Some(map) = Arc::get_mut(m_arc)
    {
        populate_ordered_map_from_params(map, params);
        return;
    }
    let mut map = match &params {
        MatchedParams::One(_, _) => OrderedMap::with_capacity(1),
        MatchedParams::Many(vec) => OrderedMap::with_capacity(vec.len()),
        MatchedParams::None => OrderedMap::new(),
    };
    populate_ordered_map_from_params(&mut map, params);
    req_mut.set_obj(5, Obj::Map(Arc::new(map)));
}

impl Vm {
    fn invoke_http_fn(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        handler: &Obj,
        args: &[Obj],
    ) -> Result<Obj, RuntimeError> {
        let (func_id, has_closure) = match handler {
            Obj::Func(fid) => (*fid, false),
            Obj::Closure(cd) => (cd.func_id, true),
            _ => return Err(err_msg("Expected HTTP handler function")),
        };
        let closure_slot = if has_closure {
            self.ensure_capacity(s_top, o_top + 1);
            self.objs[o_top] = handler.clone();
            o_top
        } else {
            usize::MAX
        };
        let child_s = s_top;
        let child_o = if has_closure { o_top + 1 } else { o_top };
        let func = &module.functions[func_id as usize];
        let used_objs = (func.num_objs as usize).max(args.len()).max(1);
        self.ensure_capacity(
            child_s + func.num_scalars.max(1) as usize,
            child_o + used_objs,
        );
        for (i, arg) in args.iter().enumerate() {
            self.objs[child_o + i] = arg.clone();
        }
        let res = self.exec_from(module, func_id, child_s, child_o, closure_slot);
        if has_closure {
            self.objs[closure_slot] = Obj::Empty;
        }
        match res {
            Ok(ExecResult::Obj(o)) => {
                if used_objs > func.num_objs as usize {
                    self.clear_obj_range(
                        child_o + func.num_objs as usize,
                        used_objs - func.num_objs as usize,
                    );
                }
                Ok(o)
            }
            Ok(_) => Err(err_msg("HTTP handler must return an object")),
            Err(e) => {
                self.clear_obj_range(child_o, used_objs);
                Err(e)
            }
        }
    }

    fn extract_router_id(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected Router"));
        };
        if rec.schema_id != SCHEMA_HTTP_ROUTER {
            return Err(err_msg("Expected Router"));
        }
        let id = rec.get_scalar(0) as usize;
        if id >= self.host.routers.len() {
            return Err(err_msg("Invalid Router handle"));
        }
        Ok(id)
    }

    fn extract_response_fields<'a>(
        &self,
        obj: &'a Obj,
    ) -> Result<(i64, &'a str, Option<&'a OrderedMap>), RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected Response"));
        };
        if rec.schema_id != SCHEMA_HTTP_RESPONSE {
            return Err(err_msg("Expected Response"));
        }
        let status = rec.get_scalar(0) as i64;
        let body = match rec.get_obj(0) {
            Obj::Str(s) => s.as_str(),
            _ => "",
        };
        let headers = match rec.get_obj(1) {
            Obj::Map(m) => Some(m.as_ref()),
            _ => None,
        };
        Ok((status, body, headers))
    }

    fn dispatch_router_request(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        router_id: usize,
        mut req_obj: Obj,
    ) -> Result<Obj, RuntimeError> {
        let (req_method, req_path) = match &req_obj {
            Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_REQUEST => {
                let m = match rec.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => "GET".to_string(),
                };
                let p = match rec.get_obj(1) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => "/".to_string(),
                };
                (m, p)
            }
            _ => return Err(err_msg("Expected Request")),
        };

        let cors_origin = self.host.routers[router_id].cors_origin.clone();
        let mut resp = if req_method == "OPTIONS"
            && cors_origin.is_some()
            && lookup_route(&self.host.routers[router_id].routes, &req_method, &req_path).is_err()
        {
            let mut hdrs = OrderedMap::with_capacity(3);
            hdrs.set(
                MapKey::Str(Arc::new("access-control-allow-methods".into())),
                MapVal::Obj(Obj::str("GET, POST, PUT, PATCH, DELETE, OPTIONS")),
            );
            hdrs.set(
                MapKey::Str(Arc::new("access-control-allow-headers".into())),
                MapVal::Obj(Obj::str("Content-Type, Authorization")),
            );
            let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
            s.set_scalar(0, 204);
            s.set_obj(0, Obj::str(""));
            s.set_obj(1, Obj::Map(Arc::new(hdrs)));
            Some(Obj::Record(Arc::new(s)))
        } else {
            None
        };
        if resp.is_none() {
            let before_len = self.host.routers[router_id].before.len();
            for idx in 0..before_len {
                let hook = {
                    let (prefix, h) = &self.host.routers[router_id].before[idx];
                    if let Some(p) = prefix
                        && !route_prefix_matches(p, &req_path)
                    {
                        continue;
                    }
                    h.clone()
                };
                let out = self.invoke_http_fn(
                    module,
                    s_top,
                    o_top,
                    &hook,
                    std::slice::from_ref(&req_obj),
                )?;
                if out != Obj::Empty {
                    resp = Some(out);
                    break;
                }
            }
        }

        let mut cached_to_restore: Option<(Arc<StructData>, Arc<StructData>)> = None;
        let mut resp = match resp {
            Some(r) => r,
            None => {
                let matched = {
                    let routes = &self.host.routers[router_id].routes;
                    match lookup_route(routes, &req_method, &req_path) {
                        Ok((entry, params)) => {
                            Ok((entry.handler.clone(), entry.body_json_plan, params))
                        }
                        Err(status) => Err(status),
                    }
                };
                match matched {
                    Ok((handler, body_plan, MatchedParams::None)) => self.invoke_http_route_handler(
                        module,
                        s_top,
                        o_top,
                        &handler,
                        body_plan,
                        &req_obj,
                    )?,
                    Ok((handler, body_plan, matched_params)) => {
                        let mut cached = self.host.cached_http_req.take();
                        let Obj::Record(ref mut rec_arc) = req_obj else {
                            return Err(err_msg("Expected Request"));
                        };
                        if let Some((ref mut last_src, ref mut cached_rec)) = cached
                            && let Some(cached_mut) = Arc::get_mut(cached_rec)
                        {
                            if !Arc::ptr_eq(last_src, rec_arc) {
                                for idx in 0..5 {
                                    cached_mut.set_obj(idx, rec_arc.get_obj(idx).clone());
                                }
                                *last_src = Arc::clone(rec_arc);
                            }
                            set_matched_params_on_record(cached_mut, matched_params);
                            req_obj = Obj::Record(Arc::clone(cached_rec));
                        } else {
                            let src_arc = Arc::clone(rec_arc);
                            let req_mut = Arc::make_mut(rec_arc);
                            set_matched_params_on_record(req_mut, matched_params);
                            if let Obj::Record(new_arc) = &req_obj {
                                cached = Some((src_arc, Arc::clone(new_arc)));
                            }
                        }
                        cached_to_restore = cached;
                        self.invoke_http_route_handler(
                            module,
                            s_top,
                            o_top,
                            &handler,
                            body_plan,
                            &req_obj,
                        )?
                    }
                    Err(status) => {
                        if let Some(static_resp) = try_serve_static(
                            &self.host.routers[router_id].statics,
                            &req_method,
                            &req_path,
                        ) {
                            static_resp
                        } else if status == 405 {
                            make_response_obj(405, "Method Not Allowed", Some("text/plain; charset=utf-8"))
                        } else {
                            make_response_obj(404, "Not Found", Some("text/plain; charset=utf-8"))
                        }
                    }
                }
            }
        };

        let after_len = self.host.routers[router_id].after.len();
        for idx in 0..after_len {
            let hook = {
                let (prefix, h) = &self.host.routers[router_id].after[idx];
                if let Some(p) = prefix
                    && !route_prefix_matches(p, &req_path)
                {
                    continue;
                }
                h.clone()
            };
            resp = self.invoke_http_fn(
                module,
                s_top,
                o_top,
                &hook,
                &[req_obj.clone(), resp],
            )?;
        }
        if let Some(origin) = cors_origin
            && let Obj::Record(ref mut rec_arc) = resp
            && rec_arc.schema_id == SCHEMA_HTTP_RESPONSE
        {
            let rec_mut = Arc::make_mut(rec_arc);
            let mut hdrs = match rec_mut.get_obj(1) {
                Obj::Map(m) => (**m).clone(),
                _ => OrderedMap::with_capacity(1),
            };
            hdrs.set(
                MapKey::Str(Arc::new("access-control-allow-origin".into())),
                MapVal::Obj(Obj::str(origin.as_ref())),
            );
            rec_mut.set_obj(1, Obj::Map(Arc::new(hdrs)));
        }
        if cached_to_restore.is_some() {
            self.host.cached_http_req = cached_to_restore;
        }
        Ok(resp)
    }

    fn invoke_http_route_handler(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        handler: &Obj,
        body_json_plan: Option<u16>,
        req_obj: &Obj,
    ) -> Result<Obj, RuntimeError> {
        let Some(plan_idx) = body_json_plan else {
            return self.invoke_http_fn(module, s_top, o_top, handler, std::slice::from_ref(req_obj));
        };
        let body_str = match req_obj {
            Obj::Record(rec) => match rec.get_obj(2) {
                Obj::Str(s) => s.as_str().to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let decoded = match crate::native::json_decode::decode_json_with_plan(
            &body_str,
            &module.json_plans[plan_idx as usize],
        ) {
            Ok(v) => v,
            Err(e) => {
                let msg = json_escape_str(&e.message);
                return Ok(make_response_obj(
                    400,
                    format!(r#"{{"error":"invalid_json","message":"{}"}}"#, msg),
                    Some("application/json; charset=utf-8"),
                ));
            }
        };
        let mut violations = Vec::new();
        self.validate_dyn_val(module, s_top, o_top, &decoded, "", &mut violations, 0)?;
        if !violations.is_empty() {
            let mut body = String::from(r#"{"error":"validation_failed","issues":["#);
            for (idx, v) in violations.iter().enumerate() {
                if idx > 0 {
                    body.push(',');
                }
                if let Obj::Record(rec) = v {
                    let path = match rec.get_obj(0) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    };
                    let code = match rec.get_obj(1) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    };
                    let msg = match rec.get_obj(2) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    };
                    body.push_str(&format!(
                        r#"{{"path":"{}","code":"{}","message":"{}"}}"#,
                        json_escape_str(path),
                        json_escape_str(code),
                        json_escape_str(msg),
                    ));
                }
            }
            body.push_str("]}");
            return Ok(make_response_obj(
                422,
                &body,
                Some("application/json; charset=utf-8"),
            ));
        }
        self.invoke_http_fn(module, s_top, o_top, handler, &[req_obj.clone(), decoded])
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_http_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            HTTP_TEXT => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.text()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("text/plain; charset=utf-8"));
            }
            HTTP_HTML => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.html()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("text/html; charset=utf-8"));
            }
            HTTP_JSON => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.json()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("application/json"));
            }
            HTTP_RESPONSE => {
                let status = self.scalars[s_base + b as usize] as i64;
                let body = match &self.objs[o_base + c as usize] {
                    Obj::Str(_) => self.objs[o_base + c as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.response()")),
                };
                self.objs[o_base + a as usize] = make_response_with_body_obj(status, body, None);
            }
            HTTP_ROUTER => {
                let id = self.host.routers.len();
                self.host.routers.push(RouterState::default());
                self.objs[o_base + a as usize] = make_router_obj(id as i64);
            }
            HTTP_ROUTE_GET
            | HTTP_ROUTE_POST
            | HTTP_ROUTE_PUT
            | HTTP_ROUTE_DELETE
            | HTTP_ROUTE_PATCH
            | HTTP_ROUTE_OPTIONS
            | HTTP_ROUTE_HEAD
            | HTTP_ROUTE_ANY => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let pattern = self.sys_str(o_base, b, "router.route()")?;
                let handler = self.objs[o_base + c as usize].clone();
                let method = match aux {
                    HTTP_ROUTE_GET => "GET",
                    HTTP_ROUTE_POST => "POST",
                    HTTP_ROUTE_PUT => "PUT",
                    HTTP_ROUTE_DELETE => "DELETE",
                    HTTP_ROUTE_PATCH => "PATCH",
                    HTTP_ROUTE_OPTIONS => "OPTIONS",
                    HTTP_ROUTE_HEAD => "HEAD",
                    _ => "*",
                };
                self.host.routers[rid]
                    .routes
                    .push(RouteEntry::new(method, &pattern, handler));
            }
            HTTP_ROUTE_BEFORE => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].before.push((None, handler));
            }
            HTTP_ROUTE_AFTER => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].after.push((None, handler));
            }
            HTTP_ROUTE_GROUP_BEFORE => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let prefix = self.sys_str(o_base, b, "router group before")?.as_str().into();
                let handler = self.objs[o_base + c as usize].clone();
                self.host.routers[rid].before.push((Some(prefix), handler));
            }
            HTTP_ROUTE_GROUP_AFTER => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let prefix = self.sys_str(o_base, b, "router group after")?.as_str().into();
                let handler = self.objs[o_base + c as usize].clone();
                self.host.routers[rid].after.push((Some(prefix), handler));
            }
            HTTP_ROUTE_STATIC => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let url_prefix = self.sys_str(o_base, b, "router static")?.as_str().into();
                let dir_path = self.sys_str(o_base, c, "router static")?.as_str().into();
                self.host.routers[rid].statics.push((url_prefix, dir_path));
            }
            HTTP_ROUTE_CORS => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let origin = self.sys_str(o_base, b, "router cors")?.as_str().into();
                self.host.routers[rid].cors_origin = Some(origin);
            }
            HTTP_ROUTE_TYPED_BODY => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                if let Some(last) = self.host.routers[rid].routes.last_mut() {
                    last.body_json_plan = Some(c);
                }
            }
            HTTP_OK_JSON | HTTP_STATUS_JSON => {
                let (status, val) = if aux == HTTP_OK_JSON {
                    (200i64, &self.objs[o_base + b as usize])
                } else {
                    (
                        self.scalars[s_base + b as usize] as i64,
                        &self.objs[o_base + c as usize],
                    )
                };
                let resp = match crate::native::json_encode::encode_json_obj(val, &module.struct_schemas) {
                    Ok(json_str) => make_response_obj(status, json_str, Some("application/json; charset=utf-8")),
                    Err(e) => make_response_obj(
                        500,
                        format!(r#"{{"error":"encode_error","message":"{}"}}"#, json_escape_str(&e.message)),
                        Some("application/json; charset=utf-8"),
                    ),
                };
                self.objs[o_base + a as usize] = resp;
            }
            HTTP_REDIRECT => {
                let status = self.scalars[s_base + b as usize] as i64;
                let url = self.sys_str(o_base, c, "http.redirect()")?;
                let mut hdrs = OrderedMap::with_capacity(1);
                hdrs.set(
                    MapKey::Str(Arc::new("location".into())),
                    MapVal::Obj(Obj::Str(url)),
                );
                let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
                s.set_scalar(0, status as u64);
                s.set_obj(0, Obj::str(""));
                s.set_obj(1, Obj::Map(Arc::new(hdrs)));
                self.objs[o_base + a as usize] = Obj::Record(Arc::new(s));
            }
            HTTP_NO_CONTENT => {
                self.objs[o_base + a as usize] = make_response_obj(204, "", None);
            }
            HTTP_REQ_HEADER | HTTP_REQ_PARAM | HTTP_REQ_QUERY_PARAM => {
                let Obj::Record(rec) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected http.Request"));
                };
                let key = self.sys_str(o_base, c, "Request lookup")?;
                let field_idx = match aux {
                    HTTP_REQ_HEADER => 3,
                    HTTP_REQ_QUERY_PARAM => 4,
                    _ => 5,
                };
                let found = match rec.get_obj(field_idx) {
                    Obj::Map(map) => {
                        if aux == HTTP_REQ_HEADER {
                            let lower = key.as_str().to_ascii_lowercase();
                            map.get(&MapKey::Str(Arc::new(lower.as_str().into())))
                                .or_else(|| {
                                    map.entries.iter().find_map(|(k, v)| match k {
                                        MapKey::Str(ks) if ks.as_str().eq_ignore_ascii_case(key.as_str()) => Some(v),
                                        _ => None,
                                    })
                                })
                                .and_then(|v| match v {
                                    MapVal::Obj(o) => Some(o.clone()),
                                    _ => None,
                                })
                        } else {
                            map.get(&MapKey::Str(key)).and_then(|v| match v {
                                MapVal::Obj(o) => Some(o.clone()),
                                _ => None,
                            })
                        }
                    }
                    _ => None,
                };
                self.objs[o_base + a as usize] = found.unwrap_or(Obj::Empty);
            }
            HTTP_GET_JSON => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed get_json args"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let mut url = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String URL in http.get_json()")),
                };
                if let Obj::Map(qmap) = packed.get_obj(1)
                    && !qmap.is_empty()
                    && let Obj::Str(qs) = crate::native::url::encode_query_map(qmap)
                {
                    if url.contains('?') {
                        url.push('&');
                    } else {
                        url.push('?');
                    }
                    url.push_str(qs.as_str());
                }
                let hdrs = vec![("Accept".to_string(), "application/json".to_string())];
                let resp = http_client_request("GET", &url, "", &hdrs);
                let out = match &resp {
                    Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_RESPONSE => {
                        let body = match rec.get_obj(0) {
                            Obj::Str(s) => s.as_str(),
                            _ => "",
                        };
                        match crate::native::json_decode::decode_json_with_plan(
                            body,
                            &module.json_plans[plan_idx],
                        ) {
                            Ok(v) => v,
                            Err(e) => make_http_error("decode_error", &e.message),
                        }
                    }
                    _ => resp,
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_POST_JSON => {
                let url = self.sys_str(o_base, b, "http.post_json()")?;
                let val = &self.objs[o_base + c as usize];
                let out = match crate::native::json_encode::encode_json_obj(val, &module.struct_schemas) {
                    Ok(body) => {
                        let hdrs = vec![
                            ("Content-Type".to_string(), "application/json".to_string()),
                            ("Accept".to_string(), "application/json".to_string()),
                        ];
                        http_client_request("POST", url.as_str(), &body, &hdrs)
                    }
                    Err(e) => make_http_error("encode_error", &e.message),
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_DISPATCH => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                let req_obj = self.objs[o_base + c as usize].clone();
                let resp = self.dispatch_router_request(module, s_top, o_top, rid, req_obj)?;
                self.objs[o_base + a as usize] = resp;
            }
            HTTP_SERVE => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                let addr = self.sys_str(o_base, c, "router.serve()")?;
                let listener = match TcpListener::bind(addr.as_str()) {
                    Ok(l) => l,
                    Err(e) => {
                        self.objs[o_base + a as usize] = make_http_error("bind_error", e.to_string());
                        return Ok(());
                    }
                };
                if let Ok(local) = listener.local_addr()
                    && let Ok(mut guard) = self.host.routers[rid].listen_addr.lock()
                {
                    *guard = Some(local);
                }
                self.host.routers[rid].stop_flag.store(false, Ordering::Release);
                let stop_flag = Arc::clone(&self.host.routers[rid].stop_flag);
                let router_state = self.host.routers[rid].clone();
                let script_args = self.script_args.clone();
                let workers = std::thread::available_parallelism()
                    .map(|n| n.get().clamp(4, 16))
                    .unwrap_or(8);
                let (tx, rx) = mpsc::sync_channel::<TcpStream>(workers * 8);
                let rx = Arc::new(Mutex::new(rx));
                std::thread::scope(|scope| {
                    for _ in 0..workers {
                        let rx = Arc::clone(&rx);
                        let router_state = router_state.clone();
                        let script_args = script_args.clone();
                        scope.spawn(move || {
                            let mut worker_vm = Vm::new();
                            worker_vm.script_args = script_args;
                            worker_vm.host.routers.push(router_state);
                            loop {
                                let stream = {
                                    let Ok(guard) = rx.lock() else { break };
                                    match guard.recv() {
                                        Ok(s) => s,
                                        Err(_) => break,
                                    }
                                };
                                serve_connection(&mut worker_vm, module, 0, stream);
                            }
                        });
                    }
                    while !stop_flag.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if stop_flag.load(Ordering::Acquire) {
                                    break;
                                }
                                if tx.send(stream).is_err() {
                                    break;
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    drop(tx);
                });
                if let Ok(mut guard) = self.host.routers[rid].listen_addr.lock() {
                    *guard = None;
                }
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            HTTP_STOP => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                self.host.routers[rid].stop_flag.store(true, Ordering::Release);
                if let Ok(guard) = self.host.routers[rid].listen_addr.lock()
                    && let Some(mut addr) = *guard
                {
                    if addr.ip().is_unspecified() {
                        addr.set_ip(if addr.is_ipv4() {
                            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                        } else {
                            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
                        });
                    }
                    let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(50));
                }
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            HTTP_GET => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.get expects String url")),
                };
                self.objs[o_base + a as usize] = http_client_request("GET", &url, "", &[]);
            }
            HTTP_POST => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.post expects String url")),
                };
                let body = match &self.objs[o_base + c as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.post expects String body")),
                };
                self.objs[o_base + a as usize] = http_client_request("POST", &url, &body, &[]);
            }
            HTTP_REQUEST | HTTP_REQUEST_TIMEOUT => {
                let req = extract_packed_request(&self.objs[o_base + b as usize])?;
                self.objs[o_base + a as usize] = if aux == HTTP_REQUEST_TIMEOUT {
                    let timeout_ms = self.scalars[s_base + c as usize] as i64;
                    http_client_request_with_timeout(
                        &req.method,
                        &req.url,
                        &req.body,
                        &req.headers,
                        timeout_ms,
                    )
                } else {
                    http_client_request(&req.method, &req.url, &req.body, &req.headers)
                };
            }
            HTTP_GET_BYTES => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.get_bytes expects String url")),
                };
                self.objs[o_base + a as usize] =
                    http_client_bytes_request("GET", &url, &[], &[], 30_000);
            }
            HTTP_REQUEST_BYTES => {
                let Obj::Record(sd) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("http.request_bytes expects packed request args"));
                };
                if sd.schema_id != SCHEMA_HTTP_REQUEST {
                    return Err(err_msg("http.request_bytes expects packed request args"));
                }
                let method = match sd.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.request_bytes expects String method")),
                };
                let url = match sd.get_obj(1) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.request_bytes expects String url")),
                };
                let mut body_bytes = Vec::new();
                if let Obj::List(ListStorage::Int(items)) = sd.get_obj(2) {
                    for &v in items.iter() {
                        match u8::try_from(v) {
                            Ok(byte) => body_bytes.push(byte),
                            Err(_) => {
                                self.objs[o_base + a as usize] = make_http_error(
                                    "invalid_body",
                                    format!("byte value {} out of range 0..=255", v),
                                );
                                return Ok(());
                            }
                        }
                    }
                }
                let hdrs = extract_header_pairs(sd.get_obj(3));
                self.objs[o_base + a as usize] =
                    http_client_bytes_request(&method, &url, &body_bytes, &hdrs, 30_000);
            }
            _ => return Err(err_msg("Unknown http sub-op")),
        }
        Ok(())
    }
}

struct PackedRequestArgs {
    method: String,
    url: String,
    body: String,
    headers: Vec<(String, String)>,
}

fn extract_packed_request(obj: &Obj) -> Result<PackedRequestArgs, RuntimeError> {
    let Obj::Record(sd) = obj else {
        return Err(err_msg("http.request expects packed request args"));
    };
    if sd.schema_id != SCHEMA_HTTP_REQUEST {
        return Err(err_msg("http.request expects packed request args"));
    }
    let method = match sd.get_obj(0) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String method")),
    };
    let url = match sd.get_obj(1) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String url")),
    };
    let body = match sd.get_obj(2) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String body")),
    };
    let hdrs = extract_header_pairs(sd.get_obj(3));
    Ok(PackedRequestArgs {
        method,
        url,
        body,
        headers: hdrs,
    })
}

fn extract_header_pairs(obj: &Obj) -> Vec<(String, String)> {
    let mut hdrs = Vec::new();
    if let Obj::Map(m) = obj {
        for (k, v) in m.entries.iter() {
            if let (crate::vm::value::MapKey::Str(ks), crate::vm::value::MapVal::Obj(Obj::Str(vs))) = (k, v) {
                hdrs.push((ks.as_str().to_string(), vs.as_str().to_string()));
            }
        }
    }
    hdrs
}

fn serve_connection(vm: &mut Vm, module: &Module, rid: usize, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Ok((mut reader, mut writer)) = reader_writer_pair(stream) else {
        return;
    };
    let mut first_req = true;
    loop {
        if !first_req {
            // Keep-alive idle timeout to prevent slowloris / connection exhaustion
            let _ = reader.get_ref().set_read_timeout(Some(Duration::from_secs(5)));
        }
        first_req = false;
        match read_http_request(&mut reader) {
            Ok(raw) => {
                let head_only = raw.method.eq_ignore_ascii_case("HEAD");
                let req_obj = make_request_obj(
                    &raw.method,
                    &raw.path,
                    raw.body,
                    raw.headers,
                    raw.query,
                    Vec::new(),
                );
                match vm.dispatch_router_request(module, 0, 0, rid, req_obj) {
                    Ok(resp_obj) => {
                        let Ok((status, body, headers)) = vm.extract_response_fields(&resp_obj) else {
                            write_error_response(&mut writer, 500, "Internal Server Error");
                            break;
                        };
                        if write_http_response_ext(
                            &mut writer,
                            status,
                            body,
                            headers,
                            raw.keep_alive,
                            head_only,
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        write_error_response(&mut writer, 500, "Internal Server Error");
                        break;
                    }
                }
                if !raw.keep_alive || vm.host.routers[rid].stop_flag.load(Ordering::Acquire) {
                    break;
                }
            }
            Err(ReadHttpError::Closed) => break,
            Err(ReadHttpError::TooLarge) => {
                write_error_response(&mut writer, 413, "Payload Too Large");
                break;
            }
            Err(ReadHttpError::Bad(msg)) => {
                write_error_response(&mut writer, 400, &msg);
                break;
            }
            Err(ReadHttpError::Io(_)) => break,
        }
    }
}

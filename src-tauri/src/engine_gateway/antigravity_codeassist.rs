//! Google Code Assist（Antigravity 账号）的线协议·请求侧。照 magpie
//! `internal/gateway/codeassist.go`（vendored v0.1.4514；v0.1.4824 多出的
//! 「Gemini 4 的 xhigh」一条已并入）逐条移植，函数旁标了 Go 行号：
//!
//! - [`plain_schema`]：JSON schema 精简成 Antigravity 函数声明收的子集（:1085）；
//! - 系统提示词改写（:52–:330）：Antigravity 对某些 agent 的身份行一律回
//!   429「Resource has been exhausted」，与额度无关；magpie 把那些行换成同义
//!   写法再发（cloak）；
//! - [`read_chat`] + [`build_request`]：OpenAI Chat 请求体 → Gemini `request`
//!   （Go readChat，chat.go:119 + buildCodeAssistSent，:547）；
//! - [`envelope`]：套信封（provider/google.go:1686）；
//! - 正文里写出来的调用（`call:default_api:tool{…}`）的解析（:1889–:2032），
//!   应答解码器 `antigravity_stream.rs` 用。
//!
//! 应答侧在 `antigravity_stream.rs`；账号侧（凭据、token、模型目录、intercept）
//! 在 `antigravity_account.rs`。
//!
//! ## 与 Go 的差距，如实列出
//!
//! - **思考签名不回传**：Go 把 Gemini 给的 thoughtSignature 藏在调用 id 里
//!   （gemini_signature.go 的 signedID / unsignCalls），下一轮带回去；这里每个
//!   调用都带 `skip_thought_signature_validator`（Go 对不带签名的客户端也是
//!   这么发的，#1445）。
//! - **dropPseudoCalls 没搬**（textcalls.go:144：模型把调用写进正文、后面又
//!   写了结果的那段历史重写）。
//! - **图片类型转换没搬**：Go 能把 GIF 之类它读得了的图转成 PNG 再发
//!   （gemini_media.go:28），这里读不了的类型发一行说明文字。
//! - **Google 自家的其它工具**（url_context、code_execution 作为 tool 传入）没搬；
//!   `web_search` 照发 `googleSearch`。
//! - **PromptAsWritten**（用户要求系统提示词原样发）没有这个开关：恒改写。
//! - 响应里的图片模型产物 / 搜索引用在应答侧，见那边的清单。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

// =====================================================================
// 名字（codeassist.go:1792–1828）
// =====================================================================

/// Gemini 自家工具所在的命名空间；模型偶尔连带它一起点名。
pub(crate) const GEMINI_NAMESPACE: &str = "default_api";
/// 正文里写出来的调用的开头。
pub(crate) const CALL_MARKER: &str = "call:default_api:";
/// 工具名不以字母/下划线开头时加的前导（Google 只收那种函数名）。
const NAME_LEAD: &str = "_x_";
/// 代替「这一轮不是在本引擎里做的调用」的思考签名：Google 校验 Gemini 3 发
/// 出去的签名，这个常量告诉它别校验（codeassist.go:33）。
const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

/// 工具名按 Google 收的写法（geminiFuncName，:1812）：数字、`-`、`.`、`:` 开头
/// 的名字加 `_x_` 前缀，否则整个请求被「Request contains an invalid argument」拒掉。
pub(crate) fn gemini_func_name(n: &str) -> String {
    if matches!(n.bytes().next(), Some(b'0'..=b'9' | b'-' | b'.' | b':')) {
        format!("{NAME_LEAD}{n}")
    } else {
        n.to_string()
    }
}

/// 调用里的工具名去掉 Gemini 的命名空间（toolOfCall，:1820）。
pub(crate) fn tool_of_call(name: &str) -> String {
    let mut name = name;
    for sep in [":", "."] {
        let namespace = format!("{GEMINI_NAMESPACE}{sep}");
        if let Some(t) = name.strip_prefix(namespace.as_str()) {
            if !t.is_empty() {
                name = t;
                break;
            }
        }
    }
    name.strip_prefix(NAME_LEAD).unwrap_or(name).to_string()
}

/// Antigravity 的调用 id 只收这些字符（unsafeToolID，:35），其余换 `_`。
fn tool_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// =====================================================================
// plainSchema（:1085）：JSON schema → Antigravity 函数声明收的子集
// =====================================================================

const MAX_DEPTH: usize = 32;

struct Schema<'a> {
    root: &'a Value,
    /// `$defs` / `definitions` 里的名字 → schema。
    defs: HashMap<&'a str, &'a Value>,
}

fn is_set(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !v.is_null())
}

/// 一个 JSON schema 精简成 Antigravity 的函数声明收的样子：没有引用、没有
/// 联合、没有子集之外的关键字（OpenAPI 风格）。
pub(crate) fn plain_schema(raw: &Value) -> Value {
    if !raw.is_object() {
        // Go：解不进 map 就原样返回；`null` 解得进去、得空表 → `{}`。
        return if raw.is_null() {
            json!({})
        } else {
            raw.clone()
        };
    }
    let mut defs = HashMap::new();
    for key in ["$defs", "definitions"] {
        if let Some(d) = raw.get(key).and_then(Value::as_object) {
            for (name, v) in d {
                defs.insert(name.as_str(), v);
            }
        }
    }
    let schema = Schema { root: raw, defs };
    let mut expanding = HashSet::new();
    walk(raw, 0, "", &schema, &mut expanding)
}

impl<'a> Schema<'a> {
    /// 本地 `$ref` 指向的 schema（localRef，:1102）：JSON 指针，或 `$defs` /
    /// `definitions` 里的名字。给的是一份拷贝——`$ref` 的目标都是小对象。
    fn local_ref(&self, r: &str) -> Option<Value> {
        if let Some(rest) = r.strip_prefix("#/") {
            let mut cur: Option<&Value> = Some(self.root);
            for token in rest.split('/') {
                let token = token.replace("~1", "/").replace("~0", "~");
                cur = match cur {
                    Some(Value::Object(m)) => m.get(&token),
                    Some(Value::Array(a)) => token
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n < a.len())
                        .and_then(|n| a.get(n)),
                    _ => return None,
                };
                // 指针指到的地方什么都没有：最后一段当 `$defs` 的名字再找一次。
                if cur.is_none() {
                    let name = r.rsplit('/').next().unwrap_or(r);
                    return self.defs.get(name).map(|d| (*d).clone());
                }
            }
            return cur.cloned();
        }
        if r == "#" {
            return Some(self.root.clone());
        }
        let name = r.rsplit('/').next().unwrap_or(r);
        self.defs.get(name).map(|d| (*d).clone())
    }

    /// 成员本身，或它的 `$ref` 指向的 schema（resolve，:1137）；不是 schema = None。
    fn resolve(&self, v: Option<&Value>) -> Option<Map<String, Value>> {
        let m = v?.as_object()?;
        match m.get("$ref").and_then(Value::as_str) {
            Some(r) => self.local_ref(r).and_then(|d| d.as_object().cloned()),
            None => Some(m.clone()),
        }
    }

    /// 有自己形状的 schema 把 anyOf / oneOf 并进来（foldUnion，:1542）：它自己
    /// 说的算数；null 分支 = 可空；对象上，只有分支里声明过的字段并进 properties
    /// （第一个声明它的分支给的），每个非 null 分支都要求的字段才 required。
    fn fold_union(&self, x: &Map<String, Value>) -> Map<String, Value> {
        let mut out: Map<String, Value> = x
            .iter()
            .filter(|(k, _)| k.as_str() != "anyOf" && k.as_str() != "oneOf")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut object =
            x.get("type").and_then(Value::as_str) == Some("object") || is_set(x.get("properties"));
        if let Some(ts) = x.get("type").and_then(Value::as_array) {
            object = object || ts.iter().any(|t| t.as_str() == Some("object"));
        }
        let mut props: Map<String, Value> = Map::new();
        if let Some(own) = x.get("properties").and_then(Value::as_object) {
            props.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let mut common: Vec<String> = Vec::new();
        let mut branches = 0usize;
        for k in ["anyOf", "oneOf"] {
            let Some(alts) = x.get(k).and_then(Value::as_array) else {
                continue;
            };
            for a in alts {
                let Some(m) = self.resolve(Some(a)) else {
                    continue;
                };
                if m.get("type").and_then(Value::as_str) == Some("null") {
                    out.insert("nullable".into(), Value::Bool(true));
                    continue;
                }
                if !object {
                    continue;
                }
                if let Some(bp) = m.get("properties").and_then(Value::as_object) {
                    for pk in bp.keys() {
                        props.entry(pk.clone()).or_insert_with(|| bp[pk].clone());
                    }
                }
                let br = required_names(m.get("required"));
                if branches == 0 {
                    common = br;
                } else {
                    common.retain(|n| br.contains(n));
                }
                branches += 1;
            }
        }
        if object {
            if !props.is_empty() {
                out.insert("properties".into(), Value::Object(props));
            }
            let req = required_names(x.get("required"));
            if req.len() + common.len() > 0 {
                out.insert(
                    "required".into(),
                    json!(distinct_names(req.into_iter().chain(common).collect())),
                );
            }
        }
        out
    }
}

/// `$ref` 指向的是它所在的位置或它的容器（selfRef，:1151）：树里引用祖先 =
/// 自引用。
fn self_ref(r: &str, path: &str) -> bool {
    match r.strip_prefix('#') {
        Some(rest) => rest.is_empty() || path == rest || path.starts_with(&format!("{rest}/")),
        None => false,
    }
}

/// `$ref` 的目标走的路径（refPath，:1163）：指针型用它自己的，名字型用所在处的。
fn ref_path<'a>(r: &'a str, path: &'a str) -> &'a str {
    r.strip_prefix('#').unwrap_or(path)
}

/// schema 自己的形状说的类型（schemaTypeOf，:1174）：自引用停在这里。
fn schema_type_of(m: &Map<String, Value>) -> &str {
    if let Some(t) = m.get("type").and_then(Value::as_str) {
        return t;
    }
    if is_set(m.get("properties")) {
        return "object";
    }
    if is_set(m.get("items")) {
        return "array";
    }
    "object"
}

/// 元组里每个说了话的成员说的都一样，就是那个（oneSays，:1191）；不一样或都没
/// 说 = None。
fn one_says(members: &[Value]) -> Option<&Map<String, Value>> {
    let mut one: Option<&Map<String, Value>> = None;
    for m in members {
        let Some(s) = m.as_object().filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Some(previous) = one {
            if previous != s {
                return None;
            }
        }
        one = Some(s);
    }
    one
}

fn walk(
    v: &Value,
    depth: usize,
    path: &str,
    schema: &Schema<'_>,
    expanding: &mut HashSet<String>,
) -> Value {
    match v {
        // 布尔 schema：true = 任意值（`{}`），false = 没有值（nil，持有它的地方
        // 把它留掉）。Google 的 Schema 只有对象，布尔整个请求被拒（:1211）。
        Value::Bool(x) => {
            if *x {
                json!({})
            } else {
                Value::Null
            }
        }
        Value::Array(x) => Value::Array(
            x.iter()
                .map(|e| walk(e, depth, path, schema, expanding))
                .collect(),
        ),
        Value::Object(x) => walk_object(x, depth, path, schema, expanding),
        _ => v.clone(),
    }
}

#[allow(clippy::too_many_lines)]
fn walk_object(
    x: &Map<String, Value>,
    depth: usize,
    path: &str,
    schema: &Schema<'_>,
    expanding: &mut HashSet<String>,
) -> Value {
    if depth > MAX_DEPTH {
        return json!({ "type": "object" });
    }
    if let Some(r) = x.get("$ref").and_then(Value::as_str) {
        if let Some(d) = schema.local_ref(r) {
            if expanding.contains(r) || self_ref(r, path) {
                // 自引用停在目标的类型上
                return json!({ "type": d.as_object().map(schema_type_of).unwrap_or("object") });
            }
            expanding.insert(r.to_string());
            let out = walk(&d, depth + 1, ref_path(r, path), schema, expanding);
            expanding.remove(r);
            return out;
        }
        return json!({ "type": "object" });
    }
    // allOf：成员的字段并成一份，required 取每个成员的并（:1252）
    if let Some(all) = x.get("allOf").and_then(Value::as_array) {
        let mut merged = x.clone();
        merged.remove("allOf");
        let mut required: Vec<String> = required_names(x.get("required"));
        for e in all {
            // 成员的 required 在走它之前读：只写了 required 的成员走完就没有
            // properties 留住这些名字了
            required.extend(required_names(
                schema
                    .resolve(Some(e))
                    .as_ref()
                    .and_then(|m| m.get("required")),
            ));
            if let Some(m) =
                walk(e, depth + 1, &format!("{path}/allOf"), schema, expanding).as_object()
            {
                for (k, v) in m {
                    if k == "required" {
                        continue;
                    }
                    if k == "properties" {
                        let props = merged.entry("properties").or_insert_with(|| json!({}));
                        if let (Some(target), Some(source)) = (props.as_object_mut(), v.as_object())
                        {
                            for (pk, pv) in source {
                                target.insert(pk.clone(), pv.clone());
                            }
                        }
                    } else if !merged.contains_key(k) {
                        merged.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        if !required.is_empty() {
            merged.insert("required".into(), json!(distinct_names(required)));
        }
        return walk(&Value::Object(merged), depth + 1, path, schema, expanding);
    }
    // 自己有形状、旁边还有 anyOf / oneOf：自己的留下，联合并进去（:1290）
    if (is_set(x.get("anyOf")) || is_set(x.get("oneOf"))) && own_shape(x) {
        return walk(
            &Value::Object(schema.fold_union(x)),
            depth + 1,
            path,
            schema,
            expanding,
        );
    }
    // 光有 anyOf / oneOf：第一个不是 null 的成员，可空（:1302）
    for k in ["anyOf", "oneOf"] {
        if let Some(alts) = x.get(k).and_then(Value::as_array) {
            let mut pick: Option<&Value> = None;
            let mut nullable = false;
            for a in alts {
                let m = a.as_object();
                if m.and_then(|m| m.get("type")).and_then(Value::as_str) == Some("null") {
                    nullable = true;
                } else if pick.is_none() && m.is_some() {
                    pick = Some(a);
                }
            }
            let mut out = match pick {
                Some(p) => walk(p, depth + 1, &format!("{path}/{k}"), schema, expanding),
                None => json!({}),
            };
            if !out.is_object() {
                out = json!({});
            }
            let object = out.as_object_mut().expect("object just ensured");
            if let Some(d) = x.get("description").and_then(Value::as_str) {
                if !is_set(object.get("description")) {
                    object.insert("description".into(), json!(d));
                }
            }
            if nullable {
                object.insert("nullable".into(), Value::Bool(true));
            }
            if !is_set(object.get("type")) {
                object.insert("type".into(), json!("string"));
            }
            return out;
        }
    }

    let mut out: Map<String, Value> = Map::new();
    for (k, v) in x {
        match k.as_str() {
            "description" | "nullable" | "required" | "pattern" | "minimum" | "maximum"
            | "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
            | "maxProperties" | "propertyOrdering" => {
                out.insert(k.clone(), v.clone());
            }
            // 只按包含端点收的界（draft 6 起是数字）（:1338）
            "exclusiveMinimum" | "exclusiveMaximum" => {
                if v.is_number() {
                    let bound = format!("m{}", &k["exclusiveM".len()..]);
                    if !is_set(x.get(&bound)) {
                        // 原样的数字：整数还是整数（Go 的 float64 编码回去也是 9）
                        out.insert(bound, v.clone());
                    }
                }
            }
            "type" => {
                if let Some(ts) = v.as_array() {
                    for t in ts {
                        if t.as_str() == Some("null") {
                            out.insert("nullable".into(), Value::Bool(true));
                        } else if !is_set(out.get("type")) {
                            out.insert("type".into(), t.clone());
                        }
                    }
                    // 一个节点一个类型：列表里取 items / properties 对应的那个
                    for t in ts {
                        let t = t.as_str().unwrap_or("");
                        if (t == "array" && is_set(x.get("items")))
                            || (t == "object" && is_set(x.get("properties")))
                        {
                            out.insert("type".into(), json!(t));
                            break;
                        }
                    }
                } else {
                    out.insert("type".into(), v.clone());
                }
            }
            "properties" => {
                let mut props = Map::new();
                if let Some(m) = v.as_object() {
                    for (pk, pv) in m {
                        // 没有名字的属性整个请求被拒（:1369）
                        if pk.is_empty() {
                            continue;
                        }
                        // schema 为 false 的属性是调用不可以有的：留掉，required
                        // 里它的名字也留掉
                        let at = pk.replace('~', "~0").replace('/', "~1");
                        let w = walk(
                            pv,
                            depth + 1,
                            &format!("{path}/properties/{at}"),
                            schema,
                            expanding,
                        );
                        if !w.is_null() {
                            props.insert(pk.clone(), w);
                        }
                    }
                }
                out.insert("properties".into(), Value::Object(props));
            }
            "items" => {
                let w = walk(v, depth + 1, &format!("{path}/items"), schema, expanding);
                if !w.is_null() {
                    out.insert("items".into(), w);
                } else if !is_set(x.get("maxItems")) {
                    // items false：什么都不装的数组
                    out.insert("items".into(), json!({}));
                    out.insert("maxItems".into(), json!(0));
                }
            }
            "enum" => {
                // Google 只收字符串枚举，而且不许有空串（:1393）
                if let Some(es) = v.as_array() {
                    let mut strs: Vec<&str> = Vec::new();
                    let mut all = true;
                    for e in es {
                        match e.as_str() {
                            Some(s) => {
                                if !s.trim().is_empty() {
                                    strs.push(s);
                                }
                            }
                            None => {
                                all = false;
                                break;
                            }
                        }
                    }
                    if all && !strs.is_empty() {
                        out.insert("enum".into(), json!(strs));
                    }
                }
            }
            // 子集之外的关键字（format、default、title、additionalProperties…）：
            // Antigravity 遇到不认识的就拒整个请求（:1416）
            _ => {}
        }
    }
    if let Some(c) = x.get("const").and_then(Value::as_str) {
        if !c.trim().is_empty() {
            out.insert("enum".into(), json!([c]));
        }
    }
    if out.get("type").and_then(Value::as_str) == Some("array") {
        if let Some(es) = out.get("enum").cloned() {
            // 数组上的 enum 是它成员的值；Google 的 enum 在字符串上
            out.remove("enum");
            if let Some(it) = out.get_mut("items").and_then(Value::as_object_mut) {
                let plain = !is_set(it.get("enum"))
                    && (!is_set(it.get("type"))
                        || it.get("type").and_then(Value::as_str) == Some("string"));
                if plain {
                    it.insert("type".into(), json!("string"));
                    it.insert("enum".into(), es);
                }
            }
        }
    }
    if !is_set(out.get("type")) {
        if is_set(out.get("properties")) {
            out.insert("type".into(), json!("object"));
        } else if is_set(out.get("items")) {
            out.insert("type".into(), json!("array"));
        }
    }
    if out.get("type").and_then(Value::as_str) == Some("array") {
        // 元组（draft 07 的 items 数组）当成一个 schema：成员一样就说一次
        if let Some(t) = out.get("items").and_then(Value::as_array) {
            let said = one_says(t)
                .map(|m| Value::Object(m.clone()))
                .unwrap_or(Value::Null);
            out.insert("items".into(), said);
        }
        if !is_set(out.get("items")) {
            if let Some(one) = x
                .get("prefixItems")
                .and_then(Value::as_array)
                .filter(|p| !p.is_empty())
                .and_then(|p| one_says(p))
            {
                out.insert(
                    "items".into(),
                    walk(
                        &Value::Object(one.clone()),
                        depth + 1,
                        &format!("{path}/items"),
                        schema,
                        expanding,
                    ),
                );
            }
            if !is_set(out.get("items")) {
                // 数组不说成员是什么，Google 就拒整个请求（#2826）；任意成员
                // `{}` 回 200
                out.insert("items".into(), json!({}));
            }
        }
    }
    let type_name = out.get("type").and_then(Value::as_str).map(String::from);
    if let Some(t) = type_name.as_deref() {
        // JSON schema 的数组关键字只在数组上读、对象的只在对象上读；放错类型
        // Google 拒整个请求（:1468）
        if t != "array" {
            out.remove("items");
            out.remove("minItems");
            out.remove("maxItems");
        }
        if t != "object" {
            out.remove("properties");
            out.remove("required");
            out.remove("minProperties");
            out.remove("maxProperties");
            out.remove("propertyOrdering");
        }
    }
    if let Some(req) = out.get("required").and_then(Value::as_array).cloned() {
        // required 只留确实有的属性
        let props = out.get("properties").and_then(Value::as_object);
        let keep: Vec<Value> = req
            .into_iter()
            .filter(|n| {
                n.as_str()
                    .is_some_and(|s| props.and_then(|p| p.get(s)).is_some_and(|v| !v.is_null()))
            })
            .collect();
        if keep.is_empty() {
            out.remove("required");
        } else {
            out.insert("required".into(), Value::Array(keep));
        }
    }
    Value::Object(out)
}

/// schema 自己说了它是什么（ownShape，:1516）：类型不只 null，或有
/// properties / items。
fn own_shape(x: &Map<String, Value>) -> bool {
    if is_set(x.get("properties")) || is_set(x.get("items")) {
        return true;
    }
    match x.get("type") {
        Some(Value::String(t)) => t != "null",
        Some(Value::Array(ts)) => ts.iter().any(|e| e.as_str() != Some("null")),
        _ => false,
    }
}

/// required 列表里的字符串（requiredNames，:1603）。
fn required_names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(|e| e.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 去重保序（distinctNames，:1615）。
fn distinct_names(vs: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    vs.into_iter().filter(|v| seen.insert(v.clone())).collect()
}

// =====================================================================
// 系统提示词改写（cloak，:52–:330）
// =====================================================================

/// Claude Code 的计费行，按 agTurnedAway 的写法。
const BILLING_LINE: &str = "x-anthropic-billing-header:";
/// omp（oh-my-pi）系统提示词的第一行。
const RFC_LINE: &str = "rfc 2119 keywords: must, required, should, recommended, may, optional.";

/// Antigravity 一律回 429 的那些行（小写；agTurnedAway，:112），每一条都在真账号
/// 上试过：
/// - Claude Code 的计费行、Claude Agent SDK 的身份行（#666）；
/// - Codex 给 GPT-5 模型自己的提示词的身份行；
/// - Hermes Agent 的人设（现在的和更早的）；
/// - 11-14 起：「You are Hermes Agent」「You are OpenCode」「You are a coding
///   agent running in the Codex CLI」「a coding assistant running inside pi」。
const TURNED_AWAY: [&str; 10] = [
    BILLING_LINE,
    "built on anthropic's claude agent sdk",
    "you are codex, an agent based on gpt-5",
    "you are codex, a coding agent based on gpt-5",
    "an intelligent ai assistant created by nous research",
    RFC_LINE,
    "you are hermes agent",
    "you are opencode",
    "you are a coding agent running in the codex cli",
    "a coding assistant running inside pi",
];

/// 空白按 Google 的检查读它的范围（agSpace，:180）：CRLF、制表、不换行空格都算。
const AG_SPACE: &str = r"[\s\v\p{Zs}\x{2028}\x{2029}\x{feff}]+";
/// 11-14 起 Google 的检查读得过去的隐形字符（agInvisible，:184）。
const AG_INVISIBLE: &str = r"[\x{00ad}\x{180e}\x{200b}-\x{200f}\x{2060}-\x{2064}\x{feff}]";

struct Cloak {
    /// TURNED_AWAY 逐条的正则（agTurnedAwayRes）。
    lines: Vec<Regex>,
    sdk_said_as: Regex,
    based_on: Regex,
    created_by: Regex,
    rfc_said_as: Regex,
    you_are: Regex,
    inside: Regex,
    invisible: Regex,
}

fn static_re(src: &str) -> Regex {
    Regex::new(src).unwrap_or_else(|e| panic!("static antigravity regex must compile: {e}\n{src}"))
}

/// 一行按任何大小写、词间任何空白、词内任何隐形字符都认出来（agLineRe，:193）；
/// 计费行匹配到行尾，因为它整行不发。
fn line_re(line: &str) -> Regex {
    let words: Vec<String> = line
        .split(' ')
        .map(|w| {
            w.chars()
                .map(|c| regex::escape(&c.to_string()))
                .collect::<Vec<_>>()
                .join(&format!("{AG_INVISIBLE}*"))
        })
        .collect();
    let mut src = format!(
        "(?i){}",
        words.join(&format!("{AG_INVISIBLE}*{AG_SPACE}{AG_INVISIBLE}*"))
    );
    if line == BILLING_LINE {
        src.push_str(r"[^\n]*\n?");
    }
    static_re(&src)
}

fn cloak() -> &'static Cloak {
    static CLOAK: OnceLock<Cloak> = OnceLock::new();
    CLOAK.get_or_init(|| Cloak {
        lines: TURNED_AWAY.iter().map(|line| line_re(line)).collect(),
        sdk_said_as: static_re(&format!(r"(?i)^(built{AG_SPACE}on{AG_SPACE})anthropic's({AG_SPACE})(claude{AG_SPACE}agent{AG_SPACE}sdk)$")),
        based_on: static_re(&format!(r"(?i)based({AG_SPACE})on")),
        created_by: static_re(&format!(r"(?i)created({AG_SPACE})by")),
        rfc_said_as: static_re(r"(?i)(key)(words)"),
        you_are: static_re(&format!(r"(?i)^(you){AG_SPACE}are")),
        inside: static_re(r"(?i)inside"),
        invisible: static_re(AG_INVISIBLE),
    })
}

/// 每条被拒的行换成同义的说法（agSaidAs，:148）：计费行整行不留；SDK 的「built
/// on Anthropic's Claude Agent SDK」→「built on the Claude Agent SDK by
/// Anthropic」；「based on」→「built on」；「created by」→「built by」；
/// 「keywords」→「key words」；「You are …」→「You're …」；「inside pi」→
/// 「within pi」。
fn said_as(index: usize, matched: &str) -> String {
    let c = cloak();
    match index {
        0 => String::new(),
        1 => c
            .sdk_said_as
            .replace_all(matched, "${1}the${2}${3} by Anthropic")
            .into_owned(),
        2 | 3 => c.based_on.replace_all(matched, "built${1}on").into_owned(),
        4 => c
            .created_by
            .replace_all(matched, "built${1}by")
            .into_owned(),
        5 => c.rfc_said_as.replace_all(matched, "$1 $2").into_owned(),
        6 | 7 | 8 => c.you_are.replace_all(matched, "${1}'re").into_owned(),
        _ => c.inside.replace_all(matched, "within").into_owned(),
    }
}

/// 系统提示词里有没有会被 Antigravity 拒的行（antigravityTurnsAway，:61）。
pub(crate) fn turns_away(system: &str) -> bool {
    cloak().lines.iter().any(|re| re.is_match(system))
}

/// 按 agTurnedAway 逐条换说法（agCloakLines，:300）；`only` 给了就只换选中
/// 的那几条。彼此重叠的匹配先来的算。
fn cloak_lines(system: &str, only: Option<&[bool]>) -> String {
    let c = cloak();
    let mut at: Vec<(usize, usize, usize)> = Vec::new();
    for (i, re) in c.lines.iter().enumerate() {
        if let Some(only) = only {
            if !only[i] {
                continue;
            }
        }
        for m in re.find_iter(system) {
            at.push((m.start(), m.end(), i));
        }
    }
    at.sort_unstable();
    let mut out = String::with_capacity(system.len());
    let mut prev = 0usize;
    for (from, to, index) in at {
        if from < prev {
            continue;
        }
        out.push_str(&system[prev..from]);
        let matched = c.invisible.replace_all(&system[from..to], "");
        out.push_str(&said_as(index, matched.as_ref()));
        prev = to;
    }
    out.push_str(&system[prev..]);
    out
}

/// 用户的话里也会被拒的行（agUserTurnedAway，:290）：11-14 起只有 Codex 的
/// GPT-5 那两条；别的行在用户的话里原样能过。
fn user_turned_away() -> Vec<bool> {
    TURNED_AWAY
        .iter()
        .map(|line| line.starts_with("you are codex, "))
        .collect()
}

/// 发给 Antigravity 账号的系统提示词（antigravityCloak，:276）：被拒的行换成
/// 同义说法，别的一个字不动。
pub(crate) fn cloak_system(system: &str) -> String {
    if !turns_away(system) {
        return system.to_string();
    }
    cloak_lines(system, None)
}

/// 用户的话里那几行也换掉（antigravityCloakMessages，:334）：模型自己的回合、
/// 工具结果和别的行不动。返回有没有改。
pub(crate) fn cloak_user_turns(messages: &mut [Msg]) -> bool {
    let only = user_turned_away();
    let mut changed = false;
    for m in messages.iter_mut() {
        if m.assistant {
            continue;
        }
        for p in m.parts.iter_mut() {
            let Part::Text(text) = p else { continue };
            let t = cloak_lines(text, Some(&only));
            if t != *text {
                *text = t;
                changed = true;
            }
        }
    }
    changed
}

/// Antigravity 收得下、但答得不好的段落，换成同义的说法（agReworded /
/// antigravityReword，:257）：DeepSeek Harness 的桌面版每个系统提示词里都放
/// 第一段；逐字带着它，gemini-4-argon 多数请求三秒就收、不思考。
const REWORDED: [(&str, &str); 1] = [(
    r#"Tokens prefixed with @ are paths the user explicitly referenced. Relative paths resolve from the workspace root; absolute paths identify files or directories on the host. A trailing slash marks a directory: list it when its contents matter. Anything else is a file: use the read tool when its contents are needed, and do not claim to have inspected it before reading. @"..." quotes a path containing spaces."#,
    r#"When the user writes @ followed by a path, they are pointing at that file or folder. Relative paths start at the workspace root; absolute paths are host paths. A path ending in a slash is a folder, so list it if its contents matter; otherwise it is a file, so open it with the read tool before relying on it. Paths with spaces appear as @"..."."#,
)];

pub(crate) fn reword(system: &str) -> String {
    REWORDED
        .iter()
        .fold(system.to_string(), |s, (from, to)| s.replace(from, to))
}

fn ag_says(system: &str, line: &str) -> bool {
    let Some(index) = TURNED_AWAY.iter().position(|l| *l == line) else {
        return false;
    };
    cloak().lines[index].is_match(system)
}

fn turns_codex_away(system: &str) -> bool {
    ag_says(system, "you are codex, an agent based on gpt-5")
        || ag_says(system, "you are codex, a coding agent based on gpt-5")
}

/// 被拒时附在 Antigravity 原话后面的提示（antigravityHint，:444）：说的是哪一
/// 行、为什么不是额度、该怎么办。
pub(crate) fn turned_away_hint(system: &str) -> &'static str {
    if turns_codex_away(system) {
        return "Antigravity answers this 429 to Codex's own system prompt for GPT-5 models whatever quota is left, so it is not a quota and waiting won't help; a Codex chat begun on a GPT model keeps that prompt after a model switch or a fork, so start a new chat on this model";
    }
    if ag_says(system, "you are a coding agent running in the codex cli") {
        return "Antigravity answers this 429 to Codex's prompt for a model it has none of its own for (\"You are a coding agent running in the Codex CLI\") whatever quota is left, so it is not a quota and waiting won't help; Marionette sends that line as \"You're …\", which goes through, so use another provider for these chats, or put one after Antigravity in a routing group";
    }
    if ag_says(system, "you are hermes agent")
        || ag_says(
            system,
            "an intelligent ai assistant created by nous research",
        )
    {
        return "Antigravity answers this 429 to Hermes Agent's default persona (\"You are Hermes Agent, …\" in its SOUL.md), the current one and the earlier one alike, whatever quota is left, so it is not a quota and waiting won't help; Marionette sends it as \"You're Hermes Agent, …\", which goes through, so start SOUL.md another way, or use another provider for these chats";
    }
    if ag_says(system, "you are opencode") {
        return "Antigravity answers this 429 to OpenCode's system prompt (\"You are OpenCode, …\") whatever quota is left, so it is not a quota and waiting won't help; Marionette sends that line as \"You're OpenCode, …\", which goes through, so use another provider for these chats, or put one after Antigravity in a routing group";
    }
    if ag_says(system, "a coding assistant running inside pi") {
        return "Antigravity answers this 429 to pi's default system prompt (\"You are a coding assistant running inside pi\") whatever quota is left, so it is not a quota and waiting won't help; Marionette sends that line as \"running within pi\", which goes through, so give pi a system prompt of your own (--system-prompt) or use another provider for these chats";
    }
    if ag_says(system, RFC_LINE) {
        return "Antigravity answers this 429 to the line \"RFC 2119 keywords: MUST, REQUIRED, SHOULD, RECOMMENDED, MAY, OPTIONAL.\" that starts oh-my-pi (omp)'s system prompt, and to any prompt that has it, whatever quota is left, so it is not a quota and waiting won't help; Marionette sends that line as \"RFC 2119 key words: …\", which goes through, so this one came from a copy of it that was worded another way";
    }
    "Antigravity answers this 429 to the system prompt of Claude Code and the Claude Agent SDK (Claude Desktop's chats) whatever quota is left, so it is not a quota and waiting won't help; use another provider for these chats, or put one after Antigravity in a routing group"
}

/// 「被拒」的两种形状里的原话（antigravityRefuses，:369）：429 的应答体，或
/// 应答流里自己的 error 事件。
pub(crate) fn refuses(said: &str) -> bool {
    said.to_lowercase().contains("resource has been exhausted")
}

// =====================================================================
// 请求侧 IR：OpenAI Chat 请求体（Go readChat，chat.go:119）
// =====================================================================

/// 一张图或一个文件：内联（base64）或一个地址。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Blob {
    pub media_type: String,
    pub data: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Part {
    Text(String),
    File(Blob),
    ToolCall {
        id: String,
        name: String,
        args: Value,
    },
    ToolResult {
        call_id: String,
        text: String,
        images: Vec<Blob>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Msg {
    pub assistant: bool,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tool {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Format {
    JsonObject,
    JsonSchema(Value),
}

/// 一个 Chat 请求里 Code Assist 关心的东西（Go `Request` 里 buildCodeAssistSent
/// 读的那些字段）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ir {
    /// system / developer 消息的文本，`\n\n` 连接。
    pub system: String,
    pub messages: Vec<Msg>,
    pub tools: Vec<Tool>,
    /// `""`（没说）/ `auto` / `none` / `required` / `name:<工具>`。
    pub tool_choice: String,
    pub web_search: bool,
    pub max_tokens: u64,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    /// 归一后的强度（effortOf，ir.go:1104）；没要 = `""`。
    pub effort: String,
    /// 客户端明确把思考关了（reasoning_effort = none）。
    pub think_off: bool,
    pub format: Option<Format>,
}

/// Go effortOf（ir.go:1104）：各家 API 的强度词归一。
pub(crate) fn effort_of(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "minimal" | "none" => "low".into(),
        "low" | "medium" | "high" | "xhigh" | "max" => s.trim().to_ascii_lowercase(),
        "ultra" => "max".into(),
        _ => String::new(),
    }
}

/// stringOrText（ir.go:998）：字符串原样；数组取其中文本块的文字。
fn string_or_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.get("type").and_then(Value::as_str),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// imagePart（chat.go:338）：`data:` 地址拆成内联的图，别的当地址留着。
fn image_part(url: &str) -> Blob {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((meta, data)) = rest.split_once(',') {
            return Blob {
                media_type: meta.trim_end_matches(";base64").to_string(),
                data: data.to_string(),
                url: String::new(),
            };
        }
    }
    Blob {
        media_type: String::new(),
        data: String::new(),
        url: url.to_string(),
    }
}

/// 用户消息的 content（chatParts，chat.go:237）：字符串或部件数组里的文字和图。
fn chat_parts(v: Option<&Value>) -> Vec<Part> {
    match v {
        Some(Value::String(s)) => {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![Part::Text(s.clone())]
            }
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|it| match it.get("type").and_then(Value::as_str)? {
                "text" => Some(Part::Text(
                    it.get("text").and_then(Value::as_str)?.to_string(),
                )),
                "image_url" => Some(Part::File(image_part(
                    it.pointer("/image_url/url").and_then(Value::as_str)?,
                ))),
                "file" => {
                    let data = it
                        .pointer("/file/file_data")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let id = it
                        .pointer("/file/file_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !data.is_empty() {
                        let p = image_part(data);
                        Some(Part::File(Blob {
                            media_type: if p.data.is_empty() {
                                String::new()
                            } else {
                                p.media_type
                            },
                            data: p.data,
                            url: String::new(),
                        }))
                    } else if !id.is_empty() {
                        Some(Part::File(Blob {
                            media_type: String::new(),
                            data: String::new(),
                            url: id.to_string(),
                        }))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// 工具结果（toolOutput，ir.go:1041）：文字，加它带的图。
fn tool_output(v: Option<&Value>) -> (String, Vec<Blob>) {
    let text = string_or_text(v);
    let mut images = Vec::new();
    if let Some(Value::Array(blocks)) = v {
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                // 工具给的一张图（source 里是内联数据或地址）
                Some("image") => {
                    let source = b.get("source");
                    if let Some(data) = source
                        .and_then(|s| s.get("data"))
                        .and_then(Value::as_str)
                        .filter(|d| !d.is_empty())
                    {
                        images.push(Blob {
                            media_type: source
                                .and_then(|s| s.get("media_type"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            data: data.to_string(),
                            url: String::new(),
                        });
                    } else if let Some(url) = source
                        .and_then(|s| s.get("url"))
                        .and_then(Value::as_str)
                        .filter(|u| !u.is_empty())
                    {
                        images.push(Blob {
                            media_type: String::new(),
                            data: String::new(),
                            url: url.to_string(),
                        });
                    }
                }
                Some("input_image") | Some("image_url") => {
                    let url = b
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !url.is_empty() {
                        images.push(image_part(url));
                    }
                }
                _ => {}
            }
        }
    }
    (text, images)
}

/// 一个 Chat 请求体读成 IR（Go readChat）。
pub(crate) fn read_chat(body: &Value) -> Ir {
    let mut sys: Vec<String> = Vec::new();
    let mut messages: Vec<Msg> = Vec::new();
    let empty = Vec::new();
    for m in body
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        match m.get("role").and_then(Value::as_str).unwrap_or("") {
            "system" | "developer" => sys.push(string_or_text(m.get("content"))),
            "user" => {
                let parts = chat_parts(m.get("content"));
                if !parts.is_empty() {
                    messages.push(Msg {
                        assistant: false,
                        parts,
                    });
                }
            }
            "assistant" => {
                let mut parts = chat_parts(m.get("content"));
                for tc in m
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let function = tc.get("function");
                    parts.push(Part::ToolCall {
                        id: tc
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        name: function
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        args: parse_args(
                            function
                                .and_then(|f| f.get("arguments"))
                                .unwrap_or(&Value::Null),
                        ),
                    });
                }
                messages.push(Msg {
                    assistant: true,
                    parts,
                });
            }
            "tool" => {
                let (text, images) = tool_output(m.get("content"));
                messages.push(Msg {
                    assistant: false,
                    parts: vec![Part::ToolResult {
                        call_id: m
                            .get("tool_call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        text,
                        images,
                    }],
                });
            }
            _ => {}
        }
    }
    let mut tools = Vec::new();
    let mut web_search = body.get("web_search_options").is_some_and(|o| !o.is_null());
    for t in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = t.get("type").and_then(Value::as_str).unwrap_or("");
        if kind.starts_with("web_search") {
            web_search = true;
        }
        let function = match t.get("function") {
            Some(f) => f,
            None => continue,
        };
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() || (!kind.is_empty() && kind != "function") {
            continue;
        }
        tools.push(Tool {
            name: name.to_string(),
            description: function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            schema: function.get("parameters").cloned().unwrap_or(Value::Null),
        });
    }
    let tool_choice = match body.get("tool_choice") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => match o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
        {
            Some(name) if !name.is_empty() => format!("name:{name}"),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let reasoning_effort = body
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    // chatEffort（chat.go:88）：reasoning_effort；没说时看中转/客户端放强度的
    // 别的地方。
    let mut effort = effort_of(&reasoning_effort);
    if effort.is_empty() && reasoning_effort.is_empty() {
        for pointer in ["/reasoning/effort", "/output_config/effort"] {
            if let Some(e) = body
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(effort_of)
                .filter(|e| !e.is_empty())
            {
                effort = e;
                break;
            }
        }
        if effort.is_empty() {
            if let Some(t) = body.get("thinking").filter(|t| t.is_object()) {
                if t.get("type").and_then(Value::as_str) == Some("enabled") {
                    let budget = t.get("budget_tokens").and_then(Value::as_u64).unwrap_or(0);
                    effort = match budget {
                        0 => String::new(),
                        n if n <= 4096 => "low".into(),
                        n if n <= 12000 => "medium".into(),
                        n if n <= 24000 => "high".into(),
                        _ => "xhigh".into(),
                    };
                }
            }
        }
    }
    let stop = match body.get("stop") {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    };
    let format = match body.get("response_format") {
        Some(f) => match f.get("type").and_then(Value::as_str) {
            Some("json_object") => Some(Format::JsonObject),
            Some("json_schema") => {
                let js = f.get("json_schema");
                let schema = js
                    .and_then(|j| j.get("schema"))
                    .cloned()
                    .unwrap_or(Value::Null);
                if schema.is_null() {
                    // 没有形状的 schema 就是任意 JSON 对象
                    Some(Format::JsonObject)
                } else {
                    Some(Format::JsonSchema(schema))
                }
            }
            _ => None,
        },
        None => None,
    };
    Ir {
        system: sys.join("\n\n"),
        messages,
        tools,
        tool_choice,
        web_search,
        max_tokens: body
            .get("max_completion_tokens")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .or_else(|| body.get("max_tokens").and_then(Value::as_u64))
            .unwrap_or(0),
        temperature: body.get("temperature").and_then(Value::as_f64),
        top_p: body.get("top_p").and_then(Value::as_f64),
        stop,
        effort: effort.clone(),
        think_off: reasoning_effort.eq_ignore_ascii_case("none"),
        format,
    }
}

/// parseArgs（ir.go:926）：参数字符串是 JSON 对象就原样，不是就包一层
/// `{"input": …}`，什么都不丢。
fn parse_args(raw: &Value) -> Value {
    let Value::String(s) = raw else {
        return if raw.is_object() {
            raw.clone()
        } else {
            json!({})
        };
    };
    let s = s.trim();
    if s.is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(s) {
        Ok(v) if v.is_object() => v,
        _ => json!({ "input": s }),
    }
}

// =====================================================================
// buildCodeAssistSent（:547）：IR → Gemini `request`（agent = antigravity）
// =====================================================================

/// Antigravity 的 id 后面带的强度词和它对应的档位，长的在前
/// （antigravityEffortWords，provider/antigravity_models.go:27）。
const EFFORT_WORDS: [(&str, &str); 7] = [
    ("extra-high", "xhigh"),
    ("extra-low", "minimal"),
    ("xhigh", "xhigh"),
    ("minimal", "minimal"),
    ("low", "low"),
    ("medium", "medium"),
    ("high", "high"),
];

/// 一个 id 的家族，和它的强度词说的档位（splitAntigravityID，:34）；没有
/// 强度词 = `""`。
pub(crate) fn split_antigravity_id(id: &str) -> (&str, &str) {
    for (word, level) in EFFORT_WORDS {
        if let Some(base) = id.strip_suffix(&format!("-{word}")) {
            if !base.is_empty() {
                return (base, level);
            }
        }
    }
    (id, "")
}

/// Gemini 模型 id 说的世代（geminiGeneration，:1073）：gemini-4-argon 是 4，
/// 不是 Gemini = 0。
fn gemini_generation(model: &str) -> u64 {
    let re = static_re(r"(?:^|/)gemini-(\d+)");
    re.captures(model)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0)
}

/// 画图的模型（catalog.DrawsID 的子集：id 里带 image / imagen）。
fn draws_id(model: &str) -> bool {
    model.contains("image")
}

fn off_effort(effort: &str) -> bool {
    effort == "none" || effort == "minimal"
}

/// Gemini 2.5 收的思考预算（vertexBudgets，vertex.go:84）：关掉的、最少的、最多的。
fn vertex_budgets(model: &str) -> Option<(u64, u64, u64)> {
    if model.starts_with("gemini-2.5-pro") {
        Some((128, 128, 32768))
    } else if model.starts_with("gemini-2.5-flash-lite") {
        Some((0, 512, 24576))
    } else if model.starts_with("gemini-2.5-flash") {
        Some((0, 1, 24576))
    } else {
        None
    }
}

/// Anthropic 口径的思考预算（budgetOf，ir.go:1157）。
fn budget_of(effort: &str) -> u64 {
    match effort {
        "low" => 4096,
        "medium" => 10000,
        "high" => 24000,
        "xhigh" | "max" => 32000,
        _ => 10000,
    }
}

/// 模型想多久（thinkingConfig，:1006）：Gemini 3 起说档位，更早的说预算；不
/// 思考的模型不说；id 自己就带档位的（gemini-3.7-flash-low）只说那一档。
#[allow(clippy::too_many_arguments)]
fn thinking_config(
    effort: &str,
    think_off: bool,
    max_tokens: u64,
    model: &str,
    claude: bool,
    at: &str,
) -> Option<Value> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-oss") || (claude && !m.contains("thinking")) {
        return None;
    }
    // Claude 被要求思考时仍得说想多少；别的没被要求就不说
    if effort.is_empty() && !claude {
        return None;
    }
    let mut tc = json!({ "includeThoughts": true });
    let generation = gemini_generation(&m);
    if generation >= 3 || m.starts_with("gemini-pro-agent") {
        let effort = if at.is_empty() { effort } else { at };
        if !effort.is_empty() {
            let mut level = "high";
            if effort == "low" || (effort == "minimal" && !at.is_empty() && !m.contains("flash")) {
                level = "low";
            } else if effort == "minimal" && !at.is_empty() {
                level = "minimal";
            } else if off_effort(effort) && !draws_id(&m) {
                level = "low";
            } else if effort == "medium" && (m.contains("flash") || generation > 3) {
                level = "medium";
            }
            // 带 xhigh 变体的（Gemini 4 Hydrogen）就按 xhigh 要；没有这种
            // 变体的留在 high
            if at == "xhigh" {
                level = "xhigh";
            }
            // xhigh 是 Gemini 4 自己的档（v0.1.4824）：Gemini 3 一律不收
            if effort == "xhigh" && generation >= 4 {
                level = "xhigh";
            }
            tc["thinkingLevel"] = json!(level);
        }
        return Some(tc);
    }
    if let Some((off, _, _)) = vertex_budgets(m.trim_start_matches("models/")) {
        if think_off && !claude {
            // 把思考关了（Chat 客户端的 effort none）：按 2.5 的关掉预算发
            return Some(json!({ "thinkingBudget": off }));
        }
    }
    let mut budget = budget_of(effort);
    if claude && max_tokens > 0 && budget >= max_tokens {
        budget = max_tokens - 1;
        if budget < 1024 {
            return None;
        }
    }
    tc["thinkingBudget"] = json!(budget);
    Some(tc)
}

/// 压缩请求（compacting.go:85）：agent 在给自己的对话做压缩。系统提示词
/// 开头是那几句，或最后一个用户回合里带着那句要求。
const COMPACT_SYSTEMS: [&str; 9] = [
    "You are a helpful AI assistant tasked with summarizing conversations.",
    "You are a context summarization agent.",
    "You are a context summarization assistant.",
    "You are a specialized system component responsible for distilling chat history into a structured XML <state_snapshot>",
    "You are the component that summarizes a conversation when its context window is about to overflow.",
    "You are a context compaction engine. Your summary will REPLACE the original messages in the conversation context window.",
    "You are a helpful assistant that compiles conversation context.",
    "You are compacting a long conversation so it can continue WITHOUT losing context.",
    "You are UPDATING an existing conversation summary with new activity so the conversation can continue WITHOUT losing context.",
];
const COMPACT_ASKS: [&str; 7] = [
    "Your task is to create a detailed summary of the conversation so far",
    "You are performing a CONTEXT CHECKPOINT COMPACTION.",
    "You are now given a task to compact this conversation context",
    "END OF CONVERSATION TO COMPACT.",
    "Merge the previous summary with the new conversation.",
    "Here is the full conversation to compact.",
    "You are now acting as a compaction engine for this AI coding assistant.",
];
const COMPACT_LEAD: usize = 2048;

fn leads(text: &str, p: &str) -> bool {
    text.find(p).is_some_and(|i| i <= COMPACT_LEAD)
}

fn compacting(system: &str, messages: &[Msg]) -> bool {
    if COMPACT_SYSTEMS.iter().any(|p| leads(system, p)) {
        return true;
    }
    for m in messages.iter().rev() {
        if m.assistant {
            continue;
        }
        for p in &m.parts {
            let Part::Text(text) = p else { continue };
            if COMPACT_ASKS.iter().any(|ask| leads(text, ask)) {
                return true;
            }
        }
        break;
    }
    false
}

/// 对话以模型的话收尾时补的用户回合（modelLastAsk，prefill.go:59）。
fn model_last_ask(system: &str, messages: &[Msg]) -> &'static str {
    if compacting(system, messages) {
        "Write the summary of the conversation above that the system prompt asks for."
    } else {
        "Continue."
    }
}

/// 一个调用没人答时给它的合成结果（interruptedToolNote，:911）。
const INTERRUPTED_TOOL_NOTE: &str =
    "[The result of this tool call is unavailable: the turn was interrupted.]";

/// 同一个调用被答了两次时只留第一个（dropSecondAnswers，:867）：Google 在
/// functionResponse 数量和 functionCall 数量对不上时拒掉调用回合。
fn drop_second_answers(messages: &[Msg]) -> Vec<Msg> {
    let mut out: Option<Vec<Msg>> = None;
    let mut answered: HashSet<String> = HashSet::new();
    for (i, m) in messages.iter().enumerate() {
        let mut kept: Option<Vec<Part>> = None;
        for (j, p) in m.parts.iter().enumerate() {
            match p {
                Part::ToolCall { id, .. } if !id.is_empty() => {
                    answered.remove(id);
                }
                Part::ToolResult { call_id, .. } if !call_id.is_empty() => {
                    if answered.contains(call_id) {
                        kept.get_or_insert_with(|| m.parts[..j].to_vec());
                        out.get_or_insert_with(|| messages[..i].to_vec());
                        continue;
                    }
                    answered.insert(call_id.clone());
                }
                _ => {}
            }
            if let Some(parts) = kept.as_mut() {
                parts.push(p.clone());
            }
        }
        let Some(out) = out.as_mut() else { continue };
        if let Some(parts) = kept {
            if parts.is_empty() {
                continue;
            }
            out.push(Msg {
                assistant: m.assistant,
                parts,
            });
            continue;
        }
        out.push(m.clone());
    }
    out.unwrap_or_else(|| messages.to_vec())
}

fn parts_of(c: &Value) -> &[Value] {
    c.get("parts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn has_response(parts: &[Value]) -> bool {
    parts
        .iter()
        .any(|p| p.get("functionResponse").is_some_and(|f| !f.is_null()))
}

/// Gemini 读历史的要求：一个调用回合里的每个调用，紧跟着的回合里都有它的
/// 应答（pairFunctionTurns，:927）。没人答的调用（中断的回合留下的）用合成
/// 的结果补上，否则 Google 因为这一个把后面的每一轮都 400 掉。
fn pair_function_turns(input: &[Value], ag_claude: bool) -> Vec<Value> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut answered: HashSet<String> = HashSet::new();
    let mut answered_name: HashSet<String> = HashSet::new();
    for c in input {
        for p in parts_of(c) {
            let Some(fr) = p.get("functionResponse") else {
                continue;
            };
            match fr
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                Some(id) => {
                    answered.insert(id.to_string());
                }
                None => {
                    // 没有 id 的调用，由一个同样没有 id、同名的应答算答过
                    answered_name.insert(
                        fr.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    );
                }
            }
        }
    }
    let mut contents = input.to_vec();
    let mut out: Vec<Value> = Vec::with_capacity(contents.len() + 1);
    for i in 0..contents.len() {
        if contents[i].get("role").and_then(Value::as_str) != Some("model") {
            out.push(contents[i].clone());
            continue;
        }
        let mut synth: Vec<Value> = Vec::new();
        for p in parts_of(&contents[i]) {
            let Some(fc) = p.get("functionCall") else {
                continue;
            };
            let id = fc.get("id").and_then(Value::as_str).unwrap_or("");
            let name = fc.get("name").and_then(Value::as_str).unwrap_or("");
            if (!id.is_empty() && answered.contains(id))
                || (id.is_empty() && answered_name.contains(name))
            {
                continue;
            }
            let mut res = json!({ "name": name, "response": { "result": INTERRUPTED_TOOL_NOTE } });
            if !id.is_empty() {
                res["id"] = json!(id);
            }
            synth.push(json!({ "functionResponse": res }));
        }
        out.push(contents[i].clone());
        if synth.is_empty() {
            continue;
        }
        let joins = i + 1 < contents.len()
            && contents[i + 1].get("role").and_then(Value::as_str) == Some("user")
            && (ag_claude || has_response(parts_of(&contents[i + 1])));
        if joins {
            // 答案进了下面那个已经有应答的回合（Antigravity 上的 Claude 连用户
            // 说的话也留在同一个回合里，Anthropic 读作 tool_result 回合）
            let mut parts = synth;
            parts.extend_from_slice(parts_of(&contents[i + 1]));
            contents[i + 1]["parts"] = Value::Array(parts);
            continue;
        }
        out.push(json!({ "role": "user", "parts": synth }));
    }
    out
}

// =====================================================================
// Gemini 那边的图和文件（gemini_media.go:28）
// =====================================================================

/// Gemini 读的图片类型（Antigravity 给 Gemini 4 列的 supportedMimeTypes）。别的
/// 图片类型让整个调用被「Request contains an invalid argument.」拒掉，而历史
/// 里留着那张图，后面每一轮都被拒。
const GEMINI_IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/heic",
    "image/heif",
];

/// 按开头几个字节认图片类型（http.DetectContentType 的图片那几行）。
fn sniff_image_type(data: &str) -> String {
    let Some(raw) = decode_head(data) else {
        return "application/octet-stream".into();
    };
    let b = raw.as_slice();
    let is = |prefix: &[u8]| b.len() >= prefix.len() && b[..prefix.len()] == *prefix;
    if is(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        "image/png".into()
    } else if is(&[0xff, 0xd8, 0xff]) {
        "image/jpeg".into()
    } else if is(b"GIF87a") || is(b"GIF89a") {
        "image/gif".into()
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp".into()
    } else if is(b"BM") {
        "image/bmp".into()
    } else {
        "application/octet-stream".into()
    }
}

/// 解 base64 的开头一块（firstChunk，gemini_media.go:54）：够认类型就行。
fn decode_head(data: &str) -> Option<Vec<u8>> {
    let head = &data[..data.len().min(64) / 4 * 4];
    base64::engine::general_purpose::STANDARD.decode(head).ok()
}

/// 一张图或一个文件按 Gemini 收的部件发（geminiInline，:28）：类型没说或
/// 说不清的按字节认；Gemini 不读的图片类型这里转不了（见头注），发一行说明。
fn gemini_inline(media_type: &str, data: &str) -> Value {
    let mut mt = media_type.trim().to_ascii_lowercase();
    if let Some(i) = mt.find(';') {
        mt = mt[..i].trim().to_string();
    }
    if mt.is_empty() || mt == "application/octet-stream" || mt == "image" {
        let detected = sniff_image_type(data);
        if detected.starts_with("image/") || mt.is_empty() {
            mt = detected;
        }
    }
    if !mt.starts_with("image/") || GEMINI_IMAGE_TYPES.contains(&mt.as_str()) {
        let mime = if mt.is_empty() {
            media_type
        } else {
            mt.as_str()
        };
        return json!({ "inlineData": { "mimeType": mime, "data": data } });
    }
    json!({ "text": format!("[An image of type {mt} was left out: Gemini reads PNG, JPEG, WebP, HEIC and HEIF.]") })
}

fn blob_part(blob: &Blob) -> Value {
    if !blob.data.is_empty() {
        gemini_inline(&blob.media_type, &blob.data)
    } else if !blob.url.is_empty() {
        json!({ "fileData": { "mimeType": blob.media_type, "fileUri": blob.url } })
    } else {
        Value::Null
    }
}

/// 一个请求按 Antigravity 的读法折成 Gemini `request`（buildCodeAssistSent，
/// :547，agent = antigravity）。
pub(crate) fn build_request(ir: &Ir, sent: &str) -> Value {
    // id 自己说的档位（gemini-3.7-flash-low = low）：要的就是那一档，不再另说
    let at = split_antigravity_id(sent).1;
    let claude = sent.to_lowercase().contains("claude");
    let mut names: HashMap<String, String> = HashMap::new();
    let mut contents: Vec<Value> = Vec::new();
    for m in drop_second_answers(&ir.messages) {
        let role = if m.assistant { "model" } else { "user" };
        let mut parts: Vec<Value> = Vec::new();
        // after：工具带回来的图先压着，等每个应答都进了再发
        let mut after: Vec<Value> = Vec::new();
        // 哪些部件是工具带回的图
        let mut tool_image: HashSet<usize> = HashSet::new();
        for p in &m.parts {
            match p {
                Part::Text(text) => {
                    if !text.is_empty() {
                        parts.push(json!({ "text": text }));
                    }
                }
                Part::File(blob) => {
                    let part = blob_part(blob);
                    if !part.is_null() {
                        parts.push(part);
                    }
                }
                Part::ToolCall { id, name, args } => {
                    names.insert(id.clone(), name.clone());
                    let mut call = json!({ "name": gemini_func_name(name), "args": args_of(args) });
                    let tid = tool_id(id);
                    if !tid.is_empty() {
                        call["id"] = json!(tid);
                    }
                    // 思考签名不回传，一律带「别校验」（见头注差距）
                    parts.push(json!({ "functionCall": call, "thoughtSignature": SKIP_SIGNATURE }));
                }
                Part::ToolResult {
                    call_id,
                    text,
                    images,
                } => {
                    // 调用不在历史里的应答，名字记作 tool
                    let name = names
                        .get(call_id)
                        .map(|n| gemini_func_name(n))
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "tool".to_string());
                    let tid = tool_id(call_id);
                    let mut res = json!({ "name": name, "response": { "result": text } });
                    if !tid.is_empty() {
                        res["id"] = json!(tid);
                    }
                    parts.push(json!({ "functionResponse": res }));
                    // 工具带回来的图跟在它的应答后面，像 Gemini CLI 读文件那样发。
                    // Antigravity 把 Claude 模型的回合按 Anthropic 块交回去，夹在
                    // 两个应答之间的图会把它们和调用拆开（「tool_use ids were found
                    // without tool_result blocks immediately after」）：那里等每个
                    // 应答都进了，再一行说明是谁的、图跟在后头。
                    let ims: Vec<Value> = images.iter().map(blob_part).collect();
                    if claude && !ims.is_empty() {
                        let label = if ims.len() > 1 {
                            format!("{} images returned by tool {name} (call {tid}):", ims.len())
                        } else {
                            format!("Image returned by tool {name} (call {tid}):")
                        };
                        after.push(json!({ "text": label }));
                        after.extend(ims);
                    } else {
                        for im in ims {
                            tool_image.insert(parts.len());
                            parts.push(im);
                        }
                    }
                }
            }
        }
        if !after.is_empty() {
            // 应答打头（Anthropic 的 tool_result 回合必须这样），用户说的话和
            // 工具的图跟在后头
            let mut lead: Vec<Value> = Vec::new();
            let mut rest: Vec<Value> = Vec::new();
            for part in parts {
                if part.get("functionResponse").is_some() {
                    lead.push(part);
                } else {
                    rest.push(part);
                }
            }
            lead.extend(after);
            lead.extend(rest);
            parts = lead;
        }
        // 用户说的话在工具应答之后自己一个回合（#1533）：Gemini 对一个混着
        // functionResponse 和文字的回合回「Requests ending with a model turn are
        // not supported」，两个回合它分开收。Antigravity 上的 Claude 模型留在同
        // 一个回合里，就是 Anthropic 读出的 tool_result 回合。
        let mut said: Vec<Value> = Vec::new();
        if role == "user" && !claude && has_response(&parts) {
            let mut tools: Vec<Value> = Vec::new();
            for (i, part) in parts.into_iter().enumerate() {
                if part.get("functionResponse").is_some() || tool_image.contains(&i) {
                    tools.push(part);
                } else {
                    said.push(part);
                }
            }
            parts = tools;
        }
        for ps in [parts, said] {
            if ps.is_empty() {
                continue;
            }
            if let Some(last) = contents.last_mut() {
                let same_role = last.get("role").and_then(Value::as_str) == Some(role);
                let last_parts = last
                    .get("parts")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if same_role && (role != "user" || has_response(last_parts) == has_response(&ps)) {
                    last["parts"]
                        .as_array_mut()
                        .expect("parts is an array")
                        .extend(ps);
                    continue;
                }
            }
            contents.push(json!({ "role": role, "parts": ps }));
        }
    }
    let paired = pair_function_turns(&contents, claude);
    let mut contents: Vec<Value> = paired;
    // 以模型的话开头的历史被 Gemini 拒（agent 的压缩会留下这种）：前面放一个
    // 用户回合（Gemini CLI 的 hardenHistory 也这么做）
    if contents
        .first()
        .and_then(|c| c.get("role"))
        .and_then(Value::as_str)
        == Some("model")
    {
        contents.insert(0, json!({ "role": "user", "parts": [{ "text": "[Continuing from previous AI thoughts...]" }] }));
    }
    // 以模型的话收尾的也不行（Code Assist、Vertex AI、Antigravity 都一样）：后面
    // 补一个用户回合
    if contents
        .last()
        .and_then(|c| c.get("role"))
        .and_then(Value::as_str)
        == Some("model")
    {
        contents.push(json!({ "role": "user", "parts": [{ "text": model_last_ask(&ir.system, &ir.messages) }] }));
    }
    let mut req = json!({ "contents": contents });
    if !ir.system.is_empty() {
        req["systemInstruction"] = json!({ "role": "user", "parts": [{ "text": ir.system }] });
    }

    if !ir.tools.is_empty() && ir.tool_choice != "none" {
        let decls: Vec<Value> = ir
            .tools
            .iter()
            .map(|t| {
                let mut d = json!({ "name": gemini_func_name(&t.name) });
                if !t.description.is_empty() {
                    d["description"] = json!(t.description);
                }
                // Antigravity 的函数声明收精简过的 schema（plainSchema）；没有
                // 参数的给个空对象
                let schema = if t.schema.is_null() {
                    json!({ "type": "object", "properties": {} })
                } else {
                    t.schema.clone()
                };
                d["parameters"] = plain_schema(&schema);
                d
            })
            .collect();
        req["tools"] = json!([{ "functionDeclarations": decls }]);
        let mut function_calling: Map<String, Value> = Map::new();
        let mode = if ir.tool_choice == "required" {
            "ANY"
        } else if let Some(name) = ir.tool_choice.strip_prefix("name:") {
            function_calling.insert(
                "allowedFunctionNames".into(),
                json!([gemini_func_name(name)]),
            );
            "ANY"
        } else if claude {
            // Antigravity 上的 Claude 要它的调用按 schema 校验
            "VALIDATED"
        } else {
            "AUTO"
        };
        function_calling.insert("mode".into(), json!(mode));
        req["toolConfig"] = json!({ "functionCallingConfig": Value::Object(function_calling) });
    } else if ir.web_search {
        // Google 自己的搜索：只要旁边没声明函数，Code Assist 就在 Gemini 上跑它
        req["tools"] = json!([{ "googleSearch": {} }]);
    }

    let mut gen: Map<String, Value> = Map::new();
    // Antigravity 上 Gemini 模型的输出上限不发（只有 Claude 要）
    if ir.max_tokens > 0 && claude {
        gen.insert("maxOutputTokens".into(), json!(ir.max_tokens));
    }
    if let Some(t) = ir.temperature {
        gen.insert("temperature".into(), json!(t));
    }
    if let Some(t) = ir.top_p {
        gen.insert("topP".into(), json!(t));
    }
    if !ir.stop.is_empty() {
        gen.insert("stopSequences".into(), json!(ir.stop));
    }
    if let Some(tc) = thinking_config(&ir.effort, ir.think_off, ir.max_tokens, sent, claude, at) {
        // Claude 的回答在思考之后得有地方放
        if claude && !gen.contains_key("maxOutputTokens") {
            if let Some(budget) = tc.get("thinkingBudget").and_then(Value::as_u64) {
                gen.insert("maxOutputTokens".into(), json!(budget + 32000));
            }
        }
        gen.insert("thinkingConfig".into(), tc);
    }
    if let Some(format) = &ir.format {
        // 按客户端给的 schema 回 JSON
        gen.insert("responseMimeType".into(), json!("application/json"));
        if let Format::JsonSchema(schema) = format {
            gen.insert("responseJsonSchema".into(), schema.clone());
        }
    }
    if !gen.is_empty() {
        req["generationConfig"] = Value::Object(gen);
    }
    req
}

fn args_of(args: &Value) -> Value {
    if args.is_null() {
        json!({})
    } else {
        args.clone()
    }
}

// =====================================================================
// 信封（provider/google.go:1686）与会话 id（:1714）
// =====================================================================

fn random_token(n: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(b"marionette-antigravity");
    hasher.update(nanos.to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hasher.finalize()[..n.min(32)])
}

/// 会话按 Antigravity 的读法起名：取对话的第一句话，所以同一场对话每一轮同名。
pub(crate) fn antigravity_session(request: &Value) -> String {
    let mut first = String::new();
    if let Some(cs) = request.get("contents").and_then(Value::as_array) {
        for c in cs {
            if c.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            if let Some(parts) = c.get("parts").and_then(Value::as_array) {
                for p in parts {
                    if let Some(t) = p
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                    {
                        first = t.to_string();
                        break;
                    }
                }
            }
            break;
        }
    }
    if first.is_empty() {
        first = random_token(16);
    }
    let sum = Sha256::digest(first.as_bytes());
    let n = u64::from_be_bytes(sum[..8].try_into().expect("sha256 has 8 bytes"))
        & 0x7fff_ffff_ffff_ffff;
    format!("-{n}")
}

/// 把网关建的 `{model, request}` 补成账号才知道的那几样：项目、Antigravity
/// 发的 id、会话名（codeAssistEnvelope）。
pub(crate) fn envelope(request: Value, sent: &str, project: &str) -> Value {
    let mut request = request;
    if request.is_object() {
        let session = antigravity_session(&request);
        request["sessionId"] = json!(session);
    }
    json!({
        "model": sent,
        "project": project,
        "userAgent": "antigravity",
        "requestType": "agent",
        "requestId": format!("agent-{}", random_token(12)),
        "request": request,
    })
}

// =====================================================================
// 正文里写出来的调用（codeassist.go:1889–:2032）
// =====================================================================

/// `<ctrl46>` 在 Gemini 写出来的调用里给字符串加引号（ctrl46，:1883）。
const CTRL46: &str = "<ctrl46>";
/// 一段可能的开头最多压多久，再长就当正文（maxHeld，:1832）。
pub(crate) const MAX_HELD: usize = 256 << 10;

/// s 开头写出来的一个调用 `call:default_api:<tool>{<参数>}`（textCall，:1889）：
/// 工具名、参数的 JSON、它有多长。长度 0 = 可能是、但还没到齐；-1 = 不是。
pub(crate) fn text_call(s: &str) -> (String, Option<String>, isize) {
    let bytes = s.as_bytes();
    let mut j = CALL_MARKER.len();
    while j < bytes.len() && is_ident(bytes[j]) {
        j += 1;
    }
    if j == bytes.len() {
        return (String::new(), None, 0);
    }
    let name = &s[CALL_MARKER.len()..j];
    if name.is_empty() || bytes[j] != b'{' {
        return (String::new(), None, -1);
    }
    let end = close_brace(bytes, j);
    if end < 0 {
        return (String::new(), None, 0);
    }
    let Some(args) = text_args(&s[j + 1..end as usize]) else {
        return (String::new(), None, -1);
    };
    (name.to_string(), Some(args), end + 1)
}

fn is_ident(c: u8) -> bool {
    c == b'_' || c == b'-' || c == b'.' || c.is_ascii_alphanumeric()
}

/// s[i] 处开的花括号在哪闭上，`<ctrl46>` 引号里的花括号不算（closeBrace，
/// :1916）；-1 = 在 s 里没闭上。
fn close_brace(s: &[u8], i: usize) -> isize {
    let mut i = i;
    let mut depth = 0isize;
    while i < s.len() {
        if s[i..].starts_with(CTRL46.as_bytes()) {
            let Some(k) = s[i + CTRL46.len()..]
                .windows(CTRL46.len())
                .position(|w| w == CTRL46.as_bytes())
            else {
                return -1;
            };
            i += 2 * CTRL46.len() + k;
            continue;
        }
        match s[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i as isize;
                }
            }
            _ => {}
        }
        i += 1;
    }
    -1
}

/// 写出来的调用的参数做成 JSON 对象（textArgs，:1946）：JSON 自己的成员，或
/// `key:value` 对——值用 `<ctrl46>` 引着、是 JSON、或到下一个 `,key:` / 结尾
/// 为止的裸文字。
pub(crate) fn text_args(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return Some("{}".into());
    }
    let whole = format!("{{{body}}}");
    if serde_json::from_str::<Value>(&whole).is_ok() {
        return Some(whole);
    }
    let mut out = String::from("{");
    let mut body = body;
    let mut first = true;
    while !body.is_empty() {
        let bytes = body.as_bytes();
        let mut k = 0;
        while k < bytes.len() && is_ident(bytes[k]) {
            k += 1;
        }
        if k == 0 || k == bytes.len() || bytes[k] != b':' {
            return None;
        }
        let key = serde_json::to_string(&body[..k]).ok()?;
        body = &body[k + 1..];
        let val: String;
        if let Some(rest) = body.strip_prefix(CTRL46) {
            let Some(e) = rest.find(CTRL46) else {
                return None;
            };
            val = serde_json::to_string(&rest[..e]).ok()?;
            body = rest[e + CTRL46.len()..].trim_start_matches(' ');
            if !body.is_empty() {
                if !body.starts_with(',') {
                    return None;
                }
                body = body[1..].trim_start_matches(' ');
            }
        } else {
            let e = next_key(body);
            let v = body[..e].trim();
            val = if serde_json::from_str::<Value>(v).is_ok() {
                v.to_string()
            } else {
                serde_json::to_string(v).ok()?
            };
            body = &body[e..];
            if !body.is_empty() {
                body = body[1..].trim_start_matches(' ');
            }
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&key);
        out.push(':');
        out.push_str(&val);
    }
    out.push('}');
    Some(out)
}

/// 裸值到哪结束（nextKey，:2006）：括号外下一个 `,key:` 的逗号处，或结尾。
fn next_key(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut depth = 0isize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b',' if depth == 0 => {
                let mut k = i + 1;
                while k < bytes.len() && bytes[k] == b' ' {
                    k += 1;
                }
                let start = k;
                while k < bytes.len() && is_ident(bytes[k]) {
                    k += 1;
                }
                if k > start && k < bytes.len() && bytes[k] == b':' {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

#[cfg(test)]
#[path = "antigravity_codeassist_tests.rs"]
mod tests;

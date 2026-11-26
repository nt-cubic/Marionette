//! `antigravity_codeassist.rs` 的测试：schema 精简、系统提示词改写、请求构造、
//! 信封。期望值都按 magpie 的注释里点名过的用例来（括号里是它的 issue 号）。

use super::*;

/// 递归按 key 排序：serde_json 的 Map 保插入序，直接比较会受键序影响（Go 那边
/// json.Marshal 对 map 是排序输出，线上键序本来就不同）。
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canon(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        _ => v.clone(),
    }
}

fn sorted(s: &str) -> Value {
    canon(&serde_json::from_str::<Value>(s).unwrap())
}

fn ps(s: &str) -> Value {
    plain_schema(&serde_json::from_str::<Value>(s).unwrap())
}

fn ps_eq(input: &str, expected: &str) {
    assert_eq!(canon(&ps(input)), sorted(expected), "input: {input}");
}

fn empty_ir() -> Ir {
    Ir {
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: String::new(),
        web_search: false,
        max_tokens: 0,
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        effort: String::new(),
        think_off: false,
        format: None,
    }
}

// ── plainSchema ──────────────────────────────────────────────────────────

/// 子集之外的关键字整个丢掉（#187：Antigravity 遇到不认识的就拒整个请求），
/// required 只留确实有的属性。
#[test]
fn plain_schema_keeps_the_subset_and_drops_the_rest() {
    ps_eq(
        r##"{"type":"object","title":"T","format":"x","default":1,"additionalProperties":false,
            "properties":{"q":{"type":"string","description":"query","minLength":2,"default":"a"},
                          "n":{"type":"integer","exclusiveMinimum":3,"exclusiveMaximum":9,"format":"int32"}},
            "required":["q","gone"]}"##,
        r##"{"type":"object","properties":{"q":{"type":"string","description":"query","minLength":2},
                                           "n":{"type":"integer","minimum":3,"maximum":9}},
            "required":["q"]}"##,
    );
}

/// 布尔 schema：true = 任意值，false = 没有值（持有它的地方把它留掉）。
#[test]
fn plain_schema_boolean_schemas() {
    assert_eq!(ps("true"), json!(true));
    assert_eq!(ps("false"), json!(false));
    ps_eq(r##"{"type":"array","items":true}"##, r##"{"type":"array","items":{}}"##);
    // items false：什么都不装的数组
    ps_eq(r##"{"type":"array","items":false}"##, r##"{"type":"array","items":{},"maxItems":0}"##);
    ps_eq(r##"{"type":"array","items":false,"maxItems":5}"##, r##"{"type":"array","items":{},"maxItems":5}"##);
    // schema 为 false 的属性留掉，required 里它的名字也留掉
    ps_eq(
        r##"{"type":"object","properties":{"a":{},"b":false},"required":["a","b"]}"##,
        r##"{"type":"object","properties":{"a":{}},"required":["a"]}"##,
    );
    // 根不是对象：原样返回；null 是空表
    assert_eq!(ps("123"), json!(123));
    ps_eq("null", r##"{}"##);
}

/// 本地 `$ref` 展开（#4561：dsh 的工具参数里带 JSON 指针）；自引用停在目标的
/// 类型上。
#[test]
fn plain_schema_resolves_local_refs() {
    ps_eq(
        r##"{"type":"object","properties":{"a":{"$ref":"#/$defs/X"}},"$defs":{"X":{"type":"string"}}}"##,
        r##"{"type":"object","properties":{"a":{"type":"string"}}}"##,
    );
    ps_eq(
        r##"{"type":"object","properties":{"a":{"$ref":"#/properties/b/properties/c"},"b":{"type":"object","properties":{"c":{"type":"number"}}}}}"##,
        r##"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"object","properties":{"c":{"type":"number"}}}}}"##,
    );
    // 指针指到数组成员
    ps_eq(
        r##"{"type":"array","items":[{"$ref":"#/definitions/S"}],"definitions":{"S":{"type":"boolean"}}}"##,
        r##"{"type":"array","items":{"type":"boolean"}}"##,
    );
    // 自引用：引用自己所在的容器 → 只留目标的类型
    ps_eq(r##"{"type":"object","properties":{"child":{"$ref":"#"}}}"##, r##"{"type":"object","properties":{"child":{"type":"object"}}}"##);
    ps_eq(r##"{"type":"object","properties":{"next":{"$ref":"#/properties/next"}}}"##, r##"{"type":"object","properties":{"next":{"type":"object"}}}"##);
    // 递归的结构：数组里的自己
    let recursive = json!({"$defs": {"node": {"type": "object", "properties": {"kids": {"type": "array", "items": {"$ref": "#/$defs/node"}}}}}, "$ref": "#/$defs/node"});
    ps_eq(&recursive.to_string(), r##"{"type":"object","properties":{"kids":{"type":"array","items":{"type":"object"}}}}"##);
    // 指不到的引用：一个空对象
    ps_eq(r##"{"$ref":"#/nowhere"}"##, r##"{"type":"object"}"##);
}

#[test]
fn plain_schema_merges_all_of() {
    ps_eq(
        r##"{"allOf":[{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]},
                      {"properties":{"b":{"type":"number"}},"required":["b"]}]}"##,
        r##"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"number"}},"required":["a","b"]}"##,
    );
    // 自己的字段留着，成员的补上；required 去重
    ps_eq(
        r##"{"description":"d","required":["z"],"allOf":[{"required":["z","a"]},{"properties":{"a":{"type":"string"}}}]}"##,
        r##"{"description":"d","type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"##,
    );
}

/// anyOf / oneOf：没有自己的形状 → 第一个不是 null 的成员，可空（:1302）。
#[test]
fn plain_schema_unions_without_their_own_shape() {
    ps_eq(r##"{"anyOf":[{"type":"null"},{"type":"string"}],"description":"d"}"##, r##"{"type":"string","nullable":true,"description":"d"}"##);
    ps_eq(r##"{"oneOf":[{"type":"number"},{"type":"boolean"}]}"##, r##"{"type":"number"}"##);
    // 成员都不是对象：空表 + string
    ps_eq(r##"{"anyOf":["x"]}"##, r##"{"type":"string"}"##);
    // 自己的 description 不盖成员的
    ps_eq(r##"{"anyOf":[{"type":"string","description":"inner"}],"description":"outer"}"##, r##"{"type":"string","description":"inner"}"##);
}

/// 自己有形状的 schema 保留它，联合并进来（#4046：Philo 的 rg 工具是对象，
/// 它的 oneOf 只说哪些字段一起走）。
#[test]
fn plain_schema_folds_a_union_into_a_schema_with_its_own_shape() {
    ps_eq(
        r##"{"type":"object","properties":{"kind":{"type":"string"}},
            "oneOf":[{"properties":{"a":{"type":"string"}},"required":["a"]},
                     {"properties":{"b":{"type":"number"}},"required":["b"]}]}"##,
        r##"{"type":"object","properties":{"kind":{"type":"string"},"a":{"type":"string"},"b":{"type":"number"}}}"##,
    );
    // 每个非 null 分支都要求的才 required；null 分支 = 可空
    ps_eq(
        r##"{"type":"object","required":["own"],
            "anyOf":[{"type":"null"},{"properties":{"a":{}},"required":["a","x"]},{"properties":{"a":{}},"required":["a","y"]}]}"##,
        r##"{"type":"object","nullable":true,"properties":{"a":{}},"required":["a"]}"##,
    );
}

#[test]
fn plain_schema_type_lists_and_keywords_of_another_type() {
    ps_eq(r##"{"type":["string","null"]}"##, r##"{"type":"string","nullable":true}"##);
    // 列表里取 items / properties 对应的那个
    ps_eq(r##"{"type":["object","string"],"properties":{}}"##, r##"{"type":"object","properties":{}}"##);
    ps_eq(r##"{"type":["string","array"],"items":{"type":"string"}}"##, r##"{"type":"array","items":{"type":"string"}}"##);
    // 别的类型的 keywords 删掉（items 在字符串枚举上，ferock on Discord）
    ps_eq(r##"{"type":"string","items":{"type":"string"},"minItems":1,"properties":{"x":{}}}"##, r##"{"type":"string"}"##);
    ps_eq(r##"{"type":"number","properties":{"x":{}},"required":["x"],"minProperties":1}"##, r##"{"type":"number"}"##);
}

#[test]
fn plain_schema_enums() {
    // 空串留掉；全是空串就整个留掉（#5235）
    ps_eq(r##"{"type":"string","enum":["a","","  "]}"##, r##"{"type":"string","enum":["a"]}"##);
    ps_eq(r##"{"type":"string","enum":[""]}"##, r##"{"type":"string"}"##);
    // 不全是字符串：enum 整个留掉
    ps_eq(r##"{"enum":[1,2]}"##, r##"{}"##);
    // const → 单值 enum
    ps_eq(r##"{"const":"c"}"##, r##"{"enum":["c"]}"##);
    // 数组上的 enum 挪到它成员上（Go 在这里只认已经有的 items：光有 enum、还没写
    // items 的数组，enum 被拿掉而无处可放——照搬）
    ps_eq(r##"{"type":"array","items":{},"enum":["x","y"]}"##, r##"{"type":"array","items":{"type":"string","enum":["x","y"]}}"##);
    ps_eq(r##"{"type":"array","enum":["x","y"]}"##, r##"{"type":"array","items":{}}"##);
}

#[test]
fn plain_schema_arrays() {
    // 不说成员是什么：任意成员（#2826）
    ps_eq(r##"{"type":"array"}"##, r##"{"type":"array","items":{}}"##);
    // 元组成员一样：说一次；不一样：任意成员
    ps_eq(r##"{"type":"array","items":[{"type":"string"},{"type":"string"}]}"##, r##"{"type":"array","items":{"type":"string"}}"##);
    ps_eq(r##"{"type":"array","items":[{"type":"string"},{"type":"number"}]}"##, r##"{"type":"array","items":{}}"##);
    ps_eq(r##"{"type":"array","items":[{},{}]}"##, r##"{"type":"array","items":{}}"##);
    // prefixItems 说的一样：它顶上
    ps_eq(r##"{"type":"array","prefixItems":[{"type":"boolean"},{"type":"boolean"}]}"##, r##"{"type":"array","items":{"type":"boolean"}}"##);
    // 没有名字的属性留掉
    ps_eq(r##"{"type":"object","properties":{"":{"type":"string"},"ok":{"type":"string"}}}"##, r##"{"type":"object","properties":{"ok":{"type":"string"}}}"##);
}

#[test]
fn plain_schema_stops_at_the_depth_cap() {
    let mut inner = json!({"type":"string"});
    for _ in 0..40 {
        inner = json!({ "type":"object", "properties":{"next": inner} });
    }
    // 走到 33 层就只剩类型
    let plain = canon(&ps(&inner.to_string()));
    let mut at: &Value = &plain;
    for _ in 0..33 {
        at = &at["properties"]["next"];
    }
    assert_eq!(at["type"], "object");
    assert!(at.get("properties").is_none(), "{at}");
}

// ── 系统提示词改写 ────────────────────────────────────────────────────────

/// Claude Code 的计费行整行不留（#666）。
#[test]
fn cloak_drops_claude_codes_billing_line() {
    let system = "You are Claude Code.\nx-anthropic-billing-header: cc_version=2.1.63.c7e2015c; cc_entrypoint=cli\nNext line.";
    assert!(turns_away(system));
    assert_eq!(cloak_system(system), "You are Claude Code.\nNext line.");
    // 文件最后一行的计费行也带走
    assert_eq!(cloak_system("intro\nx-anthropic-billing-header: x"), "intro\n");
}

/// 各家的身份行换成同义说法（agSaidAs，:148）：Codex 的两条只换「based on」，
/// 其余「You are …」换成「You're …」。
#[test]
fn cloak_rewords_the_turned_away_lines() {
    let cases = [
        ("You are OpenCode, a coding agent.", "You're OpenCode, a coding agent."),
        ("YOU ARE HERMES AGENT, built by Nous Research.", "YOU're HERMES AGENT, built by Nous Research."),
        ("You are a coding agent running in the Codex CLI.", "You're a coding agent running in the Codex CLI."),
        ("You are Codex, an agent based on GPT-5.", "You are Codex, an agent built on GPT-5."),
        ("You are Codex, a coding agent based on GPT-5.", "You are Codex, a coding agent built on GPT-5."),
        ("built on Anthropic's Claude Agent SDK", "built on the Claude Agent SDK by Anthropic"),
        ("an intelligent AI assistant created by Nous Research", "an intelligent AI assistant built by Nous Research"),
        ("You are a coding assistant running inside pi.", "You are a coding assistant running within pi."),
        (
            "RFC 2119 keywords: must, required, should, recommended, may, optional.",
            "RFC 2119 key words: must, required, should, recommended, may, optional.",
        ),
    ];
    for (from, to) in cases {
        assert_eq!(cloak_system(from), to, "{from}");
    }
    // 词间别的空白
    assert_eq!(cloak_system("You are\tOpenCode"), "You're\tOpenCode");
    assert_eq!(cloak_system("you   are   opencode"), "you're   opencode");
    // 词内被隐形字符拆开（11-14 起 Google 读得过去，这里也认）
    assert_eq!(cloak_system("You\u{200b} are Open\u{2060}Code"), "You're OpenCode");
    assert_eq!(cloak_system("x-anthropic-billing-header:\u{200b} x\nafter"), "after");
    // 没有被拒的行：一个字不动
    let plain = "You are Claude Code, Anthropic's official CLI for Claude.\nBe concise.";
    assert!(!turns_away(plain));
    assert_eq!(cloak_system(plain), plain);
    // 同一段里好几行
    assert_eq!(
        cloak_system("You are OpenCode.\nYou are Hermes Agent.\nFine."),
        "You're OpenCode.\nYou're Hermes Agent.\nFine."
    );
}

/// 提示说的是哪一行、为什么不是额度（antigravityHint，:444）。
#[test]
fn turned_away_hint_names_the_line() {
    assert!(turned_away_hint("x-anthropic-billing-header: x").starts_with("Antigravity answers this 429 to the system prompt of Claude Code"));
    assert!(turned_away_hint("You are Codex, an agent based on GPT-5.").contains("Codex's own system prompt for GPT-5 models"));
    assert!(turned_away_hint("You are a coding agent running in the Codex CLI.").contains("a model it has none of its own for"));
    assert!(turned_away_hint("You are Hermes Agent.").contains("Hermes Agent's default persona"));
    assert!(turned_away_hint("You are OpenCode.").contains("OpenCode's system prompt"));
    assert!(turned_away_hint("a coding assistant running inside pi").contains("pi's default system prompt"));
    assert!(turned_away_hint("RFC 2119 keywords: must, required, should, recommended, may, optional.").contains("oh-my-pi"));
    assert!(refuses("Resource has been exhausted (e.g. check quota)."));
    assert!(!refuses("You have exhausted your capacity on this model."));
}

/// 用户的话里只有 Codex 的 GPT-5 那两条换；模型自己的回合和别的行不动
/// （antigravityCloakMessages，:334）。
#[test]
fn user_turns_cloak_only_the_codex_lines() {
    let codex = "You are Codex, an agent based on GPT-5. Shared workspace.";
    let mut messages = vec![
        Msg { assistant: false, parts: vec![Part::Text("You are OpenCode in a user turn stays.".into()), Part::Text(codex.into())] },
        Msg { assistant: true, parts: vec![Part::Text(codex.into())] },
        Msg { assistant: false, parts: vec![Part::ToolResult { call_id: "c".into(), text: codex.into(), images: Vec::new() }] },
        Msg { assistant: false, parts: vec![Part::Text("nothing to change".into())] },
    ];
    assert!(cloak_user_turns(&mut messages));
    assert_eq!(messages[0].parts[0], Part::Text("You are OpenCode in a user turn stays.".into()));
    assert_eq!(messages[0].parts[1], Part::Text("You are Codex, an agent built on GPT-5. Shared workspace.".into()));
    // 模型的话、工具结果原样
    assert_eq!(messages[1].parts[0], Part::Text(codex.into()));
    assert!(matches!(&messages[2].parts[0], Part::ToolResult { text, .. } if text == codex));
    assert!(!cloak_user_turns(&mut messages));
}

/// DeepSeek Harness 的那一段换成同义写法（magpie-community/plugins#1028）。
#[test]
fn reword_replaces_the_dsh_paragraph() {
    let system = format!("Intro.\n\n{}\n\nRest.", REWORDED[0].0);
    assert_eq!(super::reword(&system), format!("Intro.\n\n{}\n\nRest.", REWORDED[0].1));
}

/// 改写要跑在每一条请求的系统提示词上：Claude Code 的那份有两万字节，得是
/// 毫秒级的事。
#[test]
fn cloak_is_fast_on_a_long_system_prompt() {
    let system = "You are a helpful assistant. ".repeat(600);
    let began = std::time::Instant::now();
    assert_eq!(cloak_system(&system), system);
    assert_eq!(cloak_system(&format!("{system}You are OpenCode.")), format!("{system}You're OpenCode."));
    let elapsed = began.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}

// ── 请求构造 ─────────────────────────────────────────────────────────────

fn build(body: &Value, sent: &str) -> Value {
    build_request(&read_chat(body), sent)
}

fn contents_of(body: &Value, sent: &str) -> Value {
    build(body, sent)["contents"].clone()
}

fn call(id: &str, name: &str, args: &str) -> Value {
    json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args } })
}

fn tool(id: &str, text: &str) -> Value {
    json!({ "role": "tool", "tool_call_id": id, "content": text })
}

const SKIP: &str = "skip_thought_signature_validator";

#[test]
fn read_chat_collects_the_request() {
    let body = json!({
        "model": "antigravity/gemini-3.7-flash",
        "messages": [
            {"role": "developer", "content": "D"},
            {"role": "system", "content": [{"type": "text", "text": "S"}]},
            {"role": "user", "content": [{"type": "text", "text": "hi"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}]},
            {"role": "assistant", "content": null, "reasoning_content": "let me think", "tool_calls": [call("c1", "read_file", "{\"path\":\"a\"}")]},
            {"role": "tool", "tool_call_id": "c1", "content": [{"type": "text", "text": "saw"}, {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "/9j/"}}]},
            {"role": "user", "content": ""},
        ],
        "tools": [{"type": "function", "function": {"name": "read_file", "description": "read", "parameters": {"type": "object"}}}, {"type": "web_search"}],
        "tool_choice": {"type": "function", "function": {"name": "read_file"}},
        "max_completion_tokens": 500,
        "temperature": 0.2,
        "top_p": 0.9,
        "stop": ["END"],
        "reasoning_effort": "High",
        "response_format": {"type": "json_schema", "json_schema": {"name": "out", "schema": {"type": "object"}}},
    });
    let ir = read_chat(&body);
    assert_eq!(ir.system, "D\n\nS");
    assert_eq!(ir.messages.len(), 3, "an empty user turn is left out");
    assert_eq!(ir.messages[0].parts[1], Part::File(Blob { media_type: "image/png".into(), data: "iVBORw0KGgo=".into(), url: String::new() }));
    // 模型上一轮的思考不回去
    assert_eq!(ir.messages[1].parts, vec![Part::ToolCall { id: "c1".into(), name: "read_file".into(), args: json!({"path":"a"}) }]);
    assert_eq!(
        ir.messages[2].parts[0],
        Part::ToolResult { call_id: "c1".into(), text: "saw".into(), images: vec![Blob { media_type: "image/jpeg".into(), data: "/9j/".into(), url: String::new() }] }
    );
    assert_eq!(ir.tools.len(), 1);
    assert_eq!(ir.tool_choice, "name:read_file");
    assert!(ir.web_search);
    assert_eq!(ir.max_tokens, 500);
    assert_eq!((ir.temperature, ir.top_p), (Some(0.2), Some(0.9)));
    assert_eq!(ir.stop, vec!["END"]);
    assert_eq!(ir.effort, "high");
    assert!(!ir.think_off);
    assert_eq!(ir.format, Some(Format::JsonSchema(json!({"type":"object"}))));
    // 参数不是 JSON 对象的调用：包一层，什么都不丢
    let odd = json!({"messages": [{"role": "user", "content": "hi"}, {"role": "assistant", "tool_calls": [{"id": "c", "type": "function", "function": {"name": "f", "arguments": "just text"}}]}]});
    assert!(matches!(&read_chat(&odd).messages[1].parts[0], Part::ToolCall { args, .. } if args == &json!({"input": "just text"})));
}

#[test]
fn a_plain_turn_is_two_parts_and_a_system_instruction() {
    let body = json!({"messages": [{"role": "system", "content": "S"}, {"role": "user", "content": "hi"}]});
    assert_eq!(
        canon(&build(&body, "gemini-3.7-flash")),
        sorted(r##"{"contents":[{"role":"user","parts":[{"text":"hi"}]}],"systemInstruction":{"role":"user","parts":[{"text":"S"}]}}"##)
    );
}

/// 工具往返：调用带「别校验签名」，应答用 `result` 键、名字跟着调用。
#[test]
fn a_tool_round_trip_is_calls_and_responses() {
    let body = json!({"messages": [
        {"role": "user", "content": "list files"},
        {"role": "assistant", "tool_calls": [call("call_1", "read_file", "{\"path\":\"a b.rs\"}"), call("call/2:x", "bash", "")]},
        tool("call/2:x", "done"),
        tool("call_1", "contents"),
    ]});
    assert_eq!(
        canon(&contents_of(&body, "gemini-3.7-flash")),
        sorted(r##"[
            {"role":"user","parts":[{"text":"list files"}]},
            {"role":"model","parts":[
                {"functionCall":{"name":"read_file","args":{"path":"a b.rs"},"id":"call_1"},"thoughtSignature":"skip_thought_signature_validator"},
                {"functionCall":{"name":"bash","args":{},"id":"call_2_x"},"thoughtSignature":"skip_thought_signature_validator"}]},
            {"role":"user","parts":[
                {"functionResponse":{"name":"bash","response":{"result":"done"},"id":"call_2_x"}},
                {"functionResponse":{"name":"read_file","response":{"result":"contents"},"id":"call_1"}}]}
        ]"##)
    );
}

/// 一个回合里没人答的调用补一条合成结果（#1275）：否则后面的每一轮都被 400。
#[test]
fn an_unanswered_call_gets_a_synthetic_result() {
    let result = |call_id: &str| Msg { assistant: false, parts: vec![Part::ToolResult { call_id: call_id.into(), text: "r".into(), images: Vec::new() }] };
    let text = |t: &str| Msg { assistant: false, parts: vec![Part::Text(t.into())] };
    let calls = Msg {
        assistant: true,
        parts: vec![Part::ToolCall { id: "c1".into(), name: "a".into(), args: json!({}) }, Part::ToolCall { id: "c2".into(), name: "b".into(), args: json!({}) }],
    };
    // 后面那个回合已经有应答：合成的并进去
    let ir = Ir { messages: vec![text("hi"), calls.clone(), result("c1")], ..empty_ir() };
    let contents = build_request(&ir, "gemini-3.7-flash")["contents"].clone();
    let last = contents[2]["parts"].as_array().unwrap();
    assert_eq!(last.len(), 2, "{contents:#?}");
    assert_eq!(last[0]["functionResponse"]["id"], "c2");
    assert_eq!(last[0]["functionResponse"]["response"]["result"], INTERRUPTED_TOOL_NOTE);
    assert_eq!(last[0]["functionResponse"]["name"], "b");
    assert_eq!(last[1]["functionResponse"]["id"], "c1");
    // 后面是用户的话：合成结果自己一个回合
    let ir = Ir { messages: vec![text("hi"), calls.clone(), text("hello")], ..empty_ir() };
    let contents = build_request(&ir, "gemini-3.7-flash")["contents"].clone();
    assert_eq!(contents.as_array().unwrap().len(), 4, "{contents:#?}");
    assert_eq!(contents[2]["role"], "user");
    assert!(contents[2]["parts"][0]["functionResponse"].is_object());
    assert_eq!(contents[3]["parts"][0]["text"], "hello");
    // 后面什么都没有：合成结果补在末尾
    let ir = Ir { messages: vec![text("hi"), calls], ..empty_ir() };
    let contents = build_request(&ir, "gemini-3.7-flash")["contents"].clone();
    assert_eq!(contents.as_array().unwrap().len(), 3, "{contents:#?}");
    // 没有 id 的调用，由同名、同样没有 id 的应答算答过
    let ir = Ir {
        messages: vec![
            text("hi"),
            Msg { assistant: true, parts: vec![Part::ToolCall { id: String::new(), name: "b".into(), args: json!({}) }] },
            Msg { assistant: false, parts: vec![Part::ToolResult { call_id: String::new(), text: "r".into(), images: Vec::new() }] },
        ],
        ..empty_ir()
    };
    let contents = build_request(&ir, "gemini-3.7-flash")["contents"].clone();
    assert_eq!(contents[2]["parts"].as_array().unwrap().len(), 1, "{contents:#?}");
}

/// 一个调用被答两次：第二个不留（#2262）——Google 对不上数量就拒。
#[test]
fn a_second_answer_to_the_same_call_is_left_out() {
    let body = json!({"messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "tool_calls": [call("c1", "a", "{}")]},
        tool("c1", "first"),
        tool("c1", "second"),
        {"role": "assistant", "content": "done"},
    ]});
    let contents = contents_of(&body, "gemini-3.7-flash");
    let responses: Vec<&Value> = contents[2]["parts"].as_array().unwrap().iter().collect();
    assert_eq!(responses.len(), 1, "{contents:#?}");
    assert_eq!(responses[0]["functionResponse"]["response"]["result"], "first");
    // 同一个 id 再调一次，各答各的
    let body = json!({"messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "tool_calls": [call("c1", "a", "{}")]},
        tool("c1", "first"),
        {"role": "assistant", "tool_calls": [call("c1", "a", "{}")]},
        tool("c1", "second"),
    ]});
    let contents = contents_of(&body, "gemini-3.7-flash");
    assert_eq!(contents[4]["parts"][0]["functionResponse"]["response"]["result"], "second", "{contents:#?}");
}

/// 工具的应答和用户说的话分两个回合（#1533：混在一起 Gemini 回「Requests
/// ending with a model turn are not supported」）。
#[test]
fn tool_responses_and_what_the_user_said_are_separate_turns() {
    let ir = Ir {
        messages: vec![Msg {
            assistant: false,
            parts: vec![Part::ToolResult { call_id: "c1".into(), text: "r".into(), images: vec![] }, Part::Text("and this".into())],
        }],
        ..empty_ir()
    };
    let contents = build_request(&ir, "gemini-3.7-flash")["contents"].clone();
    assert_eq!(contents.as_array().unwrap().len(), 2, "{contents:#?}");
    assert!(contents[0]["parts"][0]["functionResponse"].is_object());
    assert_eq!(contents[1]["parts"][0]["text"], "and this");
    // Antigravity 上的 Claude 模型留在同一个回合
    let contents = build_request(&ir, "claude-sonnet-4-6")["contents"].clone();
    assert_eq!(contents.as_array().unwrap().len(), 1, "{contents:#?}");
    assert_eq!(contents[0]["parts"].as_array().unwrap().len(), 2);
}

/// 以模型的话开头或收尾的历史 Gemini 不收：各补一个用户回合。
#[test]
fn a_history_that_opens_or_ends_with_the_model_gets_a_user_turn() {
    let body = json!({"messages": [{"role": "assistant", "content": "earlier"}, {"role": "user", "content": "hi"}, {"role": "assistant", "content": "partial"}]});
    let contents = contents_of(&body, "gemini-3.7-flash");
    assert_eq!(contents.as_array().unwrap().len(), 5, "{contents:#?}");
    assert_eq!(contents[0]["parts"][0]["text"], "[Continuing from previous AI thoughts...]");
    assert_eq!(contents[1]["parts"][0]["text"], "earlier");
    assert_eq!(contents[4]["parts"][0]["text"], "Continue.");
    // 压缩请求收尾时说的是另一句
    let body = json!({"messages": [
        {"role": "system", "content": "You are a helpful AI assistant tasked with summarizing conversations."},
        {"role": "user", "content": "the conversation"},
        {"role": "assistant", "content": "so far"},
    ]});
    let contents = contents_of(&body, "gemini-3.7-flash");
    assert_eq!(contents[2]["parts"][0]["text"], "Write the summary of the conversation above that the system prompt asks for.");
}

/// 函数声明收精简过的 schema，名字按 Google 的规矩，toolConfig 按选择。
#[test]
fn tools_become_function_declarations() {
    let mut body = json!({"messages": [{"role": "user", "content": "hi"}], "tools": [
        {"type": "function", "function": {"name": "7z", "description": "archive", "parameters": {"type": "object", "properties": {"path": {"type": "string", "format": "uri"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "bare"}},
    ]});
    let req = build(&body, "gemini-3.7-flash");
    assert_eq!(
        canon(&req["tools"][0]),
        sorted(r##"{"functionDeclarations":[
            {"name":"_x_7z","description":"archive","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
            {"name":"bare","parameters":{"type":"object","properties":{}}}]}"##)
    );
    assert_eq!(req["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    body["tool_choice"] = json!("required");
    assert_eq!(build(&body, "gemini-3.7-flash")["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    body["tool_choice"] = json!({"type": "function", "function": {"name": "7z"}});
    assert_eq!(
        canon(&build(&body, "gemini-3.7-flash")["toolConfig"]),
        sorted(r##"{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["_x_7z"]}}"##)
    );
    // Antigravity 上的 Claude：调用按 schema 校验
    body["tool_choice"] = json!("auto");
    assert_eq!(build(&body, "claude-sonnet-4-6")["toolConfig"]["functionCallingConfig"]["mode"], "VALIDATED");
    // 不要工具：函数和 toolConfig 都不发
    body["tool_choice"] = json!("none");
    let req = build(&body, "gemini-3.7-flash");
    assert!(req.get("tools").is_none());
    assert!(req.get("toolConfig").is_none());
}

/// 只要 web_search 时发 Google 自己的搜索；旁边有函数就不发（它只在没函数时跑）。
#[test]
fn web_search_is_googles_own_search_only_without_functions() {
    let search = json!({"messages": [{"role": "user", "content": "hi"}], "tools": [{"type": "web_search"}]});
    assert_eq!(build(&search, "gemini-3.7-flash")["tools"], json!([{"googleSearch": {}}]));
    let both = json!({"messages": [{"role": "user", "content": "hi"}], "tools": [{"type": "web_search"}, {"type": "function", "function": {"name": "f"}}]});
    assert!(build(&both, "gemini-3.7-flash")["tools"][0].get("googleSearch").is_none());
}

/// 图：内联的按字节认类型；Gemini 不读的类型发一行说明；地址留给 fileData。
#[test]
fn images_go_inline_and_unreadable_types_are_a_note() {
    let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
    let user = |url: &str| json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": url}}]}]});
    let part = |body: &Value| build(body, "gemini-3.7-flash")["contents"][0]["parts"][0].clone();
    let data = png.split_once(',').unwrap().1.to_string();
    assert_eq!(part(&user(png)), json!({"inlineData": {"mimeType": "image/png", "data": data}}));
    // 类型没说：按字节认
    assert_eq!(part(&user(&format!("data:,{data}")))["inlineData"]["mimeType"], "image/png");
    // Gemini 不读的类型
    let gif = part(&user("data:image/gif;base64,R0lGODlh"));
    assert!(gif["text"].as_str().unwrap().starts_with("[An image of type image/gif was left out"));
    // 地址
    assert_eq!(part(&user("https://example.com/a.png")), json!({"fileData": {"mimeType": "", "fileUri": "https://example.com/a.png"}}));
}

/// 工具带回来的图：Gemini 模型跟在应答后面；Claude 模型等每个应答都进了再
/// 一行说明加图。
#[test]
fn images_a_tool_returned() {
    let saw = json!({"role": "tool", "tool_call_id": "c1", "content": [
        {"type": "text", "text": "saw"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}},
    ]});
    let body = json!({"messages": [{"role": "user", "content": "look"}, {"role": "assistant", "tool_calls": [call("c1", "look", "{}")]}, saw]});
    let parts = |sent: &str| build(&body, sent)["contents"][2]["parts"].clone();
    let gemini = parts("gemini-3.7-flash");
    assert_eq!(gemini.as_array().unwrap().len(), 2, "{gemini:#?}");
    assert!(gemini[0]["functionResponse"].is_object());
    assert!(gemini[1]["inlineData"].is_object());
    let claude = parts("claude-sonnet-4-6");
    assert_eq!(claude.as_array().unwrap().len(), 3, "{claude:#?}");
    assert!(claude[0]["functionResponse"].is_object());
    assert_eq!(claude[1]["text"], "Image returned by tool look (call c1):");
    assert!(claude[2]["inlineData"].is_object());
}

/// 想多久（thinkingConfig，:1006）。
#[test]
fn thinking_follows_the_generation_and_the_effort() {
    let thinking = |extra: Value, sent: &str| -> Option<Value> {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        build(&body, sent).pointer("/generationConfig/thinkingConfig").cloned()
    };
    let level = |l: &str| Some(json!({"includeThoughts": true, "thinkingLevel": l}));
    let budget = |b: u64| Some(json!({"includeThoughts": true, "thinkingBudget": b}));
    // Gemini 3 起说档位；没要就不说
    assert_eq!(thinking(json!({}), "gemini-3.7-flash"), None);
    assert_eq!(thinking(json!({"reasoning_effort": "low"}), "gemini-3.7-flash"), level("low"));
    // effort none → 归一成 low
    assert_eq!(thinking(json!({"reasoning_effort": "none"}), "gemini-3.7-flash"), level("low"));
    assert_eq!(thinking(json!({"reasoning_effort": "medium"}), "gemini-3.7-flash"), level("medium"));
    // 3 Pro 只收 low 和 high：medium 上到 high
    assert_eq!(thinking(json!({"reasoning_effort": "medium"}), "gemini-3.1-pro"), level("high"));
    assert_eq!(thinking(json!({"reasoning_effort": "high"}), "gemini-3.7-flash"), level("high"));
    // Gemini 3 没有 xhigh：留在 high；Gemini 4 是它自己的档
    assert_eq!(thinking(json!({"reasoning_effort": "xhigh"}), "gemini-3.7-flash"), level("high"));
    assert_eq!(thinking(json!({"reasoning_effort": "xhigh"}), "gemini-4-argon"), level("xhigh"));
    // id 自己带档位的：只说那一档（#636）
    assert_eq!(thinking(json!({"reasoning_effort": "high"}), "gemini-3.7-flash-low"), level("low"));
    assert_eq!(thinking(json!({"reasoning_effort": "low"}), "gemini-3.7-flash-xhigh"), level("xhigh"));
    // 压根没要强度：带档位的 id 也不另说（Go 先判空再读 id）
    assert_eq!(thinking(json!({}), "gemini-3.7-flash-low"), None);
    // 别的地方放的强度
    assert_eq!(thinking(json!({"reasoning": {"effort": "low"}}), "gemini-3.7-flash"), level("low"));
    assert_eq!(thinking(json!({"thinking": {"type": "enabled", "budget_tokens": 3000}}), "gemini-3.7-flash"), level("low"));
    // Gemini 2.5 说预算；关掉按 2.5 的关掉预算
    assert_eq!(thinking(json!({"reasoning_effort": "high"}), "gemini-2.5-flash"), budget(24000));
    assert_eq!(thinking(json!({"reasoning_effort": "none"}), "gemini-2.5-flash"), Some(json!({"thinkingBudget": 0})));
    // 不思考的模型不说
    assert_eq!(thinking(json!({"reasoning_effort": "high"}), "gpt-oss-120b-medium"), None);
}

/// Claude 模型在 Antigravity 上的几处不同：输出上限发；会思考的得说想多少，
/// 预算之外再给地方放回答；预算不能盖过输出上限。
#[test]
fn claude_models_on_antigravity() {
    let gen = |sent: &str, extra: Value| -> Value {
        let mut body = json!({"messages": [{"role": "user", "content": "hi"}]});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        build(&body, sent)["generationConfig"].clone()
    };
    // 会思考的 Claude（id 带 thinking）：没要强度也想
    assert_eq!(
        canon(&gen("claude-opus-4-6-thinking", json!({}))),
        sorted(r##"{"thinkingConfig":{"includeThoughts":true,"thinkingBudget":10000},"maxOutputTokens":42000}"##)
    );
    // 自己给了上限：用它
    assert_eq!(gen("claude-opus-4-6-thinking", json!({"max_tokens": 50000, "reasoning_effort": "high"}))["maxOutputTokens"], 50000);
    // 预算盖过上限：收到上限下面一点
    let g = gen("claude-opus-4-6-thinking", json!({"max_tokens": 20000, "reasoning_effort": "high"}));
    assert_eq!(g["thinkingConfig"]["thinkingBudget"], 19999);
    assert_eq!(g["maxOutputTokens"], 20000);
    // 上限太小、预算没处放：不说思考
    assert_eq!(gen("claude-opus-4-6-thinking", json!({"max_tokens": 500, "reasoning_effort": "high"})), json!({"maxOutputTokens": 500}));
    // 不带 thinking 的 Claude 模型不思考；输出上限照发
    assert_eq!(gen("claude-sonnet-4-6", json!({"reasoning_effort": "high", "max_tokens": 1000})), json!({"maxOutputTokens": 1000}));
    // Gemini 模型不收输出上限
    let body = json!({"messages": [{"role": "user", "content": "hi"}], "max_tokens": 100});
    assert!(build(&body, "gemini-3.7-flash")["generationConfig"].get("maxOutputTokens").is_none());
}

#[test]
fn sampling_and_format_go_in_the_generation_config() {
    let body = json!({
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": 0.2, "top_p": 0.9, "stop": "END", "max_tokens": 99,
        "response_format": {"type": "json_schema", "json_schema": {"name": "out", "schema": {"type": "object", "properties": {"a": {"type": "string"}}}}},
    });
    assert_eq!(
        canon(&build(&body, "gemini-3.7-flash")["generationConfig"]),
        sorted(r##"{"temperature":0.2,"topP":0.9,"stopSequences":["END"],
            "responseMimeType":"application/json","responseJsonSchema":{"type":"object","properties":{"a":{"type":"string"}}}}"##)
    );
    // 没有形状的 json_schema 就是任意 JSON 对象；文本格式什么都不发
    let body = json!({"messages": [{"role": "user", "content": "hi"}], "response_format": {"type": "json_schema", "json_schema": {"name": "out"}}});
    assert_eq!(build(&body, "gemini-3.7-flash")["generationConfig"], json!({"responseMimeType": "application/json"}));
    let body = json!({"messages": [{"role": "user", "content": "hi"}], "response_format": {"type": "text"}});
    assert!(build(&body, "gemini-3.7-flash").get("generationConfig").is_none());
}

// ── 信封 ─────────────────────────────────────────────────────────────────

#[test]
fn split_ids_into_family_and_level() {
    assert_eq!(split_antigravity_id("gemini-3.7-flash-low"), ("gemini-3.7-flash", "low"));
    assert_eq!(split_antigravity_id("gemini-3.5-flash-extra-low"), ("gemini-3.5-flash", "minimal"));
    assert_eq!(split_antigravity_id("gemini-4-hydrogen-extra-high"), ("gemini-4-hydrogen", "xhigh"));
    assert_eq!(split_antigravity_id("gemini-3.7-flash"), ("gemini-3.7-flash", ""));
    assert_eq!(split_antigravity_id("gpt-oss-120b-medium"), ("gpt-oss-120b", "medium"));
    assert_eq!(split_antigravity_id("-low"), ("-low", ""));
}

#[test]
fn envelope_carries_what_the_account_knows() {
    let request = json!({"contents": [{"role": "user", "parts": [{"text": "first question"}]}]});
    let env = envelope(request, "gemini-3.7-flash", "proj-1");
    assert_eq!(env["model"], "gemini-3.7-flash");
    assert_eq!(env["project"], "proj-1");
    assert_eq!(env["userAgent"], "antigravity");
    assert_eq!(env["requestType"], "agent");
    assert!(env["requestId"].as_str().unwrap().starts_with("agent-"));
    assert_eq!(env["requestId"].as_str().unwrap().len(), "agent-".len() + 16);
    let session = env["request"]["sessionId"].as_str().unwrap().to_string();
    assert!(session.starts_with('-') && session[1..].chars().all(|c| c.is_ascii_digit()), "{session}");
    // 同一场对话每一轮同名；别的话另一个名字
    let later = json!({"contents": [{"role": "user", "parts": [{"text": "first question"}]}, {"role": "model", "parts": [{"text": "answer"}]}, {"role": "user", "parts": [{"text": "second question"}]}]});
    assert_eq!(envelope(later, "gemini-3.7-flash", "proj-1")["request"]["sessionId"].as_str().unwrap(), session);
    let other = envelope(json!({"contents": [{"role": "user", "parts": [{"text": "another chat"}]}]}), "gemini-3.7-flash", "proj-1");
    assert_ne!(other["request"]["sessionId"].as_str().unwrap(), session);
}

#[test]
fn names_follow_googles_rules() {
    assert_eq!(gemini_func_name("bash"), "bash");
    assert_eq!(gemini_func_name("7z"), "_x_7z");
    assert_eq!(gemini_func_name("-x"), "_x_-x");
    assert_eq!(gemini_func_name("mcp__server__tool"), "mcp__server__tool");
    assert_eq!(tool_of_call("default_api:bash"), "bash");
    assert_eq!(tool_of_call("default_api.search"), "search");
    assert_eq!(tool_of_call("_x_7z"), "7z");
    assert_eq!(tool_of_call("default_api:_x_7z"), "7z");
    assert_eq!(tool_of_call("plain"), "plain");
    assert_eq!(tool_of_call("default_api:"), "default_api:");
}

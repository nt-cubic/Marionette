//! CLI 动词（合同 C6）——magpie 种子同形：serve / provider / group / models。
//! 进程内形态下这些不再是子进程调用，而是 engine.rs 直调的函数；开发/测试
//! 二进制（engine_gateway_bin.rs）经 argv 暴露同一套，让合同套件可以拿
//! exe 形态验收。
//!
//! 输出红线：**动词实现绝不直接 println!/eprintln!**——engine.rs 在 GUI
//! 进程里直调这套函数，而 GUI 进程是 windows 子系统、没有控制台，写 stdout
//! 会 panic。信息行统一经 `out` 回调下发（argv 形态 = println，进程内 =
//! 丢弃），错误一律 `Err(String)`（argv 层补 `magpie: ` 前缀打 stderr +
//! 退出码；engine.rs 拿到的错误串与旧 CLI stderr 同文，含 "no provider"
//! 等对账判据）。
use super::store::{Group, Provider};

/// argv 形态入口（开发/合同套件 bin 用）：信息打 stdout、错误打 stderr、
/// 退出码 0/1。内部完全复用程序化入口 [`run_with`]。
pub fn main(args: Vec<String>) -> i32 {
    let mut print = |line: &str| println!("{line}");
    match run_with(&args, &mut print) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("magpie: {error}");
            1
        }
    }
}

/// 程序化入口（engine.rs 的 `run_magpie_cli` 在 engine-rust 形态下直调）：
/// 静默成功、`Err` 失败。注意 `serve` 动词会阻塞到网关退出——进程内形态
/// 别传它（engine.rs 直接起 httpd 线程，不走 CLI 分发）。
pub fn run(args: &[String]) -> Result<(), String> {
    run_with(args, &mut |_| {})
}

/// 动词分发的共同实现：`out` 收信息行（argv → stdout；进程内 → 丢弃）。
fn run_with(args: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let Some(first) = args.first() else {
        return serve();
    };
    match first.as_str() {
        "-v" | "--version" | "version" => {
            out(&format!("magpie {}", super::ENGINE_VERSION));
            Ok(())
        }
        "-h" | "--help" | "help" => {
            out(&format!(
                "magpie {} · Marionette engine (rust, in-process)",
                super::ENGINE_VERSION
            ));
            out("usage: magpie [serve]        (env MAGPIE_ADDR=127.0.0.1:<port>)");
            out("       magpie provider add|set|key|rm|refresh|models …");
            out("       magpie group add|rm …");
            out("       magpie models");
            Ok(())
        }
        "serve" => serve(),
        "provider" => with_store(|store| provider_cli(store, &args[1..], out)),
        "group" => with_store(|store| group_cli(store, &args[1..])),
        "models" => with_gateway(|gw| {
            let store = gw.store.lock().expect("store");
            for provider in store.providers() {
                for row in gw.catalog.listed(provider) {
                    out(&format!("{}/{}", provider.id, row.id));
                }
            }
            Ok(())
        }),
        other => Err(format!("{other} is not in the Marionette engine (rust)")),
    }
}

fn serve() -> Result<(), String> {
    let gateway = std::sync::Arc::new(super::Gateway::open()?);
    let addr = std::env::var("MAGPIE_ADDR").unwrap_or_else(|_| "127.0.0.0:0".into());
    // 默认随机端口：绑定后打印实际地址（种子同款行为，打印在 httpd::serve 里）。
    let addr = if addr.ends_with(":0") { "127.0.0.1:0".to_string() } else { addr };
    super::httpd::serve(&addr, gateway).map_err(|e| format!("serve: {e}"))
}

/// 装载 store → 跑动词 → **仅成功才落盘**（失败路径不写半截状态，与旧
/// argv 形态「退出码非 0 不 save」同语义）。
fn with_store(run: impl FnOnce(&mut super::store::Store) -> Result<(), String>) -> Result<(), String> {
    let mut store = super::store::Store::load()?;
    run(&mut store)?;
    store.save()
}

fn with_gateway(run: impl FnOnce(&super::Gateway) -> Result<(), String>) -> Result<(), String> {
    let gw = super::Gateway::open()?;
    run(&gw)
}

fn provider_cli(
    store: &mut super::store::Store,
    args: &[String],
    out: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let Some(verb) = args.first() else {
        return Err("provider add|set|key|rm|refresh|models …".into());
    };
    let rest = &args[1..];
    match verb.as_str() {
        "add" | "new" => provider_add(store, rest, out),
        "set" => {
            let Some(id) = rest.first() else {
                return Err("provider set <id> k=v…".into());
            };
            let Some(provider) = store.find_mut(id) else {
                return Err(format!("no provider \"{id}\""));
            };
            let mut holder = provider.clone();
            apply_pairs(&mut holder, &rest[1..])?;
            store.upsert(holder);
            Ok(())
        }
        "key" => {
            if rest.len() != 2 {
                return Err("provider key <id> <key>".into());
            }
            let Some(provider) = store.find_mut(&rest[0]) else {
                return Err(format!("no provider \"{}\"", rest[0]));
            };
            provider.key = rest[1].clone();
            Ok(())
        }
        "rm" | "remove" | "delete" => {
            let Some(id) = rest.first() else {
                return Err("provider rm <id>".into());
            };
            if store.remove(id) {
                Ok(())
            } else {
                Err(format!("no provider \"{id}\""))
            }
        }
        "refresh" | "fetch" => refresh(store, rest, out),
        "models" => {
            let Some(id) = rest.first() else {
                return Err("provider models <id> [all|ids…]".into());
            };
            if rest.len() == 1 {
                return refresh(store, rest, out);
            }
            let Some(provider) = store.find_mut(id) else {
                return Err(format!("no provider \"{id}\""));
            };
            provider.models = if rest.len() == 2 && rest[1] == "all" {
                None
            } else {
                Some(rest[1..].to_vec())
            };
            Ok(())
        }
        other => Err(format!("{other} is not in the Marionette engine (rust)")),
    }
}

fn provider_add(
    store: &mut super::store::Store,
    rest: &[String],
    out: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let Some(name) = rest.first() else {
        return Err("provider add <name> [key] k=v…".into());
    };
    let mut provider = Provider {
        name: name.clone(),
        ..Default::default()
    };
    let mut pairs: Vec<String> = rest[1..].to_vec();
    // 裸 key 形态（provider add <name> <key>）。
    if let Some(second) = pairs.first() {
        if !second.contains('=') && !second.starts_with('-') {
            provider.key = second.clone();
            pairs.remove(0);
        }
    }
    apply_pairs(&mut provider, &pairs)?;
    if provider.id.is_empty() {
        provider.id = super::store::id_slug(&provider.name);
    }
    let id = provider.id.clone();
    let shown = provider.name.clone();
    store.upsert(provider);
    out(&format!("added {shown} ({id})"));
    refresh(store, &[id], out)
}

fn refresh(
    store: &mut super::store::Store,
    rest: &[String],
    out: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let Some(id) = rest.first() else {
        return Err("provider refresh <id>".into());
    };
    let Some(provider) = store.find(id) else {
        return Err(format!("no provider \"{id}\""));
    };
    let catalog = super::catalog::Catalog::default();
    match catalog.vendor_rows(provider, true) {
        Ok(rows) => {
            out(&format!("{} models from {}", rows.len(), id));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// k=v 解析（种子 seedApplyPairs 同集）：id/name/url|chat|openai/responses/
/// anthropic/gemini/key/models/catalog/header.*。
fn apply_pairs(provider: &mut Provider, pairs: &[String]) -> Result<(), String> {
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("expected key=value, got {:?}", pair));
        };
        match key.to_ascii_lowercase().as_str() {
            "id" => provider.id = value.to_string(),
            "name" => provider.name = value.to_string(),
            "url" | "chat" | "openai" => provider.chat = value.to_string(),
            "responses" => provider.responses = value.to_string(),
            "anthropic" => provider.anthropic = value.to_string(),
            "gemini" => provider.gemini = value.to_string(),
            "key" => provider.key = value.to_string(),
            "models" | "model" => {
                provider.models = Some(
                    value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                );
            }
            "catalog" => provider.catalog = value.to_string(),
            other => {
                if let Some(header) = other.strip_prefix("header.") {
                    if value.trim().is_empty() {
                        provider.headers.remove(header);
                    } else {
                        provider.headers.insert(header.to_string(), value.to_string());
                    }
                } else {
                    return Err(format!("unknown field {:?}", other));
                }
            }
        }
    }
    Ok(())
}

fn group_cli(store: &mut super::store::Store, args: &[String]) -> Result<(), String> {
    let Some(verb) = args.first() else {
        return Err("group add <name> models=<m> [id=] [pick=]  |  group rm <id>".into());
    };
    match verb.as_str() {
        "add" | "new" => {
            let Some(name) = args.get(1) else {
                return Err("group add <name> models=<m> [id=] [pick=]".into());
            };
            let mut group = Group {
                name: name.trim().to_string(),
                routing: "manual".into(),
                ..Default::default()
            };
            for pair in &args[2..] {
                let Some((key, value)) = pair.split_once('=') else {
                    return Err(format!("group add: want k=v, got {:?}", pair));
                };
                match key.to_ascii_lowercase().as_str() {
                    "models" | "model" => {
                        group.members = value
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    "id" => group.id = value.trim().to_ascii_lowercase(),
                    "pick" | "use" => {
                        group.pick = value.trim().to_string();
                        group.routing = "manual".into();
                    }
                    other => {
                        return Err(format!("group add: unknown field {:?}", other));
                    }
                }
            }
            if !group.pick.is_empty() && !group.members.contains(&group.pick) {
                group.members.insert(0, group.pick.clone());
            }
            if group.members.is_empty() {
                return Err("group add: models=<provider/model> is required".into());
            }
            if group.id.is_empty() {
                group.id = super::store::id_slug(&group.name);
            }
            if group.name.trim().is_empty() {
                group.name = group.id.clone();
            }
            store.upsert_group(group);
            Ok(())
        }
        "rm" | "remove" | "delete" => {
            let Some(id) = args.get(1) else {
                return Err("group rm <id>".into());
            };
            if store.remove_group(id) {
                Ok(())
            } else {
                Err(format!("no group \"{id}\""))
            }
        }
        other => Err(format!("{other} is not in the Marionette engine (rust)")),
    }
}

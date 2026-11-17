//! CLI 动词（合同 C6）——magpie 种子同形：serve / provider / group / models。
//! 进程内形态下这些不再是子进程调用，而是 engine.rs 直调的函数；开发/测试
//! 二进制（engine_gateway_bin.rs）经 argv 暴露同一套，让合同套件可以拿
//! exe 形态验收。
use super::store::{Group, Provider};

pub fn main(args: Vec<String>) -> i32 {
    let Some(first) = args.first() else {
        return serve();
    };
    match first.as_str() {
        "-v" | "--version" | "version" => {
            println!("magpie {}", super::ENGINE_VERSION);
            0
        }
        "-h" | "--help" | "help" => {
            println!("magpie {} · Marionette engine (rust, in-process)", super::ENGINE_VERSION);
            println!("usage: magpie [serve]        (env MAGPIE_ADDR=127.0.0.1:<port>)");
            println!("       magpie provider add|set|key|rm|refresh|models …");
            println!("       magpie group add|rm …");
            println!("       magpie models");
            0
        }
        "serve" => serve(),
        "provider" => with_store(|store| provider_cli(store, &args[1..])),
        "group" => with_store(|store| group_cli(store, &args[1..])),
        "models" => with_gateway(|gw| {
            let store = gw.store.lock().expect("store");
            let mut n = 0;
            for provider in store.providers() {
                for row in gw.catalog.listed(provider) {
                    println!("{}/{}", provider.id, row.id);
                    n += 1;
                }
            }
            let _ = n;
            0
        }),
        other => {
            eprintln!("magpie: {} is not in the Marionette engine (rust)", other);
            2
        }
    }
}

fn serve() -> i32 {
    let gateway = match super::Gateway::open() {
        Ok(g) => std::sync::Arc::new(g),
        Err(e) => {
            eprintln!("magpie: {}", e);
            return 1;
        }
    };
    let addr = std::env::var("MAGPIE_ADDR").unwrap_or_else(|_| "127.0.0.0:0".into());
    // 默认随机端口：绑定后打印实际地址（种子同款行为）。
    let addr = if addr.ends_with(":0") { "127.0.0.1:0".to_string() } else { addr };
    if let Err(e) = super::httpd::serve(&addr, gateway) {
        eprintln!("serve: {}", e);
        return 1;
    }
    0
}

fn with_store(run: impl FnOnce(&mut super::store::Store) -> i32) -> i32 {
    match super::store::Store::load() {
        Ok(mut store) => {
            let code = run(&mut store);
            if code == 0 {
                if let Err(e) = store.save() {
                    eprintln!("magpie: {}", e);
                    return 1;
                }
            }
            code
        }
        Err(e) => {
            eprintln!("magpie: {}", e);
            1
        }
    }
}

fn with_gateway(run: impl FnOnce(&super::Gateway) -> i32) -> i32 {
    match super::Gateway::open() {
        Ok(gw) => run(&gw),
        Err(e) => {
            eprintln!("magpie: {}", e);
            1
        }
    }
}

fn provider_cli(store: &mut super::store::Store, args: &[String]) -> i32 {
    let Some(verb) = args.first() else {
        eprintln!("magpie: provider add|set|key|rm|refresh|models …");
        return 2;
    };
    let rest = &args[1..];
    match verb.as_str() {
        "add" | "new" => provider_add(store, rest),
        "set" => {
            let Some(id) = rest.first() else {
                eprintln!("magpie: provider set <id> k=v…");
                return 2;
            };
            let Some(provider) = store.find_mut(id) else {
                eprintln!("magpie: no provider \"{}\"", id);
                return 1;
            };
            let mut holder = provider.clone();
            if let Err(e) = apply_pairs(&mut holder, &rest[1..]) {
                eprintln!("magpie: {}", e);
                return 1;
            }
            store.upsert(holder);
            0
        }
        "key" => {
            if rest.len() != 2 {
                eprintln!("magpie: provider key <id> <key>");
                return 2;
            }
            let Some(provider) = store.find_mut(&rest[0]) else {
                eprintln!("magpie: no provider \"{}\"", rest[0]);
                return 1;
            };
            provider.key = rest[1].clone();
            0
        }
        "rm" | "remove" | "delete" => {
            let Some(id) = rest.first() else {
                eprintln!("magpie: provider rm <id>");
                return 2;
            };
            if store.remove(id) {
                0
            } else {
                eprintln!("magpie: no provider \"{}\"", id);
                1
            }
        }
        "refresh" | "fetch" => refresh(store, rest),
        "models" => {
            let Some(id) = rest.first() else {
                eprintln!("magpie: provider models <id> [all|ids…]");
                return 2;
            };
            if rest.len() == 1 {
                return refresh(store, rest);
            }
            let Some(provider) = store.find_mut(id) else {
                eprintln!("magpie: no provider \"{}\"", id);
                return 1;
            };
            provider.models = if rest.len() == 2 && rest[1] == "all" {
                None
            } else {
                Some(rest[1..].to_vec())
            };
            0
        }
        other => {
            eprintln!("magpie: provider {} is not in the Marionette engine (rust)", other);
            2
        }
    }
}

fn provider_add(store: &mut super::store::Store, rest: &[String]) -> i32 {
    let Some(name) = rest.first() else {
        eprintln!("magpie: provider add <name> [key] k=v…");
        return 2;
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
    if let Err(e) = apply_pairs(&mut provider, &pairs) {
        eprintln!("magpie: {}", e);
        return 1;
    }
    if provider.id.is_empty() {
        provider.id = super::store::id_slug(&provider.name);
    }
    let id = provider.id.clone();
    let shown = provider.name.clone();
    store.upsert(provider);
    println!("added {} ({})", shown, id);
    refresh(store, &[id])
}

fn refresh(store: &mut super::store::Store, rest: &[String]) -> i32 {
    let Some(id) = rest.first() else {
        eprintln!("magpie: provider refresh <id>");
        return 2;
    };
    let Some(provider) = store.find(id) else {
        eprintln!("magpie: no provider \"{}\"", id);
        return 1;
    };
    let catalog = super::catalog::Catalog::default();
    match catalog.vendor_rows(provider, true) {
        Ok(rows) => {
            println!("{} models from {}", rows.len(), id);
            0
        }
        Err(e) => {
            eprintln!("magpie: {}", e);
            1
        }
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

fn group_cli(store: &mut super::store::Store, args: &[String]) -> i32 {
    let Some(verb) = args.first() else {
        eprintln!("magpie: group add <name> models=<m> [id=] [pick=]  |  group rm <id>");
        return 2;
    };
    match verb.as_str() {
        "add" | "new" => {
            let Some(name) = args.get(1) else {
                eprintln!("magpie: group add <name> models=<m> [id=] [pick=]");
                return 2;
            };
            let mut group = Group {
                name: name.trim().to_string(),
                routing: "manual".into(),
                ..Default::default()
            };
            for pair in &args[2..] {
                let Some((key, value)) = pair.split_once('=') else {
                    eprintln!("magpie: group add: want k=v, got {:?}", pair);
                    return 1;
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
                        eprintln!("magpie: group add: unknown field {:?}", other);
                        return 1;
                    }
                }
            }
            if !group.pick.is_empty() && !group.members.contains(&group.pick) {
                group.members.insert(0, group.pick.clone());
            }
            if group.members.is_empty() {
                eprintln!("magpie: group add: models=<provider/model> is required");
                return 1;
            }
            if group.id.is_empty() {
                group.id = super::store::id_slug(&group.name);
            }
            if group.name.trim().is_empty() {
                group.name = group.id.clone();
            }
            store.upsert_group(group);
            0
        }
        "rm" | "remove" | "delete" => {
            let Some(id) = args.get(1) else {
                eprintln!("magpie: group rm <id>");
                return 2;
            };
            if store.remove_group(id) {
                0
            } else {
                eprintln!("magpie: no group \"{}\"", id);
                1
            }
        }
        other => {
            eprintln!("magpie: group {} is not in the Marionette engine (rust)", other);
            2
        }
    }
}

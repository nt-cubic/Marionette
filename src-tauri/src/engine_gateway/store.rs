//! providers.json 存储——与 magpie 逐字段互通（docs/plans/plan-engine-contract.md
//! C6）。未知字段用 `serde(flatten)` 原样保留：我们重写文件时不丢 magpie
//! 认的任何东西。原子写（tmp + rename），读失败 fail-open 成空表。
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Map;

/// providers.json 顶层（magpie internal/provider file struct）。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProvidersFile {
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// magpie 的 searches/noAutoGroups/autoGroupSkip/order 等字段原样保留。
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

/// 一个来源。字段名与 magpie Provider 的 json tag 一一对应；`models: None`
/// = 未钉死（live 清单语义），`Some(list)` = 钉死。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub key: String,
    /// OpenAI 兼容 chat 基址（含 /v1）。
    #[serde(default)]
    pub chat: String,
    #[serde(default)]
    pub responses: String,
    #[serde(default)]
    pub anthropic: String,
    #[serde(default)]
    pub gemini: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preset: String,
    /// models.dev 目录 id（名字/窗口兜底用）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog: String,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

impl Provider {
    /// 出站用基址：chat 优先，退 responses / anthropic 的主机根。
    pub fn chat_base(&self) -> Option<&str> {
        for url in [&self.chat, &self.responses] {
            if !url.is_empty() {
                return Some(url.trim_end_matches('/'));
            }
        }
        if !self.anthropic.is_empty() {
            return Some(self.anthropic.trim_end_matches('/'));
        }
        None
    }
}

/// 路由组（magpie Group）。members 是 `provider/model` 形态；pick 是手动
/// 选中的成员。id/name/note/members/former/match 的 json tag 与 magpie
/// 一致，其余字段 flatten 保留。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pick: String,
    /// magpie 的 Routing（"manual"）；未知值 flatten 之外单存。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing: String,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

pub struct Store {
    path: PathBuf,
    file: ProvidersFile,
}

impl Store {
    /// 从 config_dir() 读；没有目录/文件 → 空 store（首次运行）。
    pub fn load() -> Result<Store, String> {
        let dir = super::config_dir().ok_or("no config dir (set USERPROFILE)")?;
        let path = dir.join("providers.json");
        let file = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ProvidersFile>(&b).ok())
            .unwrap_or_default();
        Ok(Store { path, file })
    }

    /// 从磁盘重读（magpie heldFresh 同语义：跑着的网关在下一个请求看到
    /// CLI/别的进程写进 providers.json 的变更）。读失败保持旧内容。
    pub fn reload(&mut self) {
        if let Some(file) = std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ProvidersFile>(&b).ok())
        {
            self.file = file;
        }
    }

    /// 原子写（tmp + rename；同进程同目录，tmp 名带 pid 防互吞）。
    pub fn save(&self) -> Result<(), String> {
        let body = serde_json::to_vec_pretty(&self.file).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension(format!("json.tmp{}", std::process::id()));
        std::fs::write(&tmp, &body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn find(&self, id: &str) -> Option<&Provider> {
        self.file.providers.iter().find(|p| p.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Provider> {
        self.file.providers.iter_mut().find(|p| p.id == id)
    }

    /// upsert：同 id 覆盖（engine.rs 注册链是幂等重放，不是报错）。
    pub fn upsert(&mut self, provider: Provider) {
        match self.file.providers.iter().position(|p| p.id == provider.id) {
            Some(i) => self.file.providers[i] = provider,
            None => self.file.providers.push(provider),
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.file.providers.len();
        self.file.providers.retain(|p| p.id != id);
        self.file.providers.len() != before
    }

    pub fn providers(&self) -> &[Provider] {
        &self.file.providers
    }

    /// 组 upsert（按 id；`group add` 的 effort 重钉/改绑语义）。
    pub fn upsert_group(&mut self, group: Group) {
        match self.file.groups.iter().position(|g| g.id == group.id) {
            Some(i) => self.file.groups[i] = group,
            None => self.file.groups.push(group),
        }
    }

    pub fn remove_group(&mut self, id: &str) -> bool {
        let before = self.file.groups.len();
        self.file.groups.retain(|g| g.id != id);
        self.file.groups.len() != before
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.file.groups.iter().find(|g| g.id == id || g.name == id)
    }

    pub fn groups(&self) -> &[Group] {
        &self.file.groups
    }
}

/// provider id slug（magpie provider_id_slug 同规则：小写字母/数字/横线）。
pub fn id_slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

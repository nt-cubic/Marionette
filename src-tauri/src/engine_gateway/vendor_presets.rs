//! 内置厂商 preset 表（2026-11-18 T13 真机首验补）——magpie
//! `internal/provider/presets.go` 的 Rust 子集。为什么需要：注册链的
//! preset 来源（engine.rs `magpie_provider_for_preset`：deepseek/kimi/glm/
//! openrouter/xai/opencode-go）走 `provider add deepseek <key>`——**不带
//! url**，端点知识来自内置表；没有它 add 出来的行没有 URL，目录/额度/
//! 对话全线 404（用户真机实锤：`deepseek has no endpoint configured`、
//! `zhipu unknown field "region"`）。端点逐条照抄上游 presets.go /
//! opencode_endpoints.go（2026-11-17 克隆），改端点先对上游。
//!
//! 刻意只收 Marionette 注册链实际用到的家族 + zai（GLM 双家）；
//! 表外的名字按自定义来源处理（带 url 的老路径不变）。

/// 一个可选 region/plan（zhipu 的 coding 等）——选中即整组覆盖端点。
pub struct Region {
    pub id: &'static str,
    pub chat: &'static str,
    pub responses: &'static str,
    pub anthropic: &'static str,
}

pub struct VendorPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub chat: &'static str,
    pub responses: &'static str,
    pub anthropic: &'static str,
    pub regions: &'static [Region],
}

pub fn find(id: &str) -> Option<&'static VendorPreset> {
    TABLE.iter().find(|p| p.id.eq_ignore_ascii_case(id))
}

pub static TABLE: &[VendorPreset] = &[
    VendorPreset {
        id: "deepseek",
        name: "DeepSeek",
        chat: "https://api.deepseek.com/v1",
        responses: "https://api.deepseek.com/v1",
        anthropic: "https://api.deepseek.com/anthropic",
        regions: &[],
    },
    VendorPreset {
        id: "moonshot",
        name: "Kimi",
        chat: "https://api.moonshot.ai/v1",
        responses: "",
        anthropic: "https://api.moonshot.ai/anthropic",
        regions: &[],
    },
    VendorPreset {
        id: "zhipu",
        name: "Zhipu GLM",
        chat: "https://open.bigmodel.cn/api/paas/v4",
        responses: "",
        anthropic: "https://open.bigmodel.cn/api/anthropic",
        regions: &[
            Region {
                id: "api",
                chat: "https://open.bigmodel.cn/api/paas/v4",
                responses: "",
                anthropic: "https://open.bigmodel.cn/api/anthropic",
            },
            Region {
                id: "coding",
                chat: "https://open.bigmodel.cn/api/coding/paas/v4",
                responses: "https://open.bigmodel.cn/api/v1",
                anthropic: "https://open.bigmodel.cn/api/anthropic",
            },
        ],
    },
    VendorPreset {
        id: "zai",
        name: "Z.ai",
        chat: "https://api.z.ai/api/paas/v4",
        responses: "",
        anthropic: "https://api.z.ai/api/anthropic",
        regions: &[
            Region {
                id: "api",
                chat: "https://api.z.ai/api/paas/v4",
                responses: "",
                anthropic: "https://api.z.ai/api/anthropic",
            },
            Region {
                id: "coding",
                chat: "https://api.z.ai/api/coding/paas/v4",
                responses: "https://api.z.ai/api/v1",
                anthropic: "https://api.z.ai/api/anthropic",
            },
        ],
    },
    VendorPreset {
        id: "openrouter",
        name: "OpenRouter",
        chat: "https://openrouter.ai/api/v1",
        responses: "",
        anthropic: "https://openrouter.ai/api",
        regions: &[],
    },
    VendorPreset {
        id: "xai",
        name: "xAI",
        chat: "https://api.x.ai/v1",
        responses: "https://api.x.ai/v1",
        anthropic: "https://api.x.ai",
        regions: &[],
    },
    VendorPreset {
        id: "opencode-go",
        name: "OpenCode Go",
        chat: "https://opencode.ai/inference/go/openai/v1",
        responses: "https://opencode.ai/inference/go/openai/v1",
        anthropic: "https://opencode.ai/inference/go/anthropic",
        regions: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_lookups() {
        assert_eq!(
            find("deepseek").unwrap().chat,
            "https://api.deepseek.com/v1"
        );
        // 大小写不敏感；表外名字是 None（走自定义来源路径）。
        assert!(find("Zhipu").is_some());
        assert!(find("my-relay").is_none());
    }

    #[test]
    fn zhipu_coding_region_endpoints() {
        let zhipu = find("zhipu").unwrap();
        let coding = zhipu.regions.iter().find(|r| r.id == "coding").unwrap();
        assert_eq!(coding.chat, "https://open.bigmodel.cn/api/coding/paas/v4");
        assert_eq!(coding.responses, "https://open.bigmodel.cn/api/v1");
    }
}

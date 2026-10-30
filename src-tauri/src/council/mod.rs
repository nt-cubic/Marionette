//! Council Mode — 多模型审议与自治执行(任务书 §3)。
//!
//! 模块地图(架构审计 §1):
//! - `types`      领域类型(spec/消息/状态机枚举/计划)
//! - `state`      状态机转移表(硬编码 + 审计)
//! - `store`      持久化(~/.marionette/council/runs/{id}/)
//! - `worker`     模型客户端(引擎直调 + 测试 mock)
//! - `prompts`    审议协议 prompt(信息隔离强制点)
//! - `orchestrator` run 生命周期与协议循环
//!
//! 参考:oh-my-openagent team-core 的**语义**(幂等键、append-only、
//! bounds 固化、consumed-ledger 恢复),实现按 Marionette 的单进程
//! std-thread 模型从零编写(OmO 为 Sustainable Use License,不拷贝)。

pub mod orchestrator;
pub mod prompts;
pub mod state;
pub mod store;
pub mod types;
pub mod worker;

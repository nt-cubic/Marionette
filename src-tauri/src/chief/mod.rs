//! Marionette Chief（Telegram 远程首席助理）——M0 阶段（docs/research-telegram-chief/）。
//!
//! M0 只做两件事，都不写产品 UI：
//!
//! - [`tee`]：durable event tee。被 Chief 驱动的会话不经前端 rails
//!   （`useSessionRails` 的转录落盘是前端职责），它们的事件流由 tee 原样
//!   追加到 `~/.marionette/chief/sessions/<sessionId>.events.jsonl`，
//!   作为这类会话的"转录真相"，重启后可回放重建视图。挂点在 `acp.rs`
//!   的 `emit_now`（与 telegram 远程桥同一收口）。
//! - [`probe`]：后端驱动探针。从 Rust 直接 start → prompt → wait_turn_idle
//!   一个完整 ACP 回合，验证"主窗隐藏/最小化时无人值守路径仍可靠"，
//!   并证明事件 tee 可回放——这是 M0 的可行性闸门（05-mvp-plan.md §2）。
//!
//! 红线：tee 的 observe 在事件派发线程上执行，**绝不允许阻塞**——慢 IO
//! 全在独立 writer 线程；队列满丢事件计数，不反压 ACP 流。

pub mod broker;
pub mod policy;
pub mod probe;
pub mod runner;
pub mod store;
pub mod tee;
pub mod tools;
pub mod types;

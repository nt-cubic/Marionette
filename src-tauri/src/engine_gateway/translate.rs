//! 协议翻译（合同 C5 的 chat ↔ responses ↔ anthropic 矩阵）。
//!
//! 本文件是子代理任务：请求形态互转（openai chat / openai responses /
//! anthropic messages）+ SSE 事件映射。验收 = 单元测试（#[cfg(test)]）
//! 覆盖：文本轮、带图轮、工具调用轮、系统提示的三方向互转；事件流
//! chat.delta ↔ responses.output_text.delta ↔ anthropic content_block_delta。
//! 接线前 chat.rs 只走 openai→openai 直通。
//! 语义参照：D:\Project\magpie-upstream\internal\gateway\（研究克隆，只读）。

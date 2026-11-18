//! 开发/测试二进制：进程内引擎网关的 exe 形态（`cargo build --bin
//! marionette-engine`）。合同套件（scripts/verify-engine-contract.mts）
//! 拿它当 magpie 替身驱动；正式产品里网关由 engine.rs 在主进程内拉起，
//! **这个 bin 不进发布产物**。
#![recursion_limit = "256"] // groups.rs 的大 json! 测试字面量（与 main.rs 同款）
#[path = "engine_gateway/mod.rs"]
mod engine_gateway;

fn main() {
    let args = std::env::args().skip(1).collect();
    std::process::exit(engine_gateway::cli::main(args));
}

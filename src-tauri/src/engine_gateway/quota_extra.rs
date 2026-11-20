//! 额度补全来源的汇合点（quotas.rs 的 `collect` 只调这一处）：ZCode 订阅 /
//! 钥匙套餐 / 余额补全三路并发读，各自失败安静跳过，互不拖累。
//! 调用方拿的是 provider 快照，**这里不碰 store 锁**。
use serde_json::Value;

use super::store::Provider;

pub fn collect(providers: &[Provider]) -> Vec<Value> {
    let mut zcode: Option<Value> = None;
    let mut plans: Vec<Value> = Vec::new();
    let mut balances: Vec<Value> = Vec::new();
    std::thread::scope(|scope| {
        let z = scope.spawn(super::zcode_account::quota_row);
        let p = scope.spawn(|| super::plan_quotas::rows(providers));
        let b = scope.spawn(|| super::key_balances::rows(providers));
        zcode = z.join().ok().flatten();
        plans = p.join().unwrap_or_default();
        balances = b.join().unwrap_or_default();
    });
    // 行序：套餐、补全余额、ZCode（Codex 订阅行由 quotas.rs 自己排在更后面）
    let mut rows = plans;
    rows.extend(balances);
    rows.extend(zcode);
    rows
}

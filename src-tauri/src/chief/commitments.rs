//! 员工承诺账本（commitments）。
//!
//! 承诺是“答应过但尚未兑现的事”，与 worker task 分离：task 完成不等于
//! commitment 完成。账本独立持久化到 `~/.marionette/chief/commitments.json`，
//! 所有修改走原子快照；到期查询只做本地时间比较，不调用模型。

use super::types::{now_ms, short_id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;

const COMMITMENT_LEDGER_VERSION: u32 = 1;
const MAX_COMMITMENTS: usize = 2_000;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_CONDITION_BYTES: usize = 4 * 1024;
const MAX_SOURCE_REF_BYTES: usize = 8 * 1024;

static LEDGER_LOCK: Mutex<()> = Mutex::new(());
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    if let Ok(mut slot) = ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

fn chief_root() -> Result<PathBuf, String> {
    if let Ok(slot) = ROOT_OVERRIDE.lock() {
        if let Some(path) = slot.as_ref() {
            return Ok(path.clone());
        }
    }
    crate::app_paths::global_dir().map(|dir| dir.join("chief"))
}

pub fn commitments_path() -> Result<PathBuf, String> {
    Ok(chief_root()?.join("commitments.json"))
}

fn warn(message: &str, detail: Option<&str>) {
    crate::debug_log::append("chief-commitments", "warn", "", message, detail);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentKind {
    FollowUp,
    Reminder,
    #[default]
    Promise,
    Condition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    #[default]
    Open,
    Due,
    Done,
    Cancelled,
    Expired,
}

impl CommitmentStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Due)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Expired)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Commitment {
    pub commitment_id: String,
    pub staff_id: String,
    pub text: String,
    pub kind: CommitmentKind,
    /// Unix 毫秒；0 表示没有定时期限，只能等待条件或人工完成。
    pub due_at: u64,
    /// 条件表达式由 Host/调度器解释；模型不能通过这里扩大权限。
    pub condition: Option<String>,
    /// 原始会话/任务/事件来源，例如 `session:.../event:42`。
    pub source_ref: Option<String>,
    pub status: CommitmentStatus,
    pub created_at: u64,
    pub updated_at: u64,
    pub completed_at: Option<u64>,
}

impl Default for Commitment {
    fn default() -> Self {
        let now = now_ms();
        Self {
            commitment_id: short_id("cmt"),
            staff_id: String::new(),
            text: String::new(),
            kind: CommitmentKind::Promise,
            due_at: 0,
            condition: None,
            source_ref: None,
            status: CommitmentStatus::Open,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }
}

impl Commitment {
    pub fn normalized(mut self) -> Self {
        self.commitment_id = self.commitment_id.trim().to_string();
        self.staff_id = self.staff_id.trim().to_string();
        self.text = self.text.trim().to_string();
        self.condition = self
            .condition
            .map(|condition| condition.trim().to_string())
            .filter(|condition| !condition.is_empty());
        self.source_ref = self
            .source_ref
            .map(|source_ref| source_ref.trim().to_string())
            .filter(|source_ref| !source_ref.is_empty());
        if self.updated_at == 0 {
            self.updated_at = now_ms();
        }
        if self.created_at == 0 {
            self.created_at = self.updated_at;
        }
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_id("commitmentId", &self.commitment_id)?;
        validate_id("staffId", &self.staff_id)?;
        if self.text.is_empty() {
            return Err("承诺 text 不能为空".to_string());
        }
        if self.text.len() > MAX_TEXT_BYTES {
            return Err("承诺 text 超过大小上限".to_string());
        }
        if self
            .condition
            .as_ref()
            .is_some_and(|value| value.len() > MAX_CONDITION_BYTES)
        {
            return Err("承诺 condition 超过大小上限".to_string());
        }
        if self
            .source_ref
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SOURCE_REF_BYTES)
        {
            return Err("承诺 sourceRef 超过大小上限".to_string());
        }
        if self.status.is_terminal() && self.completed_at.is_none() {
            // 老账本可能没有 completedAt；读取时允许，新写入时补上。
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitmentLedger {
    pub version: u32,
    pub commitments: Vec<Commitment>,
}

impl Default for CommitmentLedger {
    fn default() -> Self {
        Self {
            version: COMMITMENT_LEDGER_VERSION,
            commitments: Vec::new(),
        }
    }
}

impl CommitmentLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalized(mut self) -> Self {
        self.version = COMMITMENT_LEDGER_VERSION;
        self.commitments = self
            .commitments
            .into_iter()
            .take(MAX_COMMITMENTS)
            .map(Commitment::normalized)
            .collect();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.commitments.len() > MAX_COMMITMENTS {
            return Err(format!("承诺数量超过上限 {MAX_COMMITMENTS}"));
        }
        let mut ids = std::collections::HashSet::new();
        for commitment in &self.commitments {
            commitment.validate()?;
            if !ids.insert(commitment.commitment_id.as_str()) {
                return Err(format!("重复的 commitmentId: {}", commitment.commitment_id));
            }
        }
        Ok(())
    }

    pub fn get(&self, commitment_id: &str) -> Option<&Commitment> {
        self.commitments
            .iter()
            .find(|commitment| commitment.commitment_id == commitment_id)
    }

    pub fn get_mut(&mut self, commitment_id: &str) -> Option<&mut Commitment> {
        self.commitments
            .iter_mut()
            .find(|commitment| commitment.commitment_id == commitment_id)
    }

    pub fn add(&mut self, commitment: Commitment) -> Result<(), String> {
        let commitment = commitment.normalized();
        commitment.validate()?;
        if self.get(&commitment.commitment_id).is_some() {
            return Err(format!("commitmentId 已存在: {}", commitment.commitment_id));
        }
        let mut next = self.clone();
        next.commitments.push(commitment);
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn upsert(&mut self, commitment: Commitment) -> Result<(), String> {
        let commitment = commitment.normalized();
        commitment.validate()?;
        let mut next = self.clone();
        if let Some(index) = next
            .commitments
            .iter()
            .position(|current| current.commitment_id == commitment.commitment_id)
        {
            next.commitments[index] = commitment;
        } else {
            next.commitments.push(commitment);
        }
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn update(&mut self, commitment: Commitment) -> Result<(), String> {
        let commitment = commitment.normalized();
        let Some(index) = self
            .commitments
            .iter()
            .position(|current| current.commitment_id == commitment.commitment_id)
        else {
            return Err(format!("承诺不存在: {}", commitment.commitment_id));
        };
        let mut next = self.clone();
        next.commitments[index] = commitment;
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn remove(&mut self, commitment_id: &str) -> Option<Commitment> {
        let index = self
            .commitments
            .iter()
            .position(|commitment| commitment.commitment_id == commitment_id)?;
        Some(self.commitments.remove(index))
    }

    pub fn load() -> Self {
        load()
    }

    pub fn save(&self) -> Result<(), String> {
        save(self)
    }

    /// 非终态包括 open 和 due；due 仍需要用户/员工明确完成或取消。
    pub fn open(&self) -> Vec<Commitment> {
        let mut out: Vec<_> = self
            .commitments
            .iter()
            .filter(|commitment| commitment.status.is_open())
            .cloned()
            .collect();
        sort_by_due(&mut out);
        out
    }

    pub fn open_for_staff(&self, staff_id: &str) -> Vec<Commitment> {
        self.open()
            .into_iter()
            .filter(|commitment| commitment.staff_id == staff_id)
            .collect()
    }

    /// 到点查询：已标 due 或 open 且 dueAt <= now。
    pub fn due(&self, now: u64) -> Vec<Commitment> {
        let mut out: Vec<_> = self
            .commitments
            .iter()
            .filter(|commitment| commitment_is_due(commitment, now))
            .cloned()
            .collect();
        sort_by_due(&mut out);
        out
    }

    pub fn due_for_staff(&self, staff_id: &str, now: u64) -> Vec<Commitment> {
        self.due(now)
            .into_iter()
            .filter(|commitment| commitment.staff_id == staff_id)
            .collect()
    }

    /// 将到点的 open 承诺原子地标为 due，返回本次新标记的条目。
    pub fn mark_due(&mut self, now: u64) -> Vec<Commitment> {
        let mut changed = Vec::new();
        for commitment in &mut self.commitments {
            if commitment.status == CommitmentStatus::Open
                && commitment.due_at > 0
                && commitment.due_at <= now
            {
                commitment.status = CommitmentStatus::Due;
                commitment.updated_at = now;
                changed.push(commitment.clone());
            }
        }
        changed
    }

    pub fn complete(&mut self, commitment_id: &str, now: u64) -> Result<Commitment, String> {
        self.finish(commitment_id, CommitmentStatus::Done, now)
    }

    pub fn cancel(&mut self, commitment_id: &str, now: u64) -> Result<Commitment, String> {
        self.finish(commitment_id, CommitmentStatus::Cancelled, now)
    }

    pub fn expire(&mut self, commitment_id: &str, now: u64) -> Result<Commitment, String> {
        self.finish(commitment_id, CommitmentStatus::Expired, now)
    }

    /// Dismissing a staff member must not leave open promises that can wake a
    /// nonexistent employee. Terminal rows stay in the ledger for audit.
    pub fn cancel_open_for_staff(&mut self, staff_id: &str, now: u64) -> Vec<Commitment> {
        let ids: Vec<String> = self
            .commitments
            .iter()
            .filter(|commitment| {
                commitment.staff_id == staff_id && commitment.status.is_open()
            })
            .map(|commitment| commitment.commitment_id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.cancel(&id, now).ok())
            .collect()
    }

    fn finish(
        &mut self,
        commitment_id: &str,
        status: CommitmentStatus,
        now: u64,
    ) -> Result<Commitment, String> {
        let Some(commitment) = self.get_mut(commitment_id) else {
            return Err(format!("承诺不存在: {commitment_id}"));
        };
        if commitment.status == status {
            return Ok(commitment.clone());
        }
        if commitment.status.is_terminal() {
            return Err(format!(
                "承诺 {commitment_id} 已结束（{:?}）",
                commitment.status
            ));
        }
        commitment.status = status;
        commitment.updated_at = now;
        commitment.completed_at = Some(now);
        Ok(commitment.clone())
    }
}

fn commitment_is_due(commitment: &Commitment, now: u64) -> bool {
    if !commitment.status.is_open() {
        return false;
    }
    commitment.status == CommitmentStatus::Due
        || (commitment.due_at > 0 && commitment.due_at <= now)
}

fn sort_by_due(commitments: &mut [Commitment]) {
    commitments.sort_by(|left, right| {
        let left_due = if left.due_at == 0 {
            u64::MAX
        } else {
            left.due_at
        };
        let right_due = if right.due_at == 0 {
            u64::MAX
        } else {
            right.due_at
        };
        left_due
            .cmp(&right_due)
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.commitment_id.cmp(&right.commitment_id))
    });
}

fn validate_id(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("非法 {label}: {value:?}"));
    }
    Ok(())
}

fn decode_ledger(raw: &str) -> Result<CommitmentLedger, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let ledger = if value.is_array() {
        let commitments =
            serde_json::from_value::<Vec<Commitment>>(value).map_err(|error| error.to_string())?;
        CommitmentLedger {
            version: COMMITMENT_LEDGER_VERSION,
            commitments,
        }
    } else {
        serde_json::from_value::<CommitmentLedger>(value).map_err(|error| error.to_string())?
    };
    ledger.validate()?;
    Ok(ledger.normalized())
}

fn load_unlocked() -> CommitmentLedger {
    let path = match commitments_path() {
        Ok(path) => path,
        Err(_) => return CommitmentLedger::default(),
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CommitmentLedger::default()
        }
        Err(error) => {
            warn(
                "commitments.json read failed — starting empty",
                Some(&error.to_string()),
            );
            return CommitmentLedger::default();
        }
    };
    match decode_ledger(&raw) {
        Ok(ledger) => ledger,
        Err(error) => {
            warn("commitments.json unreadable — starting empty", Some(&error));
            CommitmentLedger::default()
        }
    }
}

pub fn load() -> CommitmentLedger {
    let _guard = LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_unlocked()
}

fn save_unlocked(ledger: &CommitmentLedger) -> Result<(), String> {
    let ledger = ledger.clone().normalized();
    ledger.validate()?;
    let path = commitments_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("chief commitments: mkdir failed: {error}"))?;
    }
    let body = serde_json::to_string_pretty(&ledger).map_err(|error| error.to_string())?;
    crate::storage::write_atomic(&path, &body)
}

pub fn save(ledger: &CommitmentLedger) -> Result<(), String> {
    let _guard = LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    save_unlocked(ledger)
}

pub fn with_ledger<T>(
    operation: impl FnOnce(&mut CommitmentLedger) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load_unlocked();
    let result = operation(&mut ledger)?;
    save_unlocked(&ledger)?;
    Ok(result)
}

pub fn due(now: u64) -> Vec<Commitment> {
    load().due(now)
}

pub fn due_for_staff(staff_id: &str, now: u64) -> Vec<Commitment> {
    load().due_for_staff(staff_id, now)
}

pub fn open() -> Vec<Commitment> {
    load().open()
}

pub fn open_for_staff(staff_id: &str) -> Vec<Commitment> {
    load().open_for_staff(staff_id)
}

pub fn mark_due(now: u64) -> Result<Vec<Commitment>, String> {
    with_ledger(|ledger| Ok(ledger.mark_due(now)))
}

pub fn complete(commitment_id: &str, now: u64) -> Result<Commitment, String> {
    with_ledger(|ledger| ledger.complete(commitment_id, now))
}

pub fn cancel(commitment_id: &str, now: u64) -> Result<Commitment, String> {
    with_ledger(|ledger| ledger.cancel(commitment_id, now))
}

pub fn expire(commitment_id: &str, now: u64) -> Result<Commitment, String> {
    with_ledger(|ledger| ledger.expire(commitment_id, now))
}

pub fn cancel_open_for_staff(staff_id: &str, now: u64) -> Result<Vec<Commitment>, String> {
    with_ledger(|ledger| Ok(ledger.cancel_open_for_staff(staff_id, now)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chief::store::SANDBOX_LOCK;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    fn sandbox(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("marionette-chief-commitments-{name}-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock() -> MutexGuard<'static, ()> {
        SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn commitment(id: &str, due_at: u64) -> Commitment {
        Commitment {
            commitment_id: id.into(),
            staff_id: "staff_a".into(),
            text: "CI 通过后继续".into(),
            due_at,
            ..Default::default()
        }
    }

    #[test]
    fn ledger_roundtrip_due_and_terminal_actions() {
        let _guard = lock();
        let root = sandbox("actions");
        set_root_override_for_tests(Some(root.clone()));
        let mut ledger = CommitmentLedger::new();
        ledger.add(commitment("cmt_a", 100)).unwrap();
        ledger.add(commitment("cmt_b", 200)).unwrap();
        assert_eq!(ledger.due(150).len(), 1);
        assert_eq!(ledger.open_for_staff("staff_a").len(), 2);
        let marked = ledger.mark_due(150);
        assert_eq!(marked.len(), 1);
        assert_eq!(ledger.get("cmt_a").unwrap().status, CommitmentStatus::Due);
        ledger.complete("cmt_a", 160).unwrap();
        assert_eq!(ledger.get("cmt_a").unwrap().status, CommitmentStatus::Done);
        ledger.cancel("cmt_b", 170).unwrap();
        assert!(ledger.open().is_empty());
        assert!(ledger.complete("cmt_b", 180).is_err());
        save(&ledger).unwrap();
        let loaded = load();
        assert_eq!(loaded.commitments.len(), 2);
        let json = std::fs::read_to_string(commitments_path().unwrap()).unwrap();
        assert!(json.contains("\"dueAt\""));
        assert!(json.contains("\"completedAt\""));
        set_root_override_for_tests(None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dismissing_a_staff_member_cancels_only_that_staffs_open_promises() {
        let mut ledger = CommitmentLedger::new();
        ledger.add(commitment("cmt_a", 100)).unwrap();
        ledger
            .add(Commitment {
                commitment_id: "cmt_b".into(),
                staff_id: "staff_b".into(),
                text: "other staff".into(),
                ..Default::default()
            })
            .unwrap();
        let cancelled = ledger.cancel_open_for_staff("staff_a", 150);
        assert_eq!(cancelled.len(), 1);
        assert_eq!(
            ledger.get("cmt_a").unwrap().status,
            CommitmentStatus::Cancelled
        );
        assert_eq!(ledger.get("cmt_b").unwrap().status, CommitmentStatus::Open);
    }

    #[test]
    fn corrupt_ledger_is_fail_open_empty() {
        let _guard = lock();
        let root = sandbox("corrupt");
        set_root_override_for_tests(Some(root.clone()));
        std::fs::write(commitments_path().unwrap(), "not-json").unwrap();
        assert!(load().commitments.is_empty());
        set_root_override_for_tests(None);
        let _ = std::fs::remove_dir_all(root);
    }
}

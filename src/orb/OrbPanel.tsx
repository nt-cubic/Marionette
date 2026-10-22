/**
 * The expanded 380×540 panel — a single scroll column: header, quick input,
 * scenario grid, running tasks, pending notices, balances, footer actions.
 * Pure presentation: all behavior (commands, capture pipeline, collapse)
 * lives in OrbApp.
 */

import type { RefObject } from "react";
import type { OrbState } from "../lib/orbEvents";
import type { Scenario } from "../lib/scenarios";
import { NOTICE_BADGES } from "./OrbBall";
import { OrbQuota, type OrbQuotaSnapshot } from "./OrbQuota";
import { formatClock, formatElapsed } from "./orbCapture";

export type OrbToast = {
  id: number;
  text: string;
  tone: "ok" | "warn";
};

export type OrbPanelProps = {
  orbState: OrbState;
  scenarios: Scenario[];
  quickText: string;
  onQuickTextChange: (text: string) => void;
  onSendQuickCapture: () => void;
  onScenarioClick: (scenario: Scenario) => void;
  onCollapse: () => void;
  onOpenSession: (sessionId: string) => void;
  onShowMain: () => void;
  onNewChat: () => void;
  onQuit: () => void;
  quota: OrbQuotaSnapshot | null;
  quotaBusy: boolean;
  onQuotaRefresh: () => void;
  dragHover: boolean;
  /** Ticking clock — re-rendered every second while tasks run. */
  nowMs: number;
  toasts: OrbToast[];
  /** Capture items still being saved/fetched — shown as a small chip. */
  processingCount: number;
  quickInputRef: RefObject<HTMLInputElement>;
};

export function OrbPanel(props: OrbPanelProps) {
  const {
    orbState,
    scenarios,
    quickText,
    onQuickTextChange,
    onSendQuickCapture,
    onScenarioClick,
    onCollapse,
    onOpenSession,
    onShowMain,
    onNewChat,
    onQuit,
    quota,
    quotaBusy,
    onQuotaRefresh,
    dragHover,
    nowMs,
    toasts,
    processingCount,
    quickInputRef,
  } = props;

  return (
    <div className="orb-panel">
      <header className="orb-panel__header">
        {/* Title only carries the drag region — buttons must stay clickable. */}
        <h2 className="orb-panel__title" data-tauri-drag-region>
          Marionette
        </h2>
        <button
          type="button"
          className="orb-panel__icon-btn"
          title="收起"
          onClick={onCollapse}
        >
          ✕
        </button>
      </header>

      <div className="orb-panel__scroll custom-scrollbar">
        <section className="orb-panel__section">
          <div className="orb-quick">
            <input
              ref={quickInputRef}
              className="orb-quick__input"
              value={quickText}
              placeholder="记点什么…"
              onChange={(e) => onQuickTextChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  onSendQuickCapture();
                }
              }}
            />
            <button
              type="button"
              className="orb-quick__send"
              disabled={!quickText.trim()}
              onClick={onSendQuickCapture}
              title="送入收集箱"
            >
              收集
            </button>
          </div>
          <p className="orb-panel__hint">
            回车送入收集箱 · 可拖入链接 / 图片 / 文件
            {processingCount > 0 ? (
              <span className="orb-panel__processing">收集中 × {processingCount}</span>
            ) : null}
          </p>
        </section>

        <section className="orb-panel__section">
          <div className="orb-panel__section-head">
            <h3>场景</h3>
          </div>
          <div className="orb-scenario-grid">
            {scenarios.map((s) => (
              <button
                key={s.id}
                type="button"
                className="orb-scenario"
                title={s.name}
                onClick={() => onScenarioClick(s)}
              >
                <span className="orb-scenario__emoji" aria-hidden="true">
                  {s.emoji || "⭐"}
                </span>
                <span className="orb-scenario__name">{s.name}</span>
              </button>
            ))}
          </div>
        </section>

        <section className="orb-panel__section">
          <div className="orb-panel__section-head">
            <h3>运行中任务</h3>
          </div>
          {orbState.running.length === 0 ? (
            <p className="orb-panel__empty">暂无运行中的任务</p>
          ) : (
            <div className="orb-task-list">
              {orbState.running.map((task) => (
                <button
                  key={task.sessionId}
                  type="button"
                  className="orb-task"
                  title="打开会话"
                  onClick={() => onOpenSession(task.sessionId)}
                >
                  <span className="orb-task__spinner" aria-hidden="true" />
                  <span className="orb-task__meta">
                    <strong>{task.label || "未命名会话"}</strong>
                    <span>{task.agentLabel}</span>
                  </span>
                  <span className="orb-task__elapsed">
                    {formatElapsed(task.startedAt, nowMs)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </section>

        {orbState.notices.length > 0 ? (
          <section className="orb-panel__section">
            <div className="orb-panel__section-head">
              <h3>提醒</h3>
            </div>
            <div className="orb-notice-list">
              {orbState.notices.map((notice) => {
                const badge = NOTICE_BADGES[notice.kind];
                return (
                  <button
                    key={`${notice.sessionId}:${notice.at}`}
                    type="button"
                    className={`orb-notice orb-notice--${notice.kind}`}
                    title="打开会话"
                    onClick={() => onOpenSession(notice.sessionId)}
                  >
                    <span className="orb-notice__icon" aria-hidden="true">
                      {badge.icon}
                    </span>
                    <span className="orb-notice__label">{notice.label || "会话"}</span>
                    <span className="orb-notice__time">{formatClock(notice.at)}</span>
                  </button>
                );
              })}
            </div>
          </section>
        ) : null}

        <OrbQuota snapshot={quota} busy={quotaBusy} onRefresh={onQuotaRefresh} />
      </div>

      {toasts.length > 0 ? (
        <div className="orb-toast-strip" aria-live="polite">
          {toasts.map((t) => (
            <span key={t.id} className={`orb-toast-strip__item orb-toast-strip__item--${t.tone}`}>
              {t.text}
            </span>
          ))}
        </div>
      ) : null}

      <footer className="orb-panel__footer">
        <button type="button" className="orb-panel__footer-btn" onClick={onShowMain}>
          打开主窗口
        </button>
        <button type="button" className="orb-panel__footer-btn" onClick={onNewChat}>
          新建对话
        </button>
        <button
          type="button"
          className="orb-panel__footer-btn orb-panel__footer-btn--danger"
          onClick={onQuit}
        >
          退出
        </button>
      </footer>

      {dragHover ? (
        <div className="orb-drop-overlay">
          <span>松手送入收集箱</span>
        </div>
      ) : null}
    </div>
  );
}

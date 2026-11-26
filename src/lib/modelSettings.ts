/**
 * 打开「模型设置」的跨组件信号：Ctrl K 的动作在 App 里，对话框的常驻实例挂在
 * ProjectShelf（贴边态也在）。两边只通过这个事件见面——不要为此把对话框提升到
 * App 再复制一份状态。
 */
export const MODEL_SETTINGS_EVENT = "marionette:open-model-settings";

export function openModelSettings(): void {
  window.dispatchEvent(new CustomEvent(MODEL_SETTINGS_EVENT));
}

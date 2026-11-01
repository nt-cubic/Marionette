/** Computer Use share panel + driver installer (fe-computer batch). */
export const zh = {
  "computer.title": "Computer Use · 共享窗口",
  "computer.hint":
    "共享后 agent 可以看到（只读）或操作（点击/输入）对应窗口。agent 主动请求时会在对话里弹权限卡；这里是你手动发起的入口。",
  "computer.installIntro": "首次使用需要下载操作引擎（约 35MB，来自开源项目 trycua/cua，SHA-256 校验后安装到 ~/.marionette/cua/）。不用这个功能就不会下载。",
  "computer.installButton": "下载并安装引擎",
  "computer.progressMb": "下载中 {done} / {total} MB",
  "computer.progressIndeterminate": "下载中…",
  "computer.installFailed": "安装失败——请检查网络后重试（详情见 dev.log 的 [computer] 条目）",
  "computer.noWindows": "没有可共享的窗口（Marionette 自身、密码管理器等永不共享）",
  "computer.untitledWindow": "（无标题窗口）",
  "computer.minimized": "已最小化",
  "computer.shared.read": "已共享：只读",
  "computer.shared.control": "已共享：可操作",
  "computer.shareRead": "只读",
  "computer.shareControl": "可操作",
  "computer.shareReadHint": "agent 可以截图 / 读界面结构，不能点击输入",
  "computer.shareControlHint": "agent 可以点击 / 输入（按键白名单生效；粘贴仍受限）",
  "computer.refresh": "刷新",
  "computer.stopAll": "全部停止",
  "computer.stopShortcutHint": "全局停止键 Ctrl+Alt+Shift+S 随时撤销全部共享",
  "palette.computerShare": "共享窗口给 agent（Computer Use）",
};

/** P1-13 cheap text model settings dialog. */
export const zh = {
  "cheapText.title": "辅助文本模型",
  "cheapText.lead":
    "用一个小而便宜的模型生成会话标题、分支名和提交摘要。它独立于对话，不计入 agent 的额度统计；没有可用来源时，标题会保留第一条消息。",
  "cheapText.enable": "启用辅助文本生成",
  "cheapText.source": "来源",
  "cheapText.sourcePlaceholder": "选择一个已配置的来源",
  "cheapText.model": "模型",
  "cheapText.modelPlaceholder": "填写模型 ID",
  "cheapText.style": "标题风格",
  "cheapText.stylePlaceholder": "可选，例如：简短、动宾结构",
  "cheapText.maxTokens": "最大输出 token",
  "cheapText.timeout": "超时（毫秒）",
  "cheapText.limitsHint":
    "输出上限 16–512 token；超时 1500–30000 毫秒。范围外会自动收敛。",
  "cheapText.readinessReady": "凭据就绪",
  "cheapText.readinessMissing": "缺少可用密钥",
  "cheapText.readinessOff": "已关闭：只保留首句标题",
  "cheapText.readinessUnset": "还没有选择来源和模型",
  "cheapText.readinessProtocol": "这个来源不能用于辅助文本",
  "cheapText.savedReady": "当前配置可用",
  "cheapText.savedMissing": "当前配置缺少密钥",
  "cheapText.savedUnset": "当前还没有保存配置",
  "cheapText.noSources":
    "还没有可选的来源。先在「模型设置」里添加 API Key 或自定义来源。",
  "cheapText.loading": "正在读取配置…",
  "cheapText.save": "保存",
  "cheapText.saving": "保存中…",
  "cheapText.saveFailed": "保存失败",
  "cheapText.savedStatus":
    "已保存。新配置会用于之后的标题、分支名和提交摘要。",
} as const;

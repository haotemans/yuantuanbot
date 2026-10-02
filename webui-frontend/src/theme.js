// Naive UI 全局设计令牌：现代 SaaS 运维面板观感（亮为主，暗色同步调优）
const base = {
  common: {
    primaryColor: '#2563eb',
    primaryColorHover: '#3b82f6',
    primaryColorPressed: '#1d4ed8',
    primaryColorSuppl: '#2563eb',
    infoColor: '#0ea5e9',
    infoColorHover: '#38bdf8',
    infoColorPressed: '#0284c7',
    infoColorSuppl: '#0ea5e9',
    successColor: '#16a34a',
    warningColor: '#d97706',
    errorColor: '#dc2626',
    borderRadius: '8px',
    borderRadiusSmall: '6px',
    fontFamily:
      "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Helvetica Neue', Arial, sans-serif",
    fontSize: '14px',
  },
  Card: {
    borderRadius: '10px',
    titleFontWeight: '600',
  },
  Menu: {
    borderRadius: '8px',
  },
  Tag: {
    borderRadius: '6px',
  },
}

export const lightOverrides = {
  ...base,
  Card: {
    ...base.Card,
    borderColor: '#e6ecf5',
  },
}

export const darkOverrides = {
  ...base,
  Card: {
    ...base.Card,
    borderColor: 'rgba(255, 255, 255, 0.09)',
  },
}

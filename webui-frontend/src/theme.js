// Naive UI 全局设计令牌 v2：现代 SaaS 运维面板观感（青蓝→靛蓝渐变系，亮为主暗为辅）
const MONO = "'JetBrains Mono', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace"

const base = {
  common: {
    primaryColor: '#4f46e5',
    primaryColorHover: '#6366f1',
    primaryColorPressed: '#4338ca',
    primaryColorSuppl: '#4f46e5',
    infoColor: '#0891b2',
    infoColorHover: '#06b6d4',
    infoColorPressed: '#0e7490',
    infoColorSuppl: '#0891b2',
    successColor: '#16a34a',
    successColorHover: '#22c55e',
    successColorPressed: '#15803d',
    successColorSuppl: '#16a34a',
    warningColor: '#d97706',
    warningColorHover: '#f59e0b',
    warningColorPressed: '#b45309',
    warningColorSuppl: '#d97706',
    errorColor: '#dc2626',
    errorColorHover: '#ef4444',
    errorColorPressed: '#b91c1c',
    errorColorSuppl: '#dc2626',
    borderRadius: '10px',
    borderRadiusSmall: '8px',
    fontFamily:
      "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Helvetica Neue', Arial, sans-serif",
    fontFamilyMono: MONO,
    fontSize: '14px',
  },
  Card: {
    borderRadius: '12px',
    titleFontWeight: '600',
  },
  Menu: {
    borderRadius: '10px',
  },
  Tag: {
    borderRadius: '6px',
  },
  Button: {
    borderRadiusMedium: '10px',
    borderRadiusSmall: '8px',
  },
}

export const lightOverrides = {
  ...base,
  Card: {
    ...base.Card,
    borderColor: '#e6e9f4',
  },
  DataTable: {
    tdColorHover: 'rgba(79, 70, 229, 0.04)',
    thColor: '#f7f8fc',
  },
  Menu: {
    ...base.Menu,
    itemColorActive: 'rgba(79, 70, 229, 0.1)',
    itemColorActiveHover: 'rgba(79, 70, 229, 0.14)',
  },
}

export const darkOverrides = {
  ...base,
  Card: {
    ...base.Card,
    borderColor: 'rgba(255, 255, 255, 0.09)',
  },
  DataTable: {
    tdColorHover: 'rgba(129, 140, 248, 0.1)',
    thColor: 'rgba(255, 255, 255, 0.04)',
  },
  Menu: {
    ...base.Menu,
    itemColorActive: 'rgba(129, 140, 248, 0.16)',
    itemColorActiveHover: 'rgba(129, 140, 248, 0.2)',
  },
}

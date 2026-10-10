// 与 styles.css 共用同一组色阶；普通文字和数字统一中文优先字体。
const FONT = "'Microsoft YaHei UI', 'PingFang SC', 'Noto Sans CJK SC', 'Microsoft YaHei', sans-serif"
const CODE = "Consolas, 'SFMono-Regular', monospace"
const base = {
  common: {
    fontFamily: FONT, fontFamilyMono: CODE, fontSize: '14px',
    borderRadius: '8px', borderRadiusSmall: '6px',
  },
  Card: { borderRadius: '12px', titleFontSizeSmall: '15px', titleFontWeight: '600' },
  Menu: { borderRadius: '8px' },
  Tag: { borderRadius: '5px' },
  Button: { borderRadiusMedium: '8px', borderRadiusSmall: '6px' },
}
function palette(dark) {
  const primary = dark ? '#8bb5ff' : '#285ec8'
  const text = dark ? '#e6edf7' : '#202d43'
  const muted = dark ? '#a4b2c8' : '#5e6d83'
  const surface = dark ? '#202b3e' : '#ffffff'
  const border = dark ? '#3c4b63' : '#dce3ed'
  const soft = dark ? '#263348' : '#f4f7fb'
  const active = dark ? '#304667' : '#eaf1ff'
  const colors = { primary, info: primary, success: dark ? '#68c5a3' : '#237a5a', warning: dark ? '#edbb68' : '#946216', error: dark ? '#f4999f' : '#bb3c49' }
  const common = { ...base.common, bodyColor: dark ? '#182233' : '#f1f4f9', cardColor: surface, modalColor: surface, popoverColor: surface, tableColor: surface, inputColor: surface, textColorBase: text, textColor1: text, textColor2: muted, textColor3: muted, borderColor: border, dividerColor: border }
  for (const [name, color] of Object.entries(colors)) {
    common[`${name}Color`] = color
    common[`${name}ColorHover`] = color
    common[`${name}ColorPressed`] = color
    common[`${name}ColorSuppl`] = color
  }
  return { ...base, common,
    Layout: { color: common.bodyColor, headerColor: surface, siderColor: surface },
    Card: { ...base.Card, borderColor: border, textColor: text, titleTextColor: text },
    DataTable: { tdColorHover: soft, thColor: soft },
    Menu: { ...base.Menu, itemColorActive: active, itemColorActiveHover: active, itemTextColorActive: primary, itemIconColorActive: primary },
  }
}
export const lightOverrides = palette(false)
export const darkOverrides = palette(true)

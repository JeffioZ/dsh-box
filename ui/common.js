// DSHDesktop 共享前端工具（dialog.html / titlebar.js 引用）。
// 各窗口页面无打包器，共享脚本以普通 <script src="common.js"> 引用。

/** HTML 转义（文本插入 innerHTML 前调用）。 */
function dshdEsc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** 币种显示符号（CNY → ¥，其余原样）。 */
function dshdCurrency(c) {
  return c === 'CNY' ? '¥' : c;
}

/** 余额字段兜底格式化（空值显示 0.00）。 */
function dshdBalanceValue(v) {
  return v && v !== '' ? String(v) : '0.00';
}

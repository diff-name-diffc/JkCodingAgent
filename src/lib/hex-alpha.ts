/**
 * 分类颜色是运行期数据（chat_categories.color，建分类缺省 "#297c70"），
 * 无法进 Tailwind 静态类。徽标/卡片的着色 tint 用「hex + alpha 后缀」实现；
 * 非 #rrggbb 形态返回 null，调用方回退到 CSS 类里的中性令牌色。
 */

const HEX_RGB = /^#([0-9a-fA-F]{6})$/;

/** 给 #rrggbb 追加两位 alpha（00–ff）；非法输入返回 null。 */
export function hexWithAlpha(color: string, alphaHex: string): string | null {
  if (!HEX_RGB.test(color)) return null;
  if (!/^[0-9a-fA-F]{2}$/.test(alphaHex)) return null;
  return `${color.toLowerCase()}${alphaHex.toLowerCase()}`;
}

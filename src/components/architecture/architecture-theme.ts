/** 应用侧已把 system 解析为确定的亮/暗，画布只接收确定值（"dark" | "light" 即 Excalidraw THEME 常量值）。 */
export function resolveCanvasTheme(dark: boolean): "dark" | "light" {
  return dark ? "dark" : "light";
}

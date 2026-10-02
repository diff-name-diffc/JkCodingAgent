export default {
  plugins: {
    // Tailwind v4 的 PostCSS 插件自带 import 解析与 vendor 前缀，
    // 不再需要 postcss-import / autoprefixer。
    "@tailwindcss/postcss": {},
  },
};

import { defaultSchema } from "rehype-sanitize";

/**
 * 全应用共享的 rehype-sanitize schema：react-markdown 管线与 Streamdown
 * （聊天气泡）管线共用，保证两条渲染路径的协议白名单一致。
 *
 * 关键点：放行内部 `chat-image://` 协议（聊天图片唯一寻址）、`data:image/`
 * 与 Tauri `asset://`（`http://asset.localhost`）——缺了它 rehype-sanitize
 * 会按默认 src=['http','https'] 把这些 <img src> 静默剥掉。
 */
export const chatSafeSchema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames || []),
    "video",
    "audio",
    "source",
    "details",
    "summary",
  ],
  attributes: {
    ...defaultSchema.attributes,
    // "*" 不放行 style：模型输出的任意内联样式（如 background-image: url(...)）
    // 会在渲染时发起网络/本地协议请求。span/div 单独放行 style 服务于 rehype-raw
    // 保留的原始 HTML 片段；KaTeX 不依赖该白名单——两条管线（react-markdown 与
    // Streamdown 聊天气泡）的 rehype-katex 都排在 rehypeSanitize 之后执行，其
    // 输出除 span/div 外还含 svg 与 MathML 标签，若先过本 schema 会被剥掉。
    "*": [...(defaultSchema.attributes?.["*"] || []), "className"],
    video: ["src", "controls", "width", "height", "muted", "autoplay", "loop"],
    audio: ["src", "controls"],
    source: ["src", "type"],
    details: ["open"],
    img: [
      "src",
      "alt",
      "width",
      "height",
      "loading",
      ...(defaultSchema.attributes?.img || []),
    ],
    a: ["href", "target", "rel", ...(defaultSchema.attributes?.a || [])],
    code: ["className"],
    span: ["className", "style", ...(defaultSchema.attributes?.span || [])],
    div: ["className", "style"],
    td: ["align", "className"],
    th: ["align", "className"],
  },
  protocols: {
    ...(defaultSchema.protocols || {}),
    // Allow the internal chat-image:// protocol, local data URIs, and Tauri
    // asset:// URLs on <img src> and <a href>. Without this, rehype-sanitize
    // strips the src attribute for any non-http(s) image reference.
    src: [
      ...((defaultSchema.protocols && defaultSchema.protocols.src) || ["http", "https"]),
      "chat-image",
      "data",
      "asset",
    ],
    href: [
      ...((defaultSchema.protocols && defaultSchema.protocols.href) || ["http", "https"]),
      "asset",
    ],
  },
};

const SAFE_PROTOCOL = /^(?:https?:|irc?:|ircs?:|mailto:|xmpp:)$/i;

/**
 * markdown URL 白名单变换：内部协议原样放行，其余沿用 react-markdown
 * `defaultUrlTransform` 的安全默认（协议在白名单或无协议的相对/片段地址
 * 保留，其他清空）。不复用 react-markdown 的导出以避免把 react-markdown
 * 拖进 Streamdown（聊天气泡）的 chunk。
 */
export function chatUrlTransform(url: string) {
  if (
    url.startsWith("data:image/") ||
    url.startsWith("asset://") ||
    url.startsWith("http://asset.localhost/") ||
    url.startsWith("chat-image://") ||
    url.startsWith("/")
  ) {
    return url;
  }
  const colon = url.indexOf(":");
  const questionMark = url.indexOf("?");
  const numberSign = url.indexOf("#");
  if (
    colon < 0 ||
    (questionMark > -1 && colon > questionMark) ||
    (numberSign > -1 && colon > numberSign) ||
    SAFE_PROTOCOL.test(url.slice(0, colon + 1))
  ) {
    return url;
  }
  return "";
}

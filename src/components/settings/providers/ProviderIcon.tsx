import { Server } from "lucide-react";
import chatgptLogo from "../../../assets/chatgpt.svg";
import claudeLogo from "../../../assets/claude.svg";

function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

/** Provider 品牌图标：已知域名用内置 logo，其余用名称首字母头像。 */
export function ProviderIcon({ url, name, size = 20 }: { url: string; name: string; size?: number }) {
  const host = hostnameOf(url);
  if (host.includes("openai.com")) {
    return <img src={chatgptLogo} alt="" width={size} height={size} className="ai-set-provider-logo" />;
  }
  if (host.includes("anthropic.com")) {
    return <img src={claudeLogo} alt="" width={size} height={size} className="ai-set-provider-logo" />;
  }
  const letter = (name.trim() || host.replace(/^www\./, "") || "?").trim().charAt(0).toUpperCase();
  if (letter === "?" && !name.trim() && !host) {
    return <Server size={size} strokeWidth={1.5} className="ai-set-provider-fallback" />;
  }
  return (
    <span className="ai-set-provider-letter" style={{ width: size + 8, height: size + 8, fontSize: size * 0.6 }}>
      {letter}
    </span>
  );
}

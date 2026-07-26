import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

/**
 * API Key 输入框：右侧内嵌眼睛图标切换明文/密文，作用域仅限本字段。
 * 替代旧的全局「显示 Key」开关。
 */
export function ApiKeyInput({
  value,
  onChange,
  onBlur,
  placeholder = "sk-...",
  disabled,
}: {
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="ai-set-key-wrap">
      <input
        className="ai-settings-input ai-set-key-input"
        type={visible ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        disabled={disabled}
      />
      <button
        type="button"
        className="ai-set-key-toggle"
        onClick={() => setVisible((v) => !v)}
        aria-label={visible ? "隐藏 API Key" : "显示 API Key"}
        title={visible ? "隐藏 API Key" : "显示 API Key"}
        tabIndex={-1}
      >
        {visible ? <EyeOff size={16} strokeWidth={1.5} /> : <Eye size={16} strokeWidth={1.5} />}
      </button>
    </div>
  );
}

import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import s from "../../../styles";

interface PasswordInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
}

/**
 * 带 Eye/EyeOff 显隐切换的 API Key 密码框。
 * 复用于 Qdrant apiKey 与 Embedding apiKey 字段，样式沿用 aha* token。
 */
export function PasswordInput({
  value,
  onChange,
  placeholder,
  disabled,
}: PasswordInputProps) {
  const [show, setShow] = useState(false);
  return (
    <div style={s.ragPasswordWrap}>
      <input
        style={s.ragPasswordInput}
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        disabled={disabled}
      />
      <button
        type="button"
        style={s.ragPasswordToggle}
        onClick={() => setShow((prev) => !prev)}
        title={show ? "隐藏" : "显示"}
        tabIndex={-1}
      >
        {show ? <EyeOff size={14} /> : <Eye size={14} />}
      </button>
    </div>
  );
}

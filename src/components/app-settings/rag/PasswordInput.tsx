import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

interface PasswordInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
}

/**
 * 带 Eye/EyeOff 显隐切换的 API Key 密码框。
 */
export function PasswordInput({
  value,
  onChange,
  placeholder,
  disabled,
}: PasswordInputProps) {
  const [show, setShow] = useState(false);
  return (
    <div className="ai-rag-password">
      <input
        className="ai-rag-password-input"
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
        className="ai-rag-password-toggle"
        onClick={() => setShow((prev) => !prev)}
        title={show ? "隐藏" : "显示"}
        tabIndex={-1}
      >
        {show ? <EyeOff size={14} /> : <Eye size={14} />}
      </button>
    </div>
  );
}

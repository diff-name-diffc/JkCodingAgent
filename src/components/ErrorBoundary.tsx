import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** 用于在错误信息中标识面板，例如 "文件浏览器" */
  label?: string;
  /** 捕获到错误时的自定义回退 UI；不传则使用内置样式 */
  fallback?: (error: Error, reset: () => void) => ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      `[ErrorBoundary${this.props.label ? ` – ${this.props.label}` : ""}]`,
      error,
      info.componentStack,
    );
  }

  reset = () => {
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    if (this.props.fallback) {
      return this.props.fallback(error, this.reset);
    }

    const label = this.props.label ?? "该面板";

    return (
      <div className="ai-error-boundary">
        <div className="ai-error-boundary-icon">⚠</div>
        <div className="ai-error-boundary-title">{label}渲染出错</div>
        <div className="ai-error-boundary-message">{error.message || "未知错误"}</div>
        <button onClick={this.reset} className="ai-error-boundary-btn">
          重试
        </button>
      </div>
    );
  }
}

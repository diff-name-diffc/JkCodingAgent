import appLogo from "../../assets/app-logo.png";

/** 聊天与分类入口共用的欢迎层级；保持说明简短，把注意力留给下一步操作。 */
export function ChatWelcomeHeading({ title, copy }: { title: string; copy: string }) {
  return (
    <div className="ai-chat-welcome-heading">
      <img src={appLogo} alt="" className="ai-chat-welcome-logo" />
      <span className="ai-chat-welcome-eyebrow">JKCODINGAGENT</span>
      <h2 className="ai-chat-welcome-title">{title}</h2>
      <p className="ai-chat-welcome-copy">{copy}</p>
    </div>
  );
}

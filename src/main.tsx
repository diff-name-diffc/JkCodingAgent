import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "./styles/tailwind.css";
import App from "./App";
import { ToastProvider } from "./components/Toast";
import { QueryProvider } from "./components/providers/query-provider";
import { TooltipProvider } from "./components/ui/tooltip";
import { ErrorBoundary } from "./components/ErrorBoundary";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary label="页面">
      <QueryProvider>
        <ToastProvider>
          <TooltipProvider>
            <App />
          </TooltipProvider>
        </ToastProvider>
      </QueryProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);

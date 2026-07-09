/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["class", "html.dark"],
  content: [
    "./src/**/*.{ts,tsx}",
    "./index.html",
  ],
  // Preflight is disabled: the app ships its own baseline reset + tokens via
  // App.css. Enabling preflight would reset margins, buttons and headings
  // globally and clash with the existing UI.
  corePlugins: { preflight: false },
  theme: {
    // Map shadcn-style tokens onto the app's existing CSS variables in
    // App.css so new Tailwind components inherit the established palette
    // and automatically follow light/dark mode.
    extend: {
      colors: {
        background: "var(--bg-card)",
        foreground: "var(--text-primary)",
        muted: {
          DEFAULT: "var(--bg-subtle)",
          foreground: "var(--text-muted)",
        },
        card: {
          DEFAULT: "var(--bg-card)",
          foreground: "var(--text-primary)",
        },
        popover: {
          DEFAULT: "var(--bg-card)",
          foreground: "var(--text-primary)",
        },
        border: "var(--border-medium)",
        input: "var(--border-medium)",
        ring: "var(--accent)",
        primary: {
          DEFAULT: "var(--accent)",
          foreground: "#ffffff",
          hover: "var(--accent-hover)",
        },
        secondary: {
          DEFAULT: "var(--bg-sidebar-elevated)",
          foreground: "var(--text-secondary)",
        },
        destructive: {
          DEFAULT: "var(--danger)",
          foreground: "#ffffff",
        },
        accent: {
          DEFAULT: "var(--bg-selected)",
          foreground: "var(--text-primary)",
        },
        sidebar: {
          DEFAULT: "var(--bg-sidebar)",
          foreground: "var(--text-primary)",
          border: "var(--border-dim)",
          accent: "var(--bg-selected)",
          "accent-foreground": "var(--text-primary)",
          hover: "var(--bg-hover)",
        },
        success: "var(--success)",
        warning: "var(--warning)",
        danger: "var(--danger)",
      },
      borderRadius: {
        sm: "var(--radius-sm)",
        md: "var(--radius-md)",
        lg: "var(--radius-lg)",
        xl: "calc(var(--radius-lg) + 4px)",
      },
      fontFamily: {
        sans: "var(--font-ui)",
        mono: "var(--font-mono)",
      },
      boxShadow: {
        soft: "var(--shadow-sm)",
        medium: "var(--shadow-md)",
        chat: "var(--chat-shadow)",
        "chat-soft": "var(--chat-shadow-soft)",
      },
      keyframes: {
        "fade-in": {
          from: { opacity: "0" },
          to: { opacity: "1" },
        },
        "slide-up": {
          from: { opacity: "0", transform: "translateY(6px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
        shimmer: {
          "100%": { transform: "translateX(100%)" },
        },
      },
      animation: {
        "fade-in": "fade-in var(--motion-normal) var(--motion-ease)",
        "slide-up": "slide-up var(--motion-normal) var(--motion-ease)",
      },
      transitionDuration: {
        fast: "var(--motion-fast)",
        normal: "var(--motion-normal)",
        slow: "var(--motion-slow)",
      },
      transitionTimingFunction: {
        motion: "var(--motion-ease)",
      },
    },
  },
  plugins: [],
};

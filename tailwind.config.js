/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./src/**/*.{ts,tsx}",
    "./index.html",
    // Streamdown ships pre-classed components; scan its dist so the utility
    // classes it uses (bg-sidebar, after:content-[var(--streamdown-caret)], …)
    // are generated. This is the Tailwind v3 equivalent of the v4 `@source`
    // directive recommended by streamdown's docs.
    "./node_modules/streamdown/dist/*.js",
    "./node_modules/@streamdown/*/dist/*.js",
  ],
  // Preflight is disabled: the app ships its own baseline reset + tokens via
  // App.css. Enabling preflight would reset margins, buttons and headings
  // globally and clash with the existing UI.
  corePlugins: { preflight: false },
  theme: {
    // Map shadcn-style tokens onto the app's existing CSS variables in
    // App.css so new Tailwind components inherit the established palette.
    extend: {
      // NOTE: colors are expressed as `rgb(var(--x-rgb) / <alpha-value>)` so
      // that opacity modifiers (e.g. bg-card/70, bg-success/15) actually get
      // generated — Tailwind v3 cannot apply /NN alpha to a plain var() color
      // and silently skips those utilities. The --x-rgb triplets live in
      // App.css next to their hex counterparts.
      colors: {
        background: "rgb(var(--bg-card-rgb) / <alpha-value>)",
        foreground: "rgb(var(--text-primary-rgb) / <alpha-value>)",
        muted: {
          DEFAULT: "rgb(var(--bg-subtle-rgb) / <alpha-value>)",
          foreground: "rgb(var(--text-muted-rgb) / <alpha-value>)",
        },
        card: {
          DEFAULT: "rgb(var(--bg-card-rgb) / <alpha-value>)",
          foreground: "rgb(var(--text-primary-rgb) / <alpha-value>)",
        },
        popover: {
          DEFAULT: "rgb(var(--bg-card-rgb) / <alpha-value>)",
          foreground: "rgb(var(--text-primary-rgb) / <alpha-value>)",
        },
        // --border-medium is rgba(30,39,36,0.17); keep the 0.17 factor so
        // plain border-border renders exactly as before, while border-border/60
        // scales from it (0.6 * 0.17).
        border: "rgb(var(--border-medium-rgb) / calc(<alpha-value> * 0.17))",
        input: "rgb(var(--bg-input-rgb) / <alpha-value>)",
        ring: "rgb(var(--accent-rgb) / <alpha-value>)",
        primary: {
          DEFAULT: "rgb(var(--accent-rgb) / <alpha-value>)",
          foreground: "var(--accent-foreground)",
          hover: "rgb(var(--accent-hover-rgb) / <alpha-value>)",
        },
        secondary: {
          DEFAULT: "rgb(var(--bg-sidebar-elevated-rgb) / <alpha-value>)",
          foreground: "rgb(var(--text-secondary-rgb) / <alpha-value>)",
        },
        destructive: {
          DEFAULT: "rgb(var(--danger-rgb) / <alpha-value>)",
          foreground: "var(--destructive-foreground)",
        },
        accent: {
          DEFAULT: "rgb(var(--accent-rgb) / <alpha-value>)",
          foreground: "var(--accent-foreground)",
          soft: "var(--accent-soft)",
        },
        sidebar: {
          DEFAULT: "rgb(var(--bg-sidebar-rgb) / <alpha-value>)",
          foreground: "rgb(var(--text-primary-rgb) / <alpha-value>)",
          border: "var(--border-dim)",
          accent: "rgb(var(--bg-selected-rgb) / <alpha-value>)",
          "accent-foreground": "rgb(var(--text-primary-rgb) / <alpha-value>)",
          hover: "rgb(var(--bg-hover-rgb) / <alpha-value>)",
        },
        success: "rgb(var(--success-rgb) / <alpha-value>)",
        warning: "rgb(var(--warning-rgb) / <alpha-value>)",
        danger: "rgb(var(--danger-rgb) / <alpha-value>)",
        info: "rgb(var(--info-rgb) / <alpha-value>)",
      },
      borderRadius: {
        sm: "var(--radius-sm)",
        md: "var(--radius-md)",
        lg: "var(--radius-lg)",
        xl: "var(--radius-xl)",
        full: "var(--radius-full)",
      },
      fontFamily: {
        sans: "var(--font-ui)",
        mono: "var(--font-mono)",
      },
      boxShadow: {
        soft: "var(--shadow-sm)",
        medium: "var(--shadow-md)",
        large: "var(--shadow-lg)",
        chat: "var(--chat-shadow)",
        "chat-soft": "var(--chat-shadow-soft)",
        focus: "var(--shadow-focus)",
        "accent-glow": "var(--shadow-accent-glow)",
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

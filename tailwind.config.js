/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Medal-like layered blacks (the shell lives here).
        base: "#000000",
        surface: "#161617",
        raised: "#1f1f20",
        line: "rgba(255, 255, 255, 0.08)",
        "line-strong": "rgba(255, 255, 255, 0.16)",
        // Text scale (measured: 21:1 / 9.9:1 / 5.3:1 on surface; faint is decorative only).
        ink: {
          DEFAULT: "#ffffff",
          soft: "#b3b1b6",
          muted: "#8b8b90",
          faint: "#6e6e73",
        },
        // Logo identity: red = actions, blue = links/focus (Aero sky).
        brand: { DEFAULT: "#ef4444", bright: "#f87171" },
        link: { DEFAULT: "#3b82f6", bright: "#60a5fa" },
        ok: { DEFAULT: "#3fb950", bright: "#56d364" },
        warn: { DEFAULT: "#d29922", bright: "#e8b64a" },
        aqua: { DEFAULT: "#39c5cf", bright: "#5ad4dd" },
        edit: { DEFAULT: "#a371f7", bright: "#c297ff" },
        // v1 aliases: kept so untouched panels stay readable until step 2.
        void: "#000000",
        panel: "#161617",
        blood: { DEFAULT: "#ef4444", bright: "#f87171" },
        gold: { DEFAULT: "#d29922", bright: "#e8b64a" },
        jade: { DEFAULT: "#3fb950", bright: "#56d364" },
        aether: { DEFAULT: "#a371f7", bright: "#c297ff" },
        lava: { DEFAULT: "#f0883e", bright: "#ffa657" },
        sky: { DEFAULT: "#3b82f6", bright: "#60a5fa" },
        paper: { DEFAULT: "#1f1f20", dark: "#26262a", soft: "#2c2c30" },
        moonclip: {
          void: "#000000",
          panel: "#161617",
          card: "#1f1f20",
          lunar: "#3b82f6",
          astral: "#a371f7",
          starlight: "#ffffff",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "Consolas", "monospace"],
      },
      boxShadow: {
        panel: "0 12px 32px rgba(0, 0, 0, 0.5)",
        pop: "0 8px 24px rgba(0, 0, 0, 0.55)",
        "ring-brand": "0 0 0 1px rgba(239, 68, 68, 0.6)",
        "ring-link": "0 0 0 1px rgba(59, 130, 246, 0.7)",
      },
      borderRadius: {
        card: "10px",
        control: "8px",
      },
    },
  },
  plugins: [],
}

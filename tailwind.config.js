/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Fanzine-dark surfaces (warm, paper-adjacent blacks).
        void: "#15110d",
        panel: "#1c1814",
        raised: "#221d18",
        line: "#2a221b",
        // Text scale (aged paper ink).
        ink: {
          DEFAULT: "#efe6d2",
          soft: "#c9bda6",
          muted: "#a8967c",
          faint: "#6a5d4b",
          dark: "#1a1410",
        },
        // Literal paper for stamps/badges/empty states.
        paper: {
          DEFAULT: "#efe6d2",
          dark: "#d8cdb5",
          soft: "#f7f1e1",
        },
        // Accents (logo red -> logo blue, plus jade/gold/lava/aether).
        blood: { DEFAULT: "#c92a2a", bright: "#e04040" },
        gold: { DEFAULT: "#d8a44a", bright: "#e8bc63" },
        jade: { DEFAULT: "#2f7a5e", bright: "#3c9a76" },
        aether: { DEFAULT: "#6e58a6", bright: "#8a72c4" },
        lava: { DEFAULT: "#e87329", bright: "#ff8a3d" },
        sky: { DEFAULT: "#3b82f6", bright: "#60a5fa" },
        // Transition aliases: removed once every screen is migrated.
        moonclip: {
          void: "#15110d",
          panel: "#1c1814",
          card: "#221d18",
          lunar: "#3b82f6",
          astral: "#6e58a6",
          starlight: "#efe6d2",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        display: ["Fraunces", "Georgia", "Times New Roman", "serif"],
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "Consolas", "monospace"],
      },
      boxShadow: {
        stamp: "2px 2px 0 rgba(26, 20, 16, 0.9)",
        "stamp-blood": "2px 2px 0 rgba(201, 42, 42, 0.85)",
        panel: "0 10px 30px rgba(0, 0, 0, 0.45)",
        "glow-blood": "0 0 18px rgba(201, 42, 42, 0.25)",
        "glow-sky": "0 0 18px rgba(59, 130, 246, 0.28)",
      },
      borderRadius: {
        card: "10px",
        stamp: "4px",
      },
    },
  },
  plugins: [],
}

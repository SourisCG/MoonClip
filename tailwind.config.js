/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        moonclip: {
          void: "#050608",
          panel: "#0b0f19",
          card: "#0d1220",
          lunar: "#38bdf8",
          astral: "#818cf8",
          starlight: "#e0e7ff",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
      },
    },
  },
  plugins: [],
}


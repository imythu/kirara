import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "#f6f3eb",
        foreground: "#292822",
        card: "#fffdf8",
        border: "#ded9cc",
        input: "#fffdf8",
        primary: "#b44332",
        "primary-foreground": "#ffffff",
        secondary: "#efe8db",
        "secondary-foreground": "#5b4638",
        muted: "#746d60",
        accent: "#efeadd",
        destructive: "#b63232",
        ring: "#b44332",
        surface: "#fffdf8",
        "surface-container": "#f0ece2",
        blossom: "#c8754f",
        night: "#252620",
        jade: "#477564"
      },
      boxShadow: {
        card: "none",
        glow: "none",
      },
      borderRadius: {
        xl: "0.75rem",
        "3xl": "1rem",
      },
    },
  },
  plugins: [],
} satisfies Config;

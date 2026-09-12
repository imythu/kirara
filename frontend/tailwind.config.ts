import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "#f5f3fa",
        foreground: "#292438",
        card: "#fefcff",
        border: "#ddd6e9",
        input: "#fefcff",
        primary: "#7450a3",
        "primary-foreground": "#ffffff",
        secondary: "#ece5f5",
        "secondary-foreground": "#544067",
        muted: "#746780",
        accent: "#eee8f6",
        destructive: "#b63232",
        ring: "#7450a3",
        surface: "#fefcff",
        "surface-container": "#ede8f3",
        blossom: "#ab6386",
        night: "#292135",
        jade: "#397968"
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

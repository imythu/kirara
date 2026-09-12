import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

function getPackageName(id: string) {
  const normalized = id.replaceAll("\\", "/");
  const nodeModulesIndex = normalized.lastIndexOf("/node_modules/");
  if (nodeModulesIndex === -1) {
    return null;
  }

  const packagePath = normalized.slice(nodeModulesIndex + "/node_modules/".length);
  if (packagePath.startsWith("@")) {
    const [scope, name] = packagePath.split("/");
    return scope && name ? `${scope}/${name}` : null;
  }

  const [name] = packagePath.split("/");
  return name ?? null;
}

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    host: "0.0.0.0",
    allowedHosts: true,
    cors: true,
    port: 31234,
    strictPort: true,
    proxy: {
      "/dav/ptd": {
        target: "http://127.0.0.1:3000",
        changeOrigin: true,
      },
      "/api": {
        target: "http://127.0.0.1:3000",
        changeOrigin: true,
      },
    },
  },
  preview: {
    host: "0.0.0.0",
    port: 31234,
    strictPort: true,
    allowedHosts: true,
    cors: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          const packageName = getPackageName(id);
          if (!packageName) {
            return;
          }

          if (packageName === "recharts" || packageName.startsWith("d3-") || packageName === "victory-vendor") {
            return "vendor-recharts";
          }
          if (packageName === "lucide-react") {
            return "vendor-icons";
          }
          if (packageName === "react" || packageName === "react-dom" || packageName === "scheduler") {
            return "vendor-react";
          }
          if (packageName === "@radix-ui/react-slot" || packageName === "class-variance-authority" || packageName === "clsx" || packageName === "tailwind-merge") {
            return "vendor-ui";
          }
          return "vendor";
        },
      },
    },
  },
});

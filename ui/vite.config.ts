import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

// Casual Note WebView build. Tauri serves this bundle with a strict CSP and no
// network access (see crates/tauri-app/tauri.conf.json). The dev server port is
// mirrored by `devUrl` in the Tauri config.
const DEV_PORT = Number(process.env.VITE_DEV_PORT ?? 5173);

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  // Tauri expects a fixed port and fails if it is already taken.
  clearScreen: false,
  server: {
    port: DEV_PORT,
    strictPort: true,
  },
  build: {
    // Match the WebView engine floor across platforms.
    target: "es2022",
    outDir: "dist",
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});

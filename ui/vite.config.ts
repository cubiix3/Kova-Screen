import { defineConfig } from "vite";

// Kova Screen ships two small windows, not a web app. The config stays minimal:
// no framework, no router, no code splitting worth the name.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    // WebView2 is evergreen Chromium, so there is no reason to down-level.
    target: "chrome110",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
  },
});

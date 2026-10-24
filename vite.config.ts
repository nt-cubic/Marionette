import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    watch: {
      // Rust compilation creates and locks executables in this directory on Windows;
      // fortest/ holds isolated-instance WebView2 profiles whose locked files kill the watcher (EBUSY).
      ignored: ["**/src-tauri/target/**", "**/fortest/**"]
    }
  },
  envPrefix: ["VITE_", "TAURI_"]
});

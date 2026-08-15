import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 15620,
    strictPort: true,
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/target/**", "**/dist/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "safari13", minify: "esbuild", sourcemap: false },
});

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 1420,
    proxy: {
      // In dev, forward WebSocket and API calls to the Rust server
      "/ws":  { target: "ws://localhost:3000",  ws: true, changeOrigin: true },
      "/api": { target: "http://localhost:3000", changeOrigin: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});

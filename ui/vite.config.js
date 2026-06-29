import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The Rust server serves the built assets from ui/dist and proxies the API in
// dev. During `npm run dev`, forward /api and /ws to the running server.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://localhost:8080",
      "/ws": { target: "ws://localhost:8080", ws: true },
    },
  },
  build: { outDir: "dist" },
});

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  build: {
    // Single-page internal dashboard served from the same host as the API;
    // a single ~800 kB bundle is fine and code splitting buys nothing here.
    chunkSizeWarningLimit: 1024,
  },
  server: {
    host: "0.0.0.0",
    port: 8003,
  },
  test: {
    environment: "jsdom",
    restoreMocks: true,
    setupFiles: ["./src/setupTests.ts"],
  },
});

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The Rust binary embeds `web/dist` verbatim and serves it from `/`, so the
// build must stay relative-path free and land in the default `dist`.
export default defineConfig({
  plugins: [react()],
  server: {
    // `vite dev` talks to a `gribovik --assets` session on the usual port.
    proxy: {
      "/api": "http://127.0.0.1:7777",
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});

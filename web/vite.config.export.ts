import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

export default defineConfig({
  plugins: [react(), viteSingleFile()],
  define: {
    __GRIBOVIK_EXPORT__: "true",
  },
  build: {
    emptyOutDir: false,
    rollupOptions: {
      input: "export.html",
    },
  },
});

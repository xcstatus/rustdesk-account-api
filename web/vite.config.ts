import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  base: "/admin/",
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    proxy: {
      "/admin/api": { target: "http://127.0.0.1:21114" },
      "/api/oidc": { target: "http://127.0.0.1:21114" },
    },
  },
});

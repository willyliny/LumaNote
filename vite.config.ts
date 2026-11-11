import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Each screen is its own HTML entry so it can become a separate Tauri window.
// Paths are relative to the project root (where vite is run).
export default defineConfig({
  plugins: [react()],
  base: './',
  // Tauri dev needs a fixed address: src-tauri/tauri.conf.json build.devUrl points here.
  // 127.0.0.1 (not "localhost") avoids IPv4/IPv6 mismatches on Windows.
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**', '**/target/**', '**/crates/**'] },
  },
  // keep `vite preview` (used by tests-ui screenshots) on its previous host
  preview: { host: 'localhost' },
  build: {
    rollupOptions: {
      input: {
        index: 'index.html', // dev launcher only
        floating: 'floating.html',
        panel: 'panel.html',
        settings: 'settings.html',
      },
    },
  },
});

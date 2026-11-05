import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { coreBridge } from './server/bridge.ts';

export default defineConfig({
  plugins: [react(), coreBridge()],
  server: { host: '127.0.0.1', port: 5173, strictPort: true },
  preview: { host: '127.0.0.1', port: 5174, strictPort: true },
  // 実 Rust プロセスと複数段階の UI 操作を含むため、低速な Windows 環境にも余裕を持たせる。
  test: { environment: 'node', include: ['tests/**/*.test.{ts,tsx}'], testTimeout: 15_000 },
});

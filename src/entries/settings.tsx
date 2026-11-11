import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { boot } from './boot';
import { SettingsApp } from '../features/settings/SettingsApp';

boot();
document.title = '精靈助手 設定與記憶';
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <SettingsApp />
  </StrictMode>,
);

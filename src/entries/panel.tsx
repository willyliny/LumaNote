import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { boot } from './boot';
import { PanelApp } from '../features/PanelApp';

boot();
document.title = '精靈助手';
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <PanelApp />
  </StrictMode>,
);

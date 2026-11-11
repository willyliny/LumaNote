import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { boot } from './boot';
import { FloatingApp } from '../features/floating/FloatingApp';

boot();
document.title = '精靈助手 浮動入口';
if (new URLSearchParams(location.search).get('backdrop') === '1') {
  document.documentElement.setAttribute('data-backdrop', '1');
}
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <FloatingApp />
  </StrictMode>,
);

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { boot } from './boot';
import { FloatingApp } from '../features/floating/FloatingApp';

boot();
document.title = '精靈助手 浮動入口';
// dev/screenshot only: paint a stand-in wallpaper ('1' follows the theme, or 'light' | 'dark')
const backdrop = new URLSearchParams(location.search).get('backdrop');
if (backdrop === '1' || backdrop === 'light' || backdrop === 'dark') {
  document.documentElement.setAttribute('data-backdrop', backdrop);
}
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <FloatingApp />
  </StrictMode>,
);

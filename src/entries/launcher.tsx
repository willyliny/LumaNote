import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '../styles/tokens.css';
import '../styles/base.css';
import './launcher.css';
import { initTheme } from '../lib/theme';
import { t } from '../lib/strings';
import { ipc } from '../lib/ipc';

/** Dev-only index: links to each window entry. Not part of the product. */
initTheme();
const LINKS = [
  { href: './floating.html?backdrop=1', label: t.dev.floating },
  { href: './panel.html', label: t.dev.panel },
  { href: './settings.html', label: t.dev.settings },
];
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <main className="launcher">
      <h1>{t.dev.launcherTitle}</h1>
      <p>{t.dev.launcherBody}</p>
      <ul>
        {LINKS.map((l) => (
          <li key={l.href}>
            <a className="btn" href={l.href} target="_blank" rel="noopener">
              {l.label}
            </a>
          </li>
        ))}
      </ul>
      {ipc.kind === 'mock' && <p className="subtle">{t.dev.mockNote}</p>}
    </main>
  </StrictMode>,
);

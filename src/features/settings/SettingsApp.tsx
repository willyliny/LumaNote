import { useRef, useState, type KeyboardEvent } from 'react';
import { t } from '../../lib/strings';
import { Icon, type IconName } from '../../components/Icon';
import { Toast } from '../../components/Toast';
import { ModelsSection } from './ModelsSection';
import { ObservationSection } from './ObservationSection';
import { MemorySection } from './MemorySection';
import { AppearanceSection } from './AppearanceSection';
import './settings.css';

type Section = 'models' | 'observation' | 'memory' | 'appearance';
const NAV: { id: Section; icon: IconName }[] = [
  { id: 'models', icon: 'link' },
  { id: 'observation', icon: 'shield' },
  { id: 'memory', icon: 'leaf' },
  { id: 'appearance', icon: 'moon' },
];

function initial(): Section {
  const q = new URLSearchParams(location.search).get('tab');
  return NAV.some((n) => n.id === q) ? (q as Section) : 'models';
}

export function SettingsApp() {
  const [section, setSection] = useState<Section>(initial);
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});

  const onKey = (e: KeyboardEvent) => {
    const i = NAV.findIndex((x) => x.id === section);
    let n = i;
    if (e.key === 'ArrowDown' || e.key === 'ArrowRight') n = (i + 1) % NAV.length;
    else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') n = (i + NAV.length - 1) % NAV.length;
    else return;
    e.preventDefault();
    const next = NAV[n]!.id;
    setSection(next);
    refs.current[next]?.focus();
  };

  return (
    <div className="settings">
      <nav className="settings__nav" aria-label={t.settings.title}>
        <h1>{t.settings.title}</h1>
        <div role="tablist" aria-orientation="vertical" onKeyDown={onKey} className="settings__tabs">
          {NAV.map(({ id, icon }) => (
            <button
              key={id}
              ref={(el) => {
                refs.current[id] = el;
              }}
              role="tab"
              id={`s-tab-${id}`}
              aria-selected={section === id}
              aria-controls="s-pane"
              tabIndex={section === id ? 0 : -1}
              className="settings__tab"
              onClick={() => setSection(id)}
            >
              <Icon name={icon} />
              {t.settings.nav[id]}
            </button>
          ))}
        </div>
      </nav>
      <main className="settings__main" role="tabpanel" id="s-pane" aria-labelledby={`s-tab-${section}`}>
        {section === 'models' && <ModelsSection />}
        {section === 'observation' && <ObservationSection />}
        {section === 'memory' && <MemorySection />}
        {section === 'appearance' && <AppearanceSection />}
      </main>
      <Toast />
    </div>
  );
}

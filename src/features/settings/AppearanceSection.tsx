import { useState } from 'react';
import { getThemePref, setThemePref, type ThemePref } from '../../lib/theme';
import { t } from '../../lib/strings';

export function AppearanceSection() {
  const [pref, setPref] = useState<ThemePref>(getThemePref);
  return (
    <section className="sec">
      <h2>{t.settings.appearance.theme}</h2>
      <div className="seg" role="radiogroup" aria-label={t.settings.appearance.theme}>
        {(['system', 'light', 'dark'] as const).map((p) => (
          <label key={p} className={`seg__item ${pref === p ? 'seg__item--on' : ''}`}>
            <input
              type="radio"
              name="theme"
              value={p}
              checked={pref === p}
              onChange={() => {
                setPref(p);
                setThemePref(p);
              }}
            />
            {t.settings.appearance[p]}
          </label>
        ))}
      </div>
    </section>
  );
}

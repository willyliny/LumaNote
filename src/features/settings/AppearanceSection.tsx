import { useState } from 'react';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Sprite } from '../../components/Sprite';
import { SPRITE_STATES } from '../../components/spriteState';
import { notify } from '../../state/app-store';
import { saveAppearance, useAppearance } from '../../state/appearance-store';
import { ALL_SKINS, DEFAULT_SKIN_ID, getSkin, isComplete } from '../../skins/registry';
import type { Skin } from '../../skins/types';
import type { ThemeChoice } from '../../lib/ipc-types';

export function AppearanceSection() {
  const theme = useAppearance((s) => s.theme);
  const activeId = useAppearance((s) => s.skinId);
  const requestedId = useAppearance((s) => s.requestedSkinId);
  const fallback = useAppearance((s) => s.fallback);
  const saving = useAppearance((s) => s.saving);
  // Selection is local to this window: previewing never saves and never reaches other windows.
  const [picked, setPicked] = useState<string | null>(null);
  const selectedId = picked ?? activeId;
  const selected = getSkin(selectedId);
  const active = getSkin(activeId);
  const defaultSkin = getSkin(DEFAULT_SKIN_ID);

  const canApply = isComplete(selected) && selectedId !== activeId && !saving;
  const canRestore = !saving && (activeId !== DEFAULT_SKIN_ID || selectedId !== DEFAULT_SKIN_ID);

  const apply = async () => {
    if (!isComplete(selected)) return;
    if (await saveAppearance({ skin_id: selected.id })) {
      setPicked(null);
      notify('info', t.skins.applied(selected.name));
    }
  };
  const restore = async () => {
    if (await saveAppearance({ skin_id: DEFAULT_SKIN_ID })) {
      setPicked(null);
      notify('info', t.skins.restored(defaultSkin?.name ?? DEFAULT_SKIN_ID));
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    const usable = ALL_SKINS.filter((s) => s.complete);
    const i = usable.findIndex((s) => s.id === selectedId);
    let n = i;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') n = (i + 1) % usable.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') n = (i + usable.length - 1) % usable.length;
    else return;
    e.preventDefault();
    const next = usable[n];
    if (next) {
      setPicked(next.id);
      document.getElementById(`skin-${next.id}`)?.focus();
    }
  };

  return (
    <div className="sec-stack">
      <section className="sec">
        <h2>{t.settings.appearance.theme}</h2>
        <div className="seg" role="radiogroup" aria-label={t.settings.appearance.theme}>
          {(['system', 'light', 'dark'] as const satisfies readonly ThemeChoice[]).map((p) => (
            <label key={p} className={`seg__item ${theme === p ? 'seg__item--on' : ''}`}>
              <input
                type="radio"
                name="theme"
                value={p}
                checked={theme === p}
                onChange={() => void saveAppearance({ theme: p })}
              />
              {t.settings.appearance[p]}
            </label>
          ))}
        </div>
      </section>

      <section className="sec" aria-labelledby="skins-h">
        <h2 id="skins-h">{t.skins.heading}</h2>
        <p className="sec__intro">{t.skins.intro}</p>

        {fallback && (
          <p className="skins__notice" role="status" data-testid="skin-fallback">
            <Icon name="alert" />
            {t.skins.fallbackInline(getSkin(requestedId)?.name ?? requestedId, active?.name ?? '')}
          </p>
        )}

        <div className="skins" role="radiogroup" aria-label={t.skins.groupLabel} onKeyDown={onKey}>
          {ALL_SKINS.map((s) => (
            <SkinCard
              key={s.id}
              skin={s}
              current={s.id === activeId}
              selected={s.id === selectedId}
              onPick={() => s.complete && setPicked(s.id)}
            />
          ))}
        </div>

        {isComplete(selected) && (
          <div className="skinpreview" data-testid="skin-preview">
            <h3>{t.skins.previewTitle(selected.name)}</h3>
            <div className="skinpreview__grid">
              {(['light', 'dark'] as const).map((bg) => (
                <div key={bg} className={`skinpreview__desk skinpreview__desk--${bg}`}>
                  <ul>
                    {SPRITE_STATES.map((st) => (
                      <li key={st}>
                        <Sprite state={st} skin={selected} size={64} animate={false} title={t.sprite.label[st]} />
                        <span>{t.sprite.label[st]}</span>
                      </li>
                    ))}
                  </ul>
                  <p>{bg === 'light' ? t.skins.lightDesktop : t.skins.darkDesktop}</p>
                </div>
              ))}
            </div>
            <p className="subtle">{t.skins.previewNote}</p>
          </div>
        )}

        <div className="btnrow">
          <button className="btn btn--primary" disabled={!canApply} onClick={() => void apply()}>
            {saving ? t.skins.saving : t.skins.apply}
          </button>
          <button className="btn" disabled={!canRestore} onClick={() => void restore()}>
            {t.skins.restore}
          </button>
        </div>
      </section>
    </div>
  );
}

function SkinCard({
  skin,
  current,
  selected,
  onPick,
}: {
  skin: Skin;
  current: boolean;
  selected: boolean;
  onPick: () => void;
}) {
  const family = t.skins.family[skin.family];
  return (
    <button
      id={`skin-${skin.id}`}
      type="button"
      role="radio"
      aria-checked={selected}
      aria-disabled={!skin.complete}
      tabIndex={selected ? 0 : -1}
      aria-label={t.skins.cardLabel(skin.name, family, current, skin.complete, !!skin.layered)}
      className={`skin ${selected ? 'skin--on' : ''} ${skin.complete ? '' : 'skin--wait'}`}
      data-skin-id={skin.id}
      onClick={onPick}
    >
      <span className="skin__stage" aria-hidden="true">
        {isComplete(skin) ? (
          // only the selected card moves: a calm page, and one animated sprite at most
          <Sprite state="idle" skin={skin} size={64} animate={selected} title="" plain />
        ) : (
          <Icon name="ring" />
        )}
      </span>
      <span className="skin__name">{skin.name}</span>
      <span className="skin__family">{family}</span>
      <span className="skin__marks">
        {current && (
          <span className="badge badge--primary">
            <Icon name="check" />
            {t.skins.current}
          </span>
        )}
        {!current && selected && skin.complete && <span className="badge badge--plain">{t.skins.previewing}</span>}
        {skin.layered && (
          <span className="badge badge--plain" title={t.skins.layeredTip} data-testid="badge-layered">
            {t.skins.layered}
          </span>
        )}
        {!skin.complete && (
          <span className="badge badge--warn">
            <Icon name="clock" />
            {t.skins.preparing}
          </span>
        )}
        {skin.complete && skin.preferredTheme && !current && !selected && (
          <span className="subtle">{t.skins.preferredTheme[skin.preferredTheme]}</span>
        )}
      </span>
    </button>
  );
}

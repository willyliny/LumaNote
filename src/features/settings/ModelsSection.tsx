import { useCallback, useEffect, useState } from 'react';
import { ipc } from '../../lib/ipc';
import {
  IpcError,
  type CapabilityName,
  type ConnectionTestResult,
  type ModelProfile,
  type ModelProfileInput,
  type ModelProfileList,
  type ModelPurpose,
  type ModelRoute,
  type ProtocolType,
} from '../../lib/ipc-types';
import { formatWhen } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { notify, reportError } from '../../state/app-store';

const CAPS: CapabilityName[] = ['text', 'streaming', 'tool_calling', 'structured_output', 'image_input'];
const PURPOSES: ModelPurpose[] = ['chat', 'background_summary', 'research', 'coding', 'vision'];
const NEEDS: Partial<Record<ModelPurpose, CapabilityName>> = { coding: 'tool_calling', vision: 'image_input' };
const loopback = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/;

const EMPTY: ModelProfileInput = {
  id: null,
  display_name: '',
  provider: '',
  protocol: 'openai_chat_compatible',
  base_url: 'https://',
  model_id: '',
  credential: null,
};

export function ModelsSection() {
  const [data, setData] = useState<ModelProfileList | null>(null);
  const [editing, setEditing] = useState<ModelProfileInput | null>(null);
  const [tests, setTests] = useState<Record<string, ConnectionTestResult | 'running'>>({});

  const load = useCallback(async () => {
    try {
      setData(await ipc.invoke('list_model_profiles', {}));
    } catch (e) {
      reportError(e);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  if (!data) return <p className="subtle">{t.common.loading}</p>;

  const runTest = async (p: ModelProfile) => {
    setTests((x) => ({ ...x, [p.id]: 'running' }));
    try {
      const r = await ipc.invoke('test_model_profile', { profile_id: p.id });
      setTests((x) => ({ ...x, [p.id]: r }));
      await load();
    } catch (e) {
      setTests((x) => {
        const { [p.id]: _drop, ...rest } = x;
        return rest;
      });
      reportError(e);
    }
  };

  const remove = async (p: ModelProfile) => {
    try {
      await ipc.invoke('delete_model_profile', { id: p.id });
      notify('info', t.models.deleted);
      await load();
    } catch (e) {
      reportError(e);
    }
  };

  const setRoute = async (purpose: ModelPurpose, profileId: string | null) => {
    const routes: ModelRoute[] = data.routes.map((r) =>
      r.purpose === purpose ? { ...r, primary_profile_id: profileId } : r,
    );
    try {
      const saved = await ipc.invoke('set_model_routes', { routes });
      setData({ ...data, routes: saved });
      notify('info', t.models.routesSaved);
    } catch (e) {
      reportError(e);
    }
  };

  return (
    <div className="sec-stack">
      <section className="sec">
        <div className="sec__head">
          <div>
            <h2>{t.models.heading}</h2>
            <p className="sec__intro">{t.models.intro}</p>
          </div>
          <button className="btn btn--primary" onClick={() => setEditing({ ...EMPTY })}>
            <Icon name="plus" />
            {t.models.add}
          </button>
        </div>

        {editing && (
          <ProfileForm
            input={editing}
            protocols={data.protocols}
            existing={data.profiles.find((p) => p.id === editing.id) ?? null}
            onCancel={() => setEditing(null)}
            onSaved={async () => {
              setEditing(null);
              notify('info', t.models.saved);
              await load();
            }}
          />
        )}

        <ul className="profiles">
          {data.profiles.map((p) => (
            <ProfileCard
              key={p.id}
              p={p}
              test={tests[p.id]}
              onEdit={() =>
                setEditing({
                  id: p.id,
                  display_name: p.display_name,
                  provider: p.provider,
                  protocol: p.protocol,
                  base_url: p.base_url,
                  model_id: p.model_id,
                  credential: null,
                })
              }
              onTest={() => runTest(p)}
              onDelete={() => remove(p)}
            />
          ))}
        </ul>
      </section>

      <section className="sec">
        <h2>{t.models.routesHeading}</h2>
        <p className="sec__intro">{t.models.routesIntro}</p>
        <ul className="routes">
          {PURPOSES.map((purpose) => {
            const route = data.routes.find((r) => r.purpose === purpose);
            const need = NEEDS[purpose];
            return (
              <li key={purpose} className="route">
                <label htmlFor={`route-${purpose}`} className="route__label">
                  <span>{t.models.purpose[purpose]}</span>
                  {t.models.purposeNeeds[purpose] && <span className="subtle">{t.models.purposeNeeds[purpose]}</span>}
                </label>
                <select
                  id={`route-${purpose}`}
                  className="select"
                  value={route?.primary_profile_id ?? ''}
                  onChange={(e) => void setRoute(purpose, e.target.value || null)}
                >
                  <option value="">{t.models.notAssigned}</option>
                  {data.profiles.map((p) => {
                    const state = need ? p.capabilities.states[need] : 'supported';
                    return (
                      <option key={p.id} value={p.id} disabled={state === 'unsupported'}>
                        {p.display_name}
                        {state === 'unsupported' ? t.models.lacksSuffix : state === 'unknown' && need ? t.models.notVerifiedSuffix : ''}
                      </option>
                    );
                  })}
                </select>
              </li>
            );
          })}
        </ul>
      </section>
    </div>
  );
}

function CapabilityList({ p, caps }: { p: ModelProfile['capabilities']; caps?: CapabilityName[] }) {
  return (
    <ul className="caps" aria-label={t.models.capabilities}>
      {(caps ?? CAPS).map((c) => {
        const s = p.states[c];
        return (
          <li key={c} className={`cap cap--${s}`}>
            <Icon name={s === 'supported' ? 'check' : s === 'unsupported' ? 'x' : 'question'} />
            {t.models.capabilityName[c]}
            <span className="cap__state">{t.models.capabilityState[s]}</span>
          </li>
        );
      })}
    </ul>
  );
}

function ProfileCard({
  p,
  test,
  onEdit,
  onTest,
  onDelete,
}: {
  p: ModelProfile;
  test: ConnectionTestResult | 'running' | undefined;
  onEdit: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  return (
    <li className="profile">
      <header className="profile__head">
        <h3>{p.display_name}</h3>
        <span className={`badge ${p.has_credential ? 'badge--success' : 'badge--warn'}`}>
          <Icon name="lock" />
          {p.has_credential ? t.models.credentialSet : t.models.credentialUnset}
        </span>
      </header>
      <dl className="kv">
        <div>
          <dt>{t.models.provider}</dt>
          <dd>{p.provider}</dd>
        </div>
        <div>
          <dt>{t.models.protocol}</dt>
          <dd>{t.models.protocols[p.protocol]}</dd>
        </div>
        <div>
          <dt>{t.models.baseUrl}</dt>
          <dd className="mono">{p.base_url}</dd>
        </div>
        <div>
          <dt>{t.models.modelId}</dt>
          <dd className="mono">{p.model_id}</dd>
        </div>
      </dl>
      <CapabilityList p={p.capabilities} />
      <p className="subtle">
        {p.verified_at ? t.models.verifiedAt(formatWhen(p.verified_at)) : t.models.neverVerified}
        {p.capabilities.context_limit ? `　${t.models.contextLimit(p.capabilities.context_limit)}` : ''}
      </p>

      {test === 'running' && (
        <p className="testing" role="status">
          <Icon name="spin" className="spin" />
          {t.models.testing}
        </p>
      )}
      {test && test !== 'running' && <TestResult r={test} />}

      <div className="profile__actions">
        <button className="btn btn--small" onClick={onTest} disabled={test === 'running'}>
          {t.models.test}
        </button>
        <button className="btn btn--small" onClick={onEdit}>
          <Icon name="pencil" />
          {t.common.edit}
        </button>
        {confirming ? (
          <span className="profile__confirm" role="alert">
            <span>{t.models.confirmDeleteProfile(p.display_name)}</span>
            <button className="btn btn--danger btn--small" onClick={onDelete}>
              {t.common.confirmDelete}
            </button>
            <button className="btn btn--quiet btn--small" onClick={() => setConfirming(false)}>
              {t.common.cancel}
            </button>
          </span>
        ) : (
          <button className="btn btn--quiet btn--small profile__delete" onClick={() => setConfirming(true)}>
            <Icon name="trash" />
            {t.common.delete}
          </button>
        )}
      </div>
    </li>
  );
}

function TestResult({ r }: { r: ConnectionTestResult }) {
  return (
    <div className={`testresult ${r.ok ? 'testresult--ok' : 'testresult--bad'}`}>
      <p className="testresult__title">
        <Icon name={r.ok ? 'check' : 'x'} />
        {r.ok ? t.models.testPassed : t.models.testFailed}
      </p>
      <ol>
        {r.steps.map((s) => (
          <li key={s.step} className={s.ok ? 'tstep tstep--ok' : 'tstep tstep--bad'}>
            <Icon name={s.ok ? 'check' : 'x'} />
            <span>{t.models.testSteps[s.step]}</span>
            <span className="subtle">
              {s.ok && s.latency_ms !== null
                ? t.models.latency(s.latency_ms)
                : s.error
                  ? `${t.errors.title[s.error.kind]}：${s.error.message}`
                  : ''}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

function ProfileForm({
  input,
  protocols,
  existing,
  onCancel,
  onSaved,
}: {
  input: ModelProfileInput;
  protocols: ModelProfileList['protocols'];
  existing: ModelProfile | null;
  onCancel: () => void;
  onSaved: () => void;
}) {
  const [f, setF] = useState(input);
  const [busy, setBusy] = useState(false);
  const set = <K extends keyof ModelProfileInput>(k: K, v: ModelProfileInput[K]) => setF((x) => ({ ...x, [k]: v }));

  const urlChanged = !!existing && existing.base_url !== f.base_url;
  const needsCredential = urlChanged;
  const insecure = /^http:\/\//.test(f.base_url) && !loopback.test(f.base_url);
  const badUrl = !/^https?:\/\/.+/.test(f.base_url);
  const protoOk = protocols.find((p) => p.protocol === f.protocol)?.available ?? false;
  const valid =
    f.display_name.trim() &&
    f.provider.trim() &&
    f.model_id.trim() &&
    !badUrl &&
    !insecure &&
    protoOk &&
    (!needsCredential || !!f.credential);

  const submit = async () => {
    setBusy(true);
    try {
      await ipc.invoke('save_model_profile', {
        profile: { ...f, credential: f.credential ? f.credential : null },
      });
      onSaved();
    } catch (e) {
      reportError(e instanceof IpcError ? e : e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="pform"
      onSubmit={(e) => {
        e.preventDefault();
        if (valid) void submit();
      }}
    >
      <div className="pform__grid">
        <label className="field">
          <span className="field__label">{t.models.displayName}</span>
          <input className="input" value={f.display_name} onChange={(e) => set('display_name', e.target.value)} />
        </label>
        <label className="field">
          <span className="field__label">{t.models.provider}</span>
          <input className="input" value={f.provider} onChange={(e) => set('provider', e.target.value)} />
        </label>
        <label className="field">
          <span className="field__label">{t.models.protocol}</span>
          <select className="select" value={f.protocol} onChange={(e) => set('protocol', e.target.value as ProtocolType)}>
            {protocols.map((p) => (
              <option key={p.protocol} value={p.protocol} disabled={!p.available}>
                {t.models.protocols[p.protocol]}
                {p.available ? '' : `（${t.models.unavailable}）`}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span className="field__label">{t.models.modelId}</span>
          <input className="input mono" value={f.model_id} onChange={(e) => set('model_id', e.target.value)} />
        </label>
        <label className="field pform__wide">
          <span className="field__label">{t.models.baseUrl}</span>
          <input
            className="input mono"
            inputMode="url"
            value={f.base_url}
            onChange={(e) => set('base_url', e.target.value)}
            aria-invalid={insecure || badUrl}
          />
          {insecure && <span className="field__error">{t.models.httpWarn}</span>}
        </label>
        <label className="field pform__wide">
          <span className="field__label">{t.models.credential}</span>
          {/* Write-only: never prefilled, never read back from the backend. */}
          <input
            className="input mono"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={f.credential ?? ''}
            placeholder={
              needsCredential
                ? t.models.credentialPlaceholderRequired
                : existing?.has_credential
                  ? t.models.credentialPlaceholderKeep
                  : t.models.credentialPlaceholderNew
            }
            onChange={(e) => set('credential', e.target.value)}
          />
          <span className="field__help">{t.models.credentialHelp}</span>
        </label>
      </div>
      <div className="pform__actions">
        <button type="submit" className="btn btn--primary" disabled={!valid || busy}>
          {t.common.save}
        </button>
        <button type="button" className="btn" onClick={onCancel}>
          {t.common.cancel}
        </button>
      </div>
    </form>
  );
}

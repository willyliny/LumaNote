import { ipc } from '../lib/ipc';
import {
  IpcError,
  TERMINAL_STATUSES,
  type ChatAttachment,
  type ChatMessage,
  type EventMap,
  type ObservationPolicy,
  type ObservationStatus,
  type Task,
  type TaskStatus,
  type TodaySnapshot,
} from '../lib/ipc-types';
import { createStore, useStore } from '../lib/store';
import { t } from '../lib/strings';
import type { SpriteState } from '../components/spriteState';

export const CONVERSATION_ID = 'conv-main';

export interface Notice {
  id: number;
  tone: 'info' | 'error';
  text: string;
  detail?: string;
}

interface AppState {
  ready: boolean;
  syncError: boolean;
  tasks: Record<string, Task>;
  messages: ChatMessage[];
  today: TodaySnapshot | null;
  observation: { policy: ObservationPolicy; status: ObservationStatus } | null;
  /** UI-only: the chat input is focused / voice is listening (shared across windows) */
  listening: boolean;
  notice: Notice | null;
}

export const appStore = createStore<AppState>({
  ready: false,
  syncError: false,
  tasks: {},
  messages: [],
  today: null,
  observation: null,
  listening: false,
  notice: null,
});
export const useApp = <R>(select: (s: AppState) => R): R => useStore(appStore, select);

/* ------------------------------------------------------------------ */
/* Authoritative fetches. Events are hints; these calls are the truth. */
/* ------------------------------------------------------------------ */

/** last applied sequence per task_id, and for task-less events */
const lastSeq = new Map<string, number>();
const GLOBAL = '__global__';

let noticeId = 0;
export function notify(tone: Notice['tone'], text: string, detail?: string) {
  appStore.set({ notice: { id: ++noticeId, tone, text, detail } });
}
export function dismissNotice() {
  appStore.set({ notice: null });
}

export function reportError(e: unknown) {
  if (e instanceof IpcError) {
    notify('error', t.errors.title[e.kind], t.errors.hint[e.kind]);
  } else {
    notify('error', t.errors.action, e instanceof Error ? e.message : String(e));
  }
}

function putTask(task: Task) {
  const prev = lastSeq.get(task.id) ?? -1;
  lastSeq.set(task.id, Math.max(prev, task.sequence));
  appStore.set((s) => ({ tasks: { ...s.tasks, [task.id]: task } }));
}

export async function refreshTask(taskId: string): Promise<Task | null> {
  try {
    const task = await ipc.invoke('get_task', { task_id: taskId });
    putTask(task);
    if (TERMINAL_STATUSES.includes(task.status)) void refreshToday();
    return task;
  } catch (e) {
    if (e instanceof IpcError && e.kind === 'not_found') return null;
    appStore.set({ syncError: true });
    return null;
  }
}

export async function refreshMessages() {
  try {
    const messages = await ipc.invoke('list_messages', { conversation_id: CONVERSATION_ID });
    appStore.set({ messages });
  } catch {
    appStore.set({ syncError: true });
  }
}

let todayTimer: ReturnType<typeof setTimeout> | undefined;
export function refreshToday(): Promise<void> {
  clearTimeout(todayTimer);
  return new Promise((resolve) => {
    todayTimer = setTimeout(async () => {
      try {
        const today = await ipc.invoke('get_today', {});
        appStore.set({ today });
      } catch {
        appStore.set({ syncError: true });
      }
      resolve();
    }, 80);
  });
}

export async function refreshObservation() {
  try {
    const observation = await ipc.invoke('get_observation_policy', {});
    appStore.set({ observation });
  } catch {
    appStore.set({ syncError: true });
  }
}

/** Mount / reconnect: replace everything with what the backend says is true. */
export async function syncAll() {
  try {
    const [tasks] = await Promise.all([
      ipc.invoke('list_tasks', { statuses: null }),
      refreshMessages(),
      refreshToday(),
      refreshObservation(),
    ]);
    lastSeq.clear();
    for (const task of tasks) lastSeq.set(task.id, task.sequence);
    appStore.set({
      tasks: Object.fromEntries(tasks.map((x) => [x.id, x])),
      ready: true,
      syncError: false,
    });
  } catch {
    appStore.set({ syncError: true, ready: true });
  }
}

/* ------------------------------------------------------------------ */
/* Event handling with sequence-gap detection                          */
/* ------------------------------------------------------------------ */

type Verdict = 'apply' | 'duplicate' | 'gap';

function judge(key: string, seq: number, known: boolean): Verdict {
  const last = lastSeq.get(key);
  if (last === undefined) return known ? 'apply' : 'gap';
  if (seq <= last) return 'duplicate';
  if (seq > last + 1) return 'gap';
  return 'apply';
}

function patchTask(taskId: string, fn: (t: Task) => Task) {
  appStore.set((s) => {
    const cur = s.tasks[taskId];
    return cur ? { tasks: { ...s.tasks, [taskId]: fn(cur) } } : {};
  });
}

/** Re-read both the task and its messages, because a lost event can hide either. */
async function resyncTask(taskId: string) {
  await refreshMessages(); // messages first: a later duplicate is impossible, a gap heals on `done`
  await refreshTask(taskId);
}

function handleTaskEvent<E extends { task_id: string | null; sequence: number }>(
  ev: E,
  apply: (ev: E) => void,
) {
  const id = ev.task_id as string;
  const known = id in appStore.get().tasks;
  const verdict = judge(id, ev.sequence, known);
  if (verdict === 'duplicate') return;
  if (verdict === 'gap') {
    // Don't guess what we missed. Ask the backend.
    lastSeq.set(id, ev.sequence);
    void resyncTask(id);
    return;
  }
  lastSeq.set(id, ev.sequence);
  apply(ev);
}

function onProgress(ev: EventMap['task_progress']) {
  handleTaskEvent(ev, (e) => {
    // Terminal states are never inferred from an event: confirm with get_task.
    if (TERMINAL_STATUSES.includes(e.status)) {
      void refreshTask(e.task_id);
      return;
    }
    patchTask(e.task_id, (task) => {
      let steps = task.steps;
      if (e.step) {
        const i = steps.findIndex((x) => x.seq === e.step!.seq);
        steps = i >= 0 ? steps.map((x, j) => (j === i ? e.step! : x)) : [...steps, e.step];
      }
      return {
        ...task,
        status: e.status,
        steps,
        sequence: e.sequence,
        error: e.error ?? task.error,
        updated_at: new Date().toISOString(),
      };
    });
    if (e.status === 'awaiting_user' || e.status === 'paused' || e.status === 'uncertain') {
      void refreshTask(e.task_id); // pull needs_user / pending_approval text from the source
    }
    if (e.step && e.step.side_effect === 'local_write' && e.step.status === 'succeeded') {
      void refreshToday();
    }
  });
}

function onDelta(ev: EventMap['assistant_delta']) {
  handleTaskEvent(ev, (e) => {
    appStore.set((s) => {
      const i = s.messages.findIndex((m) => m.id === e.message_id);
      if (i < 0) {
        const msg: ChatMessage = {
          id: e.message_id,
          conversation_id: e.conversation_id,
          task_id: e.task_id,
          role: 'assistant',
          content: e.delta,
          created_at: new Date().toISOString(),
          sources: e.sources ?? [],
          artifact_ids: [],
          attachments: [],
          streaming: !e.done,
        };
        return { messages: [...s.messages, msg] };
      }
      const messages = s.messages.slice();
      const cur = messages[i]!;
      messages[i] = {
        ...cur,
        content: cur.content + e.delta,
        streaming: !e.done,
        sources: e.sources ?? cur.sources,
      };
      return { messages };
    });
    if (e.done) void refreshMessages(); // final text always comes from the backend
  });
}

function onApproval(ev: EventMap['approval_required']) {
  handleTaskEvent(ev, (e) => {
    patchTask(e.task_id, (task) => ({ ...task, pending_approval: e.approval }));
  });
}

function onArtifact(ev: EventMap['artifact_ready']) {
  handleTaskEvent(ev, (e) => {
    patchTask(e.task_id, (task) =>
      task.artifacts.some((a) => a.id === e.artifact.id)
        ? task
        : { ...task, artifacts: [...task.artifacts, e.artifact] },
    );
  });
}

function onSuggestion(ev: EventMap['suggestion_ready']) {
  const verdict = judge(GLOBAL, ev.sequence, true);
  if (verdict === 'duplicate') return;
  lastSeq.set(GLOBAL, ev.sequence);
  if (verdict === 'gap') void Promise.all([refreshToday(), refreshObservation()]);
  else {
    appStore.set((s) =>
      s.today && !s.today.suggestions.some((x) => x.id === ev.suggestion.id)
        ? { today: { ...s.today, suggestions: [...s.today.suggestions, ev.suggestion] } }
        : {},
    );
  }
}

function onObservation(ev: EventMap['observation_status']) {
  const verdict = judge(GLOBAL, ev.sequence, true);
  if (verdict === 'duplicate') return;
  lastSeq.set(GLOBAL, ev.sequence);
  if (verdict === 'gap') void Promise.all([refreshToday(), refreshObservation()]);
  appStore.set((s) => (s.observation ? { observation: { ...s.observation, status: ev.status } } : {}));
}

let started = false;
export function startApp() {
  if (started) return;
  started = true;
  ipc.listen('task_progress', onProgress);
  ipc.listen('assistant_delta', onDelta);
  ipc.listen('approval_required', onApproval);
  ipc.listen('artifact_ready', onArtifact);
  ipc.listen('suggestion_ready', onSuggestion);
  ipc.listen('observation_status', onObservation);
  void syncAll();

  // Reconnect triggers: window shown again, network back, tab refocused.
  let last = 0;
  const resync = () => {
    if (Date.now() - last < 1500) return;
    last = Date.now();
    void syncAll();
  };
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') resync();
  });
  window.addEventListener('online', resync);
  window.addEventListener('focus', resync);

  // Cross-window "listening" flag (chat input focused in the panel, shown on the floating sprite).
  if (typeof BroadcastChannel !== 'undefined') {
    const ch = new BroadcastChannel('luma-ui');
    ch.onmessage = (e: MessageEvent) => {
      if (typeof e.data?.listening === 'boolean') appStore.set({ listening: e.data.listening });
    };
    uiChannel = ch;
  }
}
let uiChannel: BroadcastChannel | null = null;
export function setListening(v: boolean) {
  appStore.set({ listening: v });
  uiChannel?.postMessage({ listening: v });
}

/* ------------------------------------------------------------------ */
/* Actions                                                             */
/* ------------------------------------------------------------------ */

async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    reportError(e);
    return undefined;
  }
}

export const actions = {
  submit: (intent: string, attachments: ChatAttachment[]) =>
    run(async () => {
      const r = await ipc.invoke('submit_task', {
        intent,
        conversation_id: CONVERSATION_ID,
        attachments,
      });
      await Promise.all([refreshMessages(), refreshTask(r.task_id)]);
      return r;
    }),
  cancel: (taskId: string) =>
    run(async () => {
      try {
        putTask(await ipc.invoke('cancel_task', { task_id: taskId }));
      } catch (e) {
        if (e instanceof IpcError && e.kind === 'invalid_transition') {
          notify('error', t.tasks.cancelFailed);
          await refreshTask(taskId);
          return;
        }
        throw e;
      }
      await refreshMessages();
    }),
  retry: (taskId: string) =>
    run(async () => {
      const r = await ipc.invoke('retry_task', { task_id: taskId });
      await refreshTask(r.task_id);
    }),
  resume: (taskId: string) => run(async () => putTask(await ipc.invoke('resume_task', { task_id: taskId }))),
  resolveUncertain: (taskId: string, outcome: 'completed' | 'failed') =>
    run(async () => {
      putTask(await ipc.invoke('resolve_uncertain_task', { task_id: taskId, outcome }));
    }),
  respondApproval: (approvalId: string, decision: 'approve_once' | 'deny') =>
    run(async () => {
      putTask(await ipc.invoke('respond_to_approval', { approval_id: approvalId, decision }));
    }),
  toggleTodo: (id: string, done: boolean) =>
    run(async () => {
      await ipc.invoke('update_todo', { id, done });
      await refreshToday();
    }),
  respondSuggestion: (id: string, response: 'accept' | 'later' | 'never') =>
    run(async () => {
      const r = await ipc.invoke('respond_to_suggestion', { suggestion_id: id, response });
      notify(
        'info',
        response === 'accept' ? t.today.suggestionDone : response === 'later' ? t.today.suggestionLater : t.today.suggestionNever,
      );
      await refreshToday();
      if (r.task_id) await refreshTask(r.task_id);
    }),
  setPolicy: (policy: ObservationPolicy) =>
    run(async () => {
      const r = await ipc.invoke('set_observation_policy', { policy });
      appStore.set({ observation: r });
    }),
};

/* ------------------------------------------------------------------ */
/* Selectors                                                           */
/* ------------------------------------------------------------------ */

export type TaskGroup = 'waiting' | 'running' | 'pending' | 'failed' | 'done';
export const GROUP_ORDER: TaskGroup[] = ['waiting', 'running', 'pending', 'failed', 'done'];

export function groupOf(status: TaskStatus): TaskGroup {
  switch (status) {
    case 'awaiting_user':
    case 'paused':
    case 'uncertain':
      return 'waiting';
    case 'running':
    case 'verifying':
      return 'running';
    case 'queued':
      return 'pending';
    case 'failed':
    case 'cancelled':
      return 'failed';
    case 'completed':
      return 'done';
  }
}

export function selectJobs(tasks: Record<string, Task>): Task[] {
  return Object.values(tasks).filter((x) => x.kind === 'job');
}

export function deriveSpriteState(s: AppState): SpriteState {
  const tasks = Object.values(s.tasks);
  if (tasks.some((x) => x.status === 'awaiting_user')) return 'awaiting';
  if (s.listening) return 'listening';
  if (tasks.some((x) => x.status === 'running' || x.status === 'verifying')) return 'working';
  const ob = s.observation?.status;
  if (ob && (ob.dnd || ob.paused)) return 'dnd';
  if (s.today?.suggestions.length) return 'suggestion';
  return 'idle';
}

import { FLOATING_COLLAPSED_PX, FLOATING_EXPANDED_PX } from '../features/floating/layout';
import type { IpcClient } from './ipc';
import type { Appearance } from './ipc-types';
import {
  IpcError,
  SCHEMA_VERSION,
  TASK_TRANSITIONS,
  TERMINAL_STATUSES,
  type AgendaItem,
  type ApprovalRequest,
  type Artifact,
  type AssistantDeltaEvent,
  type CapabilityResults,
  type ChatAttachment,
  type ChatMessage,
  type CommandArgs,
  type CommandName,
  type CommandResult,
  type ConnectionTestResult,
  type EventMap,
  type EventName,
  type IpcErrorKind,
  type Memory,
  type ModelProfile,
  type ModelRoute,
  type ObservationPolicy,
  type ObservationStatus,
  type ProtocolInfo,
  type Schedule,
  type Source,
  type Suggestion,
  type Task,
  type TaskStatus,
  type TaskStep,
  type TestStepResult,
  type Todo,
  type TodaySnapshot,
  type Unsubscribe,
} from './ipc-types';

/* ------------------------------------------------------------------ */
/* Time helpers. mockSpeed=4 in the URL makes the demo 4x faster.      */
/* ------------------------------------------------------------------ */

function readSpeed(): number {
  try {
    const v = Number(new URLSearchParams(location.search).get('mockSpeed'));
    return v > 0 ? v : 1;
  } catch {
    return 1;
  }
}
const SPEED = readSpeed();
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms / SPEED));

const HOUR = 3600_000;
const DAY = 24 * HOUR;
const isoAt = (ms: number) => new Date(ms).toISOString();
const nowIso = () => new Date().toISOString();

/** Tomorrow 09:00 in Asia/Taipei (UTC+8, no DST) as a UTC instant. */
function taipeiTomorrowAt(hour: number): number {
  const local = new Date(Date.now() + 8 * HOUR);
  const y = local.getUTCFullYear();
  const m = local.getUTCMonth();
  const d = local.getUTCDate() + 1;
  return Date.UTC(y, m, d, hour, 0, 0) - 8 * HOUR;
}
function taipeiTodayAt(hour: number, minute = 0): number {
  const local = new Date(Date.now() + 8 * HOUR);
  return (
    Date.UTC(local.getUTCFullYear(), local.getUTCMonth(), local.getUTCDate(), hour, minute, 0) -
    8 * HOUR
  );
}

/* ------------------------------------------------------------------ */
/* State                                                               */
/* ------------------------------------------------------------------ */

interface MockState {
  counter: number;
  globalSeq: number;
  tasks: Record<string, Task>;
  messages: Record<string, ChatMessage[]>;
  todos: Todo[];
  agenda: AgendaItem[];
  schedules: Schedule[];
  suggestions: Suggestion[];
  dailySummary: TodaySnapshot['daily_summary'];
  memories: Memory[];
  profiles: ModelProfile[];
  routes: ModelRoute[];
  policy: ObservationPolicy;
  lastSampleAt: string | null;
}

const CONVERSATION_ID = 'conv-main';
const DEFAULT_CAPS: CapabilityResults = {
  states: {
    text: 'unknown',
    streaming: 'unknown',
    tool_calling: 'unknown',
    structured_output: 'unknown',
    image_input: 'unknown',
  },
  context_limit: null,
};

const PROTOCOLS: ProtocolInfo[] = [
  { protocol: 'openai_chat_compatible', available: true },
  { protocol: 'anthropic_messages', available: true },
  { protocol: 'openai_responses', available: false },
  { protocol: 'gemini_generate', available: false },
  { protocol: 'local_endpoint', available: false },
];

function seedTask(p: Partial<Task> & Pick<Task, 'id' | 'intent' | 'status'>): Task {
  const created = Date.now() - 20 * HOUR;
  return {
    kind: 'job',
    conversation_id: CONVERSATION_ID,
    sequence: 6,
    created_at: isoAt(created),
    updated_at: isoAt(created + 12 * 60_000),
    steps: [],
    artifacts: [],
    evidence_refs: [],
    result_ref: null,
    needs_user: null,
    pending_approval: null,
    error: null,
    model: { provider: 'OpenAI 相容', model_id: 'sample-chat-1' },
    tainted_by_external: false,
    ...p,
  };
}

function step(
  seq: number,
  title: string,
  tool: string | null,
  side: TaskStep['side_effect'],
  status: TaskStep['status'] = 'succeeded',
  evidence: string | null = null,
): TaskStep {
  return { seq, title, tool, side_effect: side, status, evidence_ref: evidence };
}

function seedState(): MockState {
  const yesterdayArtifact: Artifact = {
    id: 'art-seed-1',
    task_id: 'task-seed-done',
    kind: 'markdown',
    title: '上週週報重點.md',
    path: 'C:\\Users\\使用者\\文件\\精靈輸出\\上週週報重點.md',
    bytes: 2310,
    content_hash: 'sha256:5e1c…a09f',
    created_at: isoAt(Date.now() - 19 * HOUR),
  };
  const tasks: Task[] = [
    seedTask({
      id: 'task-seed-done',
      intent: '把上週的任務紀錄整理成週報重點',
      status: 'completed',
      steps: [
        step(1, '讀取上週已完成任務（7 筆）', 'read_tasks', 'none'),
        step(2, '產生週報重點草稿', 'write_markdown', 'local_write', 'succeeded', 'file:art-seed-1'),
        step(3, '回讀檔案確認內容', 'verify_file', 'none', 'succeeded', 'hash:5e1c'),
      ],
      artifacts: [yesterdayArtifact],
      evidence_refs: ['file:art-seed-1', 'hash:5e1c'],
      result_ref: 'art-seed-1',
    }),
    seedTask({
      id: 'task-seed-fail',
      intent: '查詢三家雲端儲存服務的企業方案報價並做比較表',
      status: 'failed',
      steps: [
        step(1, '搜尋第一家供應商的價格頁', 'web_search', 'none'),
        step(2, '搜尋第二家供應商的價格頁', 'web_search', 'none', 'failed'),
      ],
      error: {
        kind: 'external_failure',
        message: '搜尋服務回應 503（已重試 3 次）',
        retryable: true,
        occurred_at: isoAt(Date.now() - 3 * HOUR),
      },
      updated_at: isoAt(Date.now() - 3 * HOUR),
    }),
    seedTask({
      id: 'task-seed-denied',
      intent: '讀取 D:\\財務備份 內的對帳單並彙整',
      status: 'failed',
      error: {
        kind: 'permission_denied',
        message: 'D:\\財務備份 不在已授權的資料夾範圍內',
        retryable: false,
        occurred_at: isoAt(Date.now() - 26 * HOUR),
      },
      created_at: isoAt(Date.now() - 27 * HOUR),
      updated_at: isoAt(Date.now() - 26 * HOUR),
    }),
    seedTask({
      id: 'task-seed-paused',
      intent: '分析 shop-api 專案 build 失敗的原因',
      status: 'paused',
      steps: [
        step(1, '讀取 build 日誌', 'read_file', 'none'),
        step(2, '搜尋相關程式碼', 'search_code', 'none'),
        step(3, '比對最近三次提交', 'git_diff', 'none'),
      ],
      needs_user: {
        kind: 'resume',
        reason: 'tool_limit',
        summary: '已用完這個任務的 20 次工具呼叫。要繼續請按「繼續」。',
      },
      updated_at: isoAt(Date.now() - 50 * 60_000),
    }),
    seedTask({
      id: 'task-seed-uncertain',
      intent: '把週會時間改到週四下午三點並通知設計組',
      status: 'uncertain',
      steps: [
        step(1, '讀取行程', 'calendar_read', 'none'),
        step(2, '更新外部事件', 'calendar_update', 'external_write', 'started'),
      ],
      needs_user: {
        kind: 'confirm_outcome',
        summary: '更新後連線中斷，無法確認外部事件是否已改成功。請到行事曆確認後告訴我結果。',
      },
      error: {
        kind: 'outcome_uncertain',
        message: '外部寫入後連線中斷，讀回確認逾時',
        retryable: false,
        occurred_at: isoAt(Date.now() - 2 * HOUR),
      },
      updated_at: isoAt(Date.now() - 2 * HOUR),
    }),
    seedTask({
      id: 'task-seed-queued',
      intent: '整理「下載」資料夾裡的收據圖片並建立清單',
      status: 'queued',
      sequence: 1,
      model: null,
      created_at: isoAt(Date.now() - 4 * 60_000),
      updated_at: isoAt(Date.now() - 4 * 60_000),
    }),
  ];

  const todos: Todo[] = [
    {
      id: 'todo-1',
      title: '繳交本月差旅報銷',
      due_at: isoAt(taipeiTodayAt(17)),
      due_uncertain: false,
      done: false,
      source_task_id: null,
    },
    {
      id: 'todo-2',
      title: '整理「下載」資料夾',
      due_at: null,
      due_uncertain: false,
      done: false,
      source_task_id: null,
    },
    {
      id: 'todo-3',
      title: '預約年度健康檢查',
      due_at: isoAt(taipeiTodayAt(12) - DAY),
      due_uncertain: false,
      done: true,
      source_task_id: null,
    },
  ];

  const memories: Memory[] = [
    {
      id: 'mem-1',
      type: 'explicit_preference',
      content: '週報使用條列式，結尾附下週計畫。',
      confidence: null,
      sources: [{ kind: 'user_statement', id: 'msg-0301', label: '你在 11/03 的對話中說的' }],
      evidence_note: null,
      confirmed_at: isoAt(Date.now() - 8 * DAY),
      expires_at: null,
      created_at: isoAt(Date.now() - 8 * DAY),
    },
    {
      id: 'mem-2',
      type: 'task_fact',
      content: '專案 A 的輸出資料夾是 C:\\Users\\使用者\\文件\\專案A\\輸出。',
      confidence: null,
      sources: [{ kind: 'task', id: 'task-seed-done', label: '任務「整理週報重點」' }],
      evidence_note: null,
      confirmed_at: null,
      expires_at: null,
      created_at: isoAt(Date.now() - 5 * DAY),
    },
    {
      id: 'mem-3',
      type: 'short_term',
      content: '本週在準備年度報價，報價相關任務優先處理。',
      confidence: null,
      sources: [{ kind: 'user_statement', id: 'msg-1109', label: '你在 11/09 的對話中提到' }],
      evidence_note: null,
      confirmed_at: null,
      expires_at: isoAt(Date.now() + 4 * DAY),
      created_at: isoAt(Date.now() - 2 * DAY),
    },
    {
      id: 'mem-4',
      type: 'inferred_habit',
      content: '上午常使用開發工具超過 45 分鐘。',
      confidence: 0.7,
      sources: [{ kind: 'activity_summary', id: 'sum-wk45', label: '過去兩週的活動日彙整' }],
      evidence_note: '過去 10 個有觀察資料的工作日中，有 7 天符合；有 2 天因休假沒有資料。',
      confirmed_at: null,
      expires_at: isoAt(Date.now() + 26 * DAY),
      created_at: isoAt(Date.now() - 4 * DAY),
    },
    {
      id: 'mem-5',
      type: 'inferred_habit',
      content: '週五下午常整理本週完成的事項。',
      confidence: 0.55,
      sources: [{ kind: 'activity_summary', id: 'sum-wk44', label: '過去四週的活動日彙整' }],
      evidence_note: '過去 4 個週五中有 3 天符合。樣本偏少，僅供參考。',
      confirmed_at: null,
      expires_at: isoAt(Date.now() + 20 * DAY),
      created_at: isoAt(Date.now() - 4 * DAY),
    },
  ];

  const profiles: ModelProfile[] = [
    {
      id: 'prof-1',
      display_name: '日常主力',
      provider: 'OpenAI 相容服務',
      protocol: 'openai_chat_compatible',
      base_url: 'https://api.sample-llm.example/v1',
      model_id: 'sample-chat-1',
      has_credential: true,
      capabilities: {
        states: {
          text: 'supported',
          streaming: 'supported',
          tool_calling: 'supported',
          structured_output: 'unknown',
          image_input: 'unsupported',
        },
        context_limit: 128000,
      },
      verified_at: isoAt(Date.now() - 22 * HOUR),
      config_version: 3,
    },
    {
      id: 'prof-2',
      display_name: '研究與長文',
      provider: 'Anthropic',
      protocol: 'anthropic_messages',
      base_url: 'https://api.sample-claude.example',
      model_id: 'sample-long-1',
      has_credential: false,
      capabilities: DEFAULT_CAPS,
      verified_at: null,
      config_version: 1,
    },
  ];

  const routes: ModelRoute[] = [
    { purpose: 'chat', primary_profile_id: 'prof-1', fallback_profile_ids: [] },
    { purpose: 'background_summary', primary_profile_id: 'prof-1', fallback_profile_ids: [] },
    { purpose: 'research', primary_profile_id: null, fallback_profile_ids: [] },
    { purpose: 'coding', primary_profile_id: null, fallback_profile_ids: [] },
    { purpose: 'vision', primary_profile_id: null, fallback_profile_ids: [] },
  ];

  const policy: ObservationPolicy = {
    enabled: true,
    paused_until: null,
    dnd: false,
    sources: [
      { id: 'foreground_app', enabled: true },
      { id: 'idle_state', enabled: true },
      { id: 'window_title', enabled: false },
      { id: 'project_path', enabled: false },
    ],
    exclusions: [
      { id: 'ex-1', kind: 'app', pattern: '1Password.exe' },
      { id: 'ex-2', kind: 'folder', pattern: 'C:\\Users\\使用者\\私人' },
      { id: 'ex-3', kind: 'title_pattern', pattern: '*網路銀行*' },
    ],
    cloud_scope: {
      send_daily_summary: true,
      send_window_titles: false,
      allowed_profile_ids: ['prof-1'],
    },
  };

  const agenda: AgendaItem[] = [
    {
      id: 'ag-1',
      title: '週會',
      start_at: isoAt(taipeiTodayAt(10)),
      end_at: isoAt(taipeiTodayAt(10, 30)),
      source: 'local',
    },
    {
      id: 'ag-2',
      title: '學習時段：Rust 所有權',
      start_at: isoAt(taipeiTodayAt(14)),
      end_at: isoAt(taipeiTodayAt(15)),
      source: 'local',
    },
  ];

  const schedules: Schedule[] = [
    {
      id: 'sch-1',
      title: '備份專案資料夾',
      next_run_at: isoAt(taipeiTodayAt(20, 30)),
      timezone: 'Asia/Taipei',
      original_rule: '每天晚上八點半',
      recurrence: 'FREQ=DAILY',
      enabled: true,
      source_task_id: null,
    },
  ];

  const byId: Record<string, Task> = {};
  for (const t of tasks) byId[t.id] = t;

  return {
    counter: 100,
    globalSeq: 1,
    tasks: byId,
    messages: { [CONVERSATION_ID]: [] },
    todos,
    agenda,
    schedules,
    suggestions: [],
    dailySummary: {
      date: new Date().toISOString().slice(0, 10),
      body: '昨天完成 1 項任務、1 項失敗、1 項結果待確認。今天有 2 項待辦（1 項今天到期）和 2 個行程。',
      source_ids: ['task-seed-done', 'task-seed-fail', 'task-seed-uncertain'],
    },
    memories,
    profiles,
    routes,
    policy,
    lastSampleAt: isoAt(Date.now() - 20_000),
  };
}

/* ------------------------------------------------------------------ */
/* Demo copy                                                           */
/* ------------------------------------------------------------------ */

function nextFriday() {
  const local = new Date(Date.now() + 8 * HOUR);
  const add = ((5 - local.getUTCDay() + 7) % 7) || 7;
  const ms = Date.UTC(local.getUTCFullYear(), local.getUTCMonth(), local.getUTCDate() + add, 17, 0, 0) - 8 * HOUR;
  const d = new Date(ms + 8 * HOUR);
  return { ms, label: `${d.getUTCMonth() + 1}/${d.getUTCDate()}` };
}

const demoSummary = (fri: string) => `## 會議摘要

這次週會確認三件事：**報價單**本週五前寄出、測試環境升級延後到下週、設計組先交出新版圖示。

## 待辦

- 小林：整理報價單並寄給客戶，**本週五（${fri}）前**
- 阿凱：升級測試環境（筆記沒有寫日期，**待確認**）
- 設計組：交出 48px 與 96px 版本的圖示
- 你：週一前回覆設計組關於圖示尺寸的問題

## 需要你決定

- 測試環境升級的日期。筆記只寫「下週」，我先標成待確認，沒有自己猜。

來源是你貼上的筆記，第 3、7、12 行。我可以把這份待辦存成檔案，並設定明早的提醒。`;

const DEMO_SOURCES: Source[] = [
  {
    id: 'src-notes',
    title: '你貼上的會議筆記（第 3、7、12 行）',
    url: null,
    path: null,
    quote: '報價單週五前要出；測試環境下週再說；圖示先給 48 跟 96 兩種。',
  },
  {
    id: 'src-prev',
    title: '精靈輸出／上週週報重點.md',
    url: null,
    path: 'C:\\Users\\使用者\\文件\\精靈輸出\\上週週報重點.md',
    quote: '延續上週：報價單仍是第一優先。',
  },
];

const TODO_FILE_PATH = 'C:\\Users\\使用者\\文件\\專案A\\週會待辦.md';

/* ------------------------------------------------------------------ */
/* Implementation                                                      */
/* ------------------------------------------------------------------ */

type Handlers = {
  [K in CommandName]: (a: CommandArgs<K>) => Promise<CommandResult<K>> | CommandResult<K>;
};

interface MockDebug {
  /** Swallow the next n events on THIS window to prove the UI re-syncs from get_task. */
  dropNextEvents(n: number): void;
  speed: number;
  /** every set_floating_expanded call, with whether the wheel was already in the DOM at call time */
  expandedLog: { expanded: boolean; wheelInDom: boolean }[];
}

/** The mock "backend" keeps appearance in localStorage so it survives reloads and is shared by windows. */
const APPEARANCE_KEY = 'luma-mock-appearance';
function readStoredAppearance(): Appearance {
  try {
    const raw = localStorage.getItem(APPEARANCE_KEY);
    if (raw) {
      const o = JSON.parse(raw) as Partial<Appearance>;
      if (typeof o.skin_id === 'string' && (o.theme === 'light' || o.theme === 'dark' || o.theme === 'system')) {
        return { skin_id: o.skin_id, theme: o.theme };
      }
    }
  } catch {
    /* fall through to default */
  }
  return { skin_id: 'night-letter', theme: 'system' };
}

export function createMockIpc(): IpcClient {
  let state: MockState = seedState();
  const listeners = new Map<EventName, Set<(p: never) => void>>();
  const approvalWaiters = new Map<string, (d: 'approve_once' | 'deny') => void>();
  let dropCount = 0;

  /* Cross-window sync: all mock windows in one browser profile behave as one backend. */
  const channel = typeof BroadcastChannel !== 'undefined' ? new BroadcastChannel('luma-mock') : null;
  let owner = false; // true once this window handled a command; only owners run timers
  const broadcastState = () => channel?.postMessage({ t: 'state', state });
  const deliverLocal = <K extends EventName>(name: K, payload: EventMap[K]) => {
    listeners.get(name)?.forEach((fn) => (fn as (p: EventMap[K]) => void)(structuredClone(payload)));
  };
  if (channel) {
    channel.onmessage = (ev: MessageEvent) => {
      const m = ev.data as
        | { t: 'state'; state: MockState }
        | { t: 'event'; name: EventName; payload: EventMap[EventName] }
        | { t: 'hello' };
      if (m.t === 'state') state = m.state;
      else if (m.t === 'event') deliverLocal(m.name, m.payload as never);
      else if (m.t === 'hello' && owner) broadcastState();
    };
    channel.postMessage({ t: 'hello' });
  }

  const id = (prefix: string) => `${prefix}-${++state.counter}`;

  function emit<K extends EventName>(name: K, payload: EventMap[K]) {
    broadcastState();
    channel?.postMessage({ t: 'event', name, payload });
    if (dropCount > 0) dropCount--;
    else deliverLocal(name, payload);
  }
  const commit = () => broadcastState();

  const base = (task: Task) => ({
    schema_version: SCHEMA_VERSION,
    sequence: ++task.sequence,
    task_id: task.id,
  });
  const globalBase = () => ({
    schema_version: SCHEMA_VERSION,
    sequence: ++state.globalSeq,
    task_id: null,
  });

  const err = (kind: IpcErrorKind, message: string, retryable = false) =>
    new IpcError({ kind, message, retryable });

  const getTask = (taskId: string): Task => {
    const t = state.tasks[taskId];
    if (!t) throw err('not_found', `task ${taskId} not found`);
    return t;
  };
  const isTerminal = (t: Task) => TERMINAL_STATUSES.includes(t.status);

  /** The only place a task status changes; enforces the handbook 7.5 transition table. */
  function transition(
    taskId: string,
    to: TaskStatus,
    extra: { message?: string; step?: TaskStep | null; patch?: Partial<Task> } = {},
  ) {
    const t = getTask(taskId);
    if (!TASK_TRANSITIONS[t.status].includes(to)) {
      throw err('invalid_transition', `${t.status} -> ${to} is not allowed`);
    }
    if (to === 'completed' && t.evidence_refs.length === 0) {
      throw err('invalid_transition', 'completed requires at least one evidence_ref');
    }
    t.status = to;
    t.updated_at = nowIso();
    Object.assign(t, extra.patch ?? {});
    emit('task_progress', {
      ...base(t),
      task_id: t.id,
      status: to,
      step: extra.step ?? null,
      message: extra.message ?? null,
      error: t.error,
    });
  }

  function beginStep(
    taskId: string,
    title: string,
    tool: string | null,
    side: TaskStep['side_effect'],
  ): TaskStep {
    const t = getTask(taskId);
    const s = step(t.steps.length + 1, title, tool, side, 'started');
    t.steps.push(s);
    t.updated_at = nowIso();
    emit('task_progress', {
      ...base(t),
      task_id: t.id,
      status: t.status,
      step: { ...s },
      message: null,
      error: null,
    });
    return s;
  }
  function endStep(taskId: string, s: TaskStep, evidence: string | null, ok = true) {
    const t = getTask(taskId);
    s.status = ok ? 'succeeded' : 'failed';
    s.evidence_ref = evidence;
    if (evidence) t.evidence_refs.push(evidence);
    emit('task_progress', {
      ...base(t),
      task_id: t.id,
      status: t.status,
      step: { ...s },
      message: null,
      error: null,
    });
  }

  async function streamAssistant(
    t: Task,
    text: string,
    sources: Source[],
  ): Promise<ChatMessage> {
    const msg: ChatMessage = {
      id: id('msg'),
      conversation_id: t.conversation_id,
      task_id: t.id,
      role: 'assistant',
      content: '',
      created_at: nowIso(),
      sources: [],
      artifact_ids: [],
      attachments: [],
      streaming: true,
    };
    (state.messages[t.conversation_id] ??= []).push(msg);
    const chars = Array.from(text);
    const send = (delta: string, done: boolean, src: Source[] | null) => {
      msg.content += delta;
      if (done) {
        msg.streaming = false;
        msg.sources = src ?? [];
      }
      const ev: AssistantDeltaEvent = {
        ...base(t),
        task_id: t.id,
        conversation_id: t.conversation_id,
        message_id: msg.id,
        delta,
        done,
        sources: src,
      };
      emit('assistant_delta', ev);
    };
    for (let i = 0; i < chars.length; i += 3) {
      if (isTerminal(t)) {
        send('', true, null);
        return msg;
      }
      send(chars.slice(i, i + 3).join(''), false, null);
      await sleep(28);
    }
    send('', true, sources);
    return msg;
  }

  function addArtifact(t: Task, title: string, path: string, bytes: number): Artifact {
    const art: Artifact = {
      id: id('art'),
      task_id: t.id,
      kind: 'markdown',
      title,
      path,
      bytes,
      content_hash: `sha256:${Math.random().toString(16).slice(2, 6)}…${Math.random().toString(16).slice(2, 6)}`,
      created_at: nowIso(),
    };
    t.artifacts.push(art);
    t.result_ref = art.id;
    emit('artifact_ready', { ...base(t), task_id: t.id, artifact: art });
    return art;
  }

  function createSchedule(
    args: CommandArgs<'create_schedule'>,
  ): Schedule {
    const s: Schedule = {
      id: id('sch'),
      title: args.title,
      next_run_at: args.run_at,
      timezone: args.timezone,
      original_rule: args.original_rule,
      recurrence: args.recurrence,
      enabled: true,
      source_task_id: args.source_task_id,
    };
    state.schedules.push(s);
    commit();
    return s;
  }

  function newTask(intent: string, kind: Task['kind']): Task {
    const created = nowIso();
    const t: Task = {
      id: id('task'),
      kind,
      conversation_id: CONVERSATION_ID,
      intent,
      status: 'queued',
      sequence: 0,
      created_at: created,
      updated_at: created,
      steps: [],
      artifacts: [],
      evidence_refs: [],
      result_ref: null,
      needs_user: null,
      pending_approval: null,
      error: null,
      model: { provider: 'OpenAI 相容服務', model_id: 'sample-chat-1' },
      tainted_by_external: false,
    };
    state.tasks[t.id] = t;
    emit('task_progress', {
      ...base(t),
      task_id: t.id,
      status: 'queued',
      step: null,
      message: null,
      error: null,
    });
    return t;
  }

  /* ---------------- scenarios ---------------- */

  async function runDemo(taskId: string) {
    const t = getTask(taskId);
    const alive = () => !isTerminal(getTask(taskId));
    await sleep(700);
    if (!alive()) return;
    transition(taskId, 'running', { message: '開始處理' });

    let s = beginStep(taskId, '讀取貼上的會議筆記', 'read_input', 'none');
    await sleep(600);
    if (!alive()) return;
    endStep(taskId, s, 'input:pasted-notes');

    s = beginStep(taskId, '整理摘要與待辦', 'summarize', 'none');
    await streamAssistant(t, demoSummary(nextFriday().label), DEMO_SOURCES);
    if (!alive()) return;
    endStep(taskId, s, 'msg:summary');

    s = beginStep(taskId, '在本機建立 4 項待辦', 'create_todos', 'local_write');
    await sleep(500);
    if (!alive()) return;
    const created: Todo[] = [
      ['整理報價單並寄給客戶', nextFriday().ms, false],
      ['升級測試環境', null, true],
      ['設計組交出 48px 與 96px 圖示', null, false],
      ['回覆設計組關於圖示尺寸的問題', nextFriday().ms + 3 * DAY, false],
    ].map(([title, due, uncertain]) => ({
      id: id('todo'),
      title: title as string,
      due_at: due ? isoAt(due as number) : null,
      due_uncertain: uncertain as boolean,
      done: false,
      source_task_id: taskId,
    }));
    state.todos.unshift(...created);
    commit();
    endStep(taskId, s, `todos:${created.length}`);

    // Approval: modifying an existing document needs a diff + consent (handbook 8).
    s = beginStep(taskId, '準備更新「週會待辦.md」', 'write_file', 'local_write');
    const approval: ApprovalRequest = {
      approval_id: id('appr'),
      task_id: taskId,
      action: 'modify_file',
      target: TODO_FILE_PATH,
      preview: {
        kind: 'diff',
        lines: [
          { op: 'context', text: '# 專案 A 週會待辦' },
          { op: 'context', text: '' },
          { op: 'remove', text: '- [ ] 小林：整理報價單' },
          { op: 'add', text: `- [ ] 小林：整理報價單並寄給客戶（${nextFriday().label} 前）` },
          { op: 'add', text: '- [ ] 阿凱：升級測試環境（日期待確認）' },
          { op: 'add', text: '- [ ] 設計組：交出 48px 與 96px 圖示' },
          { op: 'add', text: '- [ ] 我：週一前回覆設計組圖示尺寸問題' },
          { op: 'context', text: '' },
          { op: 'context', text: '## 備註' },
        ],
      },
      consequences: [
        '會覆寫這個檔案的第 3 行，並新增 4 行。',
        '覆寫前會先備份成「週會待辦.md.bak」，可以還原。',
        '不會寄出任何訊息，也不會改動這個資料夾以外的檔案。',
      ],
      arguments_hash: 'sha256:9c2e…41d7',
      expires_at: isoAt(Date.now() + 30 * 60_000),
      once_only_reason: null,
      status: 'pending',
    };
    t.pending_approval = approval;
    t.needs_user = {
      kind: 'approval',
      approval_id: approval.approval_id,
      summary: '要更新「週會待辦.md」，需要你核准。',
    };
    transition(taskId, 'awaiting_user', { message: '等待你核准', step: { ...s } });
    emit('approval_required', { ...base(t), task_id: taskId, approval });

    const decision = await new Promise<'approve_once' | 'deny'>((resolve) => {
      approvalWaiters.set(approval.approval_id, resolve);
    });
    // On deny (or cancel) respond_to_approval / cancel_task already moved the task.
    if (decision === 'deny' || !alive()) return;

    // respond_to_approval already moved the task back to running.
    await sleep(600);
    if (!alive()) return;
    endStep(taskId, s, 'file:週會待辦.md');
    const art = addArtifact(t, '週會待辦.md', TODO_FILE_PATH, 612);

    transition(taskId, 'verifying', { message: '回讀檔案確認內容' });
    const v = beginStep(taskId, '回讀檔案並比對雜湊', 'verify_file', 'none');
    await sleep(700);
    endStep(taskId, v, `hash:${art.content_hash.slice(7, 11)}`);

    transition(taskId, 'running', { message: '建立明早提醒' });
    const r = beginStep(taskId, '建立明早 09:00 的提醒', 'create_schedule', 'local_write');
    await sleep(500);
    if (!alive()) return;
    const sch = createSchedule({
      title: '寄出報價單',
      run_at: isoAt(taipeiTomorrowAt(9)),
      timezone: 'Asia/Taipei',
      original_rule: '明早',
      recurrence: null,
      source_task_id: taskId,
    });
    endStep(taskId, r, `schedule:${sch.id}`);

    transition(taskId, 'verifying', { message: '最後檢查' });
    await sleep(500);
    t.needs_user = null;
    transition(taskId, 'completed', { message: '完成' });
    void followUpAfterCompletion();
  }

  async function followUpAfterCompletion() {
    await sleep(9000);
    if (state.suggestions.some((x) => x.rule_id === 'due_task_split')) return;
    const sug: Suggestion = {
      id: id('sug'),
      rule_id: 'due_task_split',
      title: '「繳交本月差旅報銷」今天要到期',
      body: '要把它拆成幾個小步驟，並排進今天下午嗎？',
      why: '這項待辦的到期日是今天，目前還沒有標示完成。我不會自動更改截止日。',
      data_used: ['助理內的待辦「繳交本月差旅報銷」', '今天的本機行程（2 項）'],
      intent_on_accept: '把「繳交本月差旅報銷」拆成下一步，並排進今天下午',
      created_at: nowIso(),
      outcome: 'pending',
    };
    state.suggestions.push(sug);
    emit('suggestion_ready', { ...globalBase(), task_id: null, suggestion: sug });
  }

  async function runConversation(taskId: string, intent: string) {
    const t = getTask(taskId);
    await sleep(400);
    if (isTerminal(t)) return;
    transition(taskId, 'running');
    const reply = `收到。你說的是「${intent.slice(0, 24)}${intent.length > 24 ? '…' : ''}」。\n\n這是展示用的模擬回覆。把一段**會議筆記**貼進來，我會示範完整流程：摘要、待辦、核准、成果與提醒。`;
    await streamAssistant(t, reply, []);
    if (isTerminal(t)) return;
    transition(taskId, 'verifying');
    t.evidence_refs.push('msg:reply');
    transition(taskId, 'completed');
  }

  async function runSimpleJob(
    taskId: string,
    doneTitle: string,
    opts: { fail?: boolean; resume?: boolean } = {},
  ) {
    const alive = () => !isTerminal(getTask(taskId));
    await sleep(600);
    if (!alive()) return;
    if (!opts.resume) transition(taskId, 'running');
    const s = beginStep(taskId, doneTitle, 'work', 'none');
    await sleep(1800);
    if (!alive()) return;
    if (opts.fail) {
      endStep(taskId, s, null, false);
      const t = getTask(taskId);
      t.error = {
        kind: 'timeout',
        message: '模型 90 秒內沒有回應',
        retryable: true,
        occurred_at: nowIso(),
      };
      transition(taskId, 'failed', { message: '逾時' });
      return;
    }
    endStep(taskId, s, 'result:ok');
    transition(taskId, 'verifying');
    await sleep(400);
    transition(taskId, 'completed');
  }

  function startTask(intent: string, attachments: ChatAttachment[]): Task {
    const looksLikeNotes =
      /會議|筆記|待辦|週會|紀錄/.test(intent) || intent.length > 80 || attachments.length > 0;
    if (intent.includes('#fail')) {
      const t = newTask(intent, 'job');
      void runSimpleJob(t.id, '呼叫外部搜尋', { fail: true }).catch(console.error);
      return t;
    }
    if (looksLikeNotes) {
      const t = newTask(intent, 'job');
      void runDemo(t.id).catch(console.error);
      return t;
    }
    const t = newTask(intent, 'conversation');
    void runConversation(t.id, intent).catch(console.error);
    return t;
  }

  /* ---------------- command handlers ---------------- */

  function deriveStatus(): ObservationStatus {
    const p = state.policy;
    const paused = !!p.paused_until && Date.parse(p.paused_until) > Date.now();
    return {
      observing: p.enabled && !paused,
      paused: p.enabled && paused,
      dnd: p.dnd,
      last_sample_at: state.lastSampleAt,
    };
  }

  const loopback = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/;

  const handlers: Handlers = {
    submit_task: ({ intent, conversation_id, attachments }) => {
      const cid = conversation_id ?? CONVERSATION_ID;
      (state.messages[cid] ??= []).push({
        id: id('msg'),
        conversation_id: cid,
        task_id: null,
        role: 'user',
        content: intent,
        created_at: nowIso(),
        sources: [],
        artifact_ids: [],
        attachments,
        streaming: false,
      });
      const t = startTask(intent, attachments);
      const last = state.messages[cid]?.[state.messages[cid].length - 1];
      if (last) last.task_id = t.id;
      commit();
      return { task_id: t.id, conversation_id: cid };
    },
    cancel_task: ({ task_id }) => {
      const t = getTask(task_id);
      transition(task_id, 'cancelled', { message: '已取消' });
      t.needs_user = null;
      const pendingId = t.pending_approval?.approval_id;
      t.pending_approval = null;
      if (pendingId) {
        approvalWaiters.get(pendingId)?.('deny');
        approvalWaiters.delete(pendingId);
      }
      commit();
      return t;
    },
    retry_task: ({ task_id }) => {
      const old = getTask(task_id);
      if (!['failed', 'cancelled'].includes(old.status)) {
        throw err('invalid_transition', `cannot retry a ${old.status} task`);
      }
      const t = startTask(old.intent, []);
      return { task_id: t.id };
    },
    resume_task: ({ task_id }) => {
      const t = getTask(task_id);
      t.needs_user = null;
      transition(task_id, 'running', { message: '已繼續' });
      void runSimpleJob(task_id, '繼續分析剩餘的提交紀錄', { resume: true }).catch(console.error);
      commit();
      return t;
    },
    resolve_uncertain_task: ({ task_id, outcome }) => {
      const t = getTask(task_id);
      t.needs_user = null;
      if (outcome === 'completed') {
        t.evidence_refs.push('user_confirmed');
        t.error = null;
        transition(task_id, 'completed', { message: '你已確認結果' });
      } else {
        transition(task_id, 'failed', { message: '你確認沒有成功' });
      }
      commit();
      return t;
    },
    get_task: ({ task_id }) => getTask(task_id),
    list_tasks: ({ statuses }) =>
      Object.values(state.tasks)
        .filter((t) => !statuses || statuses.includes(t.status))
        .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
    respond_to_approval: ({ approval_id, decision }) => {
      const t = Object.values(state.tasks).find((x) => x.pending_approval?.approval_id === approval_id);
      if (!t) throw err('not_found', `approval ${approval_id} not found or already answered`);
      const approval = t.pending_approval;
      if (approval) approval.status = decision === 'approve_once' ? 'approved' : 'denied';
      t.pending_approval = null;
      t.needs_user = null;
      if (decision === 'approve_once') {
        transition(t.id, 'running', { message: '已核准，繼續執行' });
      } else {
        t.error = {
          kind: 'cancelled',
          message: '你拒絕了這項操作，任務已取消，沒有修改任何檔案。',
          retryable: true,
          occurred_at: nowIso(),
        };
        transition(t.id, 'cancelled', { message: '你拒絕了操作' });
      }
      approvalWaiters.get(approval_id)?.(decision);
      approvalWaiters.delete(approval_id);
      commit();
      return t;
    },
    get_observation_policy: () => ({ policy: state.policy, status: deriveStatus() }),
    set_observation_policy: ({ policy }) => {
      state.policy = policy;
      const status = deriveStatus();
      commit();
      emit('observation_status', { ...globalBase(), task_id: null, status });
      return { policy, status };
    },
    list_memories: ({ type }) => state.memories.filter((m) => !type || m.type === type),
    correct_memory: ({ id: mid, content }) => {
      const m = state.memories.find((x) => x.id === mid);
      if (!m) throw err('not_found', `memory ${mid} not found`);
      // A correction turns an inference into an explicit preference (handbook 5.2).
      m.content = content;
      m.type = m.type === 'inferred_habit' ? 'explicit_preference' : m.type;
      m.confidence = null;
      m.evidence_note = null;
      m.expires_at = m.type === 'explicit_preference' ? null : m.expires_at;
      m.confirmed_at = nowIso();
      commit();
      return m;
    },
    delete_memory: ({ id: mid }) => {
      const i = state.memories.findIndex((x) => x.id === mid);
      if (i < 0) throw err('not_found', `memory ${mid} not found`);
      state.memories.splice(i, 1);
      commit();
      return { derived_invalidated: 1 };
    },
    create_schedule: (a) => createSchedule(a),
    list_messages: ({ conversation_id }) => state.messages[conversation_id] ?? [],
    get_today: () => ({
      todos: state.todos,
      agenda: state.agenda,
      reminders: state.schedules,
      daily_summary: state.dailySummary,
      suggestions: state.suggestions.filter((s) => s.outcome === 'pending'),
    }),
    update_todo: ({ id: tid, done }) => {
      const todo = state.todos.find((x) => x.id === tid);
      if (!todo) throw err('not_found', `todo ${tid} not found`);
      todo.done = done;
      commit();
      return todo;
    },
    respond_to_suggestion: ({ suggestion_id, response }) => {
      const sug = state.suggestions.find((x) => x.id === suggestion_id);
      if (!sug) throw err('not_found', `suggestion ${suggestion_id} not found`);
      sug.outcome = response === 'accept' ? 'accepted' : response === 'later' ? 'later' : 'never';
      if (response === 'accept') {
        const t = newTask(sug.intent_on_accept, 'job');
        void runSimpleJob(t.id, '拆解待辦並排入今天下午').catch(console.error);
        commit();
        return { task_id: t.id };
      }
      commit();
      return { task_id: null };
    },
    list_model_profiles: () => ({
      profiles: state.profiles,
      routes: state.routes,
      protocols: PROTOCOLS,
    }),
    save_model_profile: ({ profile }) => {
      if (!/^https:\/\//.test(profile.base_url) && !loopback.test(profile.base_url)) {
        throw err('permission_denied', '遠端端點必須使用 HTTPS；HTTP 只允許本機 loopback 位址。');
      }
      const existing = profile.id ? state.profiles.find((p) => p.id === profile.id) : undefined;
      if (existing && existing.base_url !== profile.base_url && !profile.credential) {
        throw err('authorization_required', '修改端點後必須重新輸入金鑰，確認金鑰綁定的對象。');
      }
      if (existing) {
        const changed =
          existing.base_url !== profile.base_url ||
          existing.model_id !== profile.model_id ||
          existing.protocol !== profile.protocol;
        Object.assign(existing, {
          display_name: profile.display_name,
          provider: profile.provider,
          protocol: profile.protocol,
          base_url: profile.base_url,
          model_id: profile.model_id,
          has_credential: profile.credential ? true : existing.has_credential,
          config_version: existing.config_version + 1,
        });
        if (changed) {
          existing.capabilities = DEFAULT_CAPS;
          existing.verified_at = null;
        }
        commit();
        return existing;
      }
      const created: ModelProfile = {
        id: id('prof'),
        display_name: profile.display_name,
        provider: profile.provider,
        protocol: profile.protocol,
        base_url: profile.base_url,
        model_id: profile.model_id,
        has_credential: !!profile.credential,
        capabilities: DEFAULT_CAPS,
        verified_at: null,
        config_version: 1,
      };
      state.profiles.push(created);
      commit();
      return created;
    },
    delete_model_profile: ({ id: pid }) => {
      state.profiles = state.profiles.filter((p) => p.id !== pid);
      for (const r of state.routes) {
        if (r.primary_profile_id === pid) r.primary_profile_id = null;
        r.fallback_profile_ids = r.fallback_profile_ids.filter((x) => x !== pid);
      }
      state.policy.cloud_scope.allowed_profile_ids = state.policy.cloud_scope.allowed_profile_ids.filter(
        (x) => x !== pid,
      );
      commit();
      return {};
    },
    test_model_profile: async ({ profile_id }) => {
      const p = state.profiles.find((x) => x.id === profile_id);
      if (!p) throw err('not_found', `profile ${profile_id} not found`);
      const available = PROTOCOLS.find((x) => x.protocol === p.protocol)?.available ?? false;
      const steps: TestStepResult[] = [];
      const add = async (
        step: TestStepResult['step'],
        ok: boolean,
        ms: number,
        error: TestStepResult['error'] = null,
      ) => {
        await sleep(350);
        steps.push({ step, ok, latency_ms: ok ? ms : null, error });
        return ok;
      };
      let ok = await add('connect', available, 120, {
        kind: 'external_failure',
        message: '這個協定的適配器尚未通過整合測試',
        retryable: false,
      });
      if (ok) {
        ok = await add('auth', p.has_credential, 210, {
          kind: 'permission_denied',
          message: '尚未設定金鑰，端點回應 401',
          retryable: false,
        });
      }
      if (ok) {
        ok = await add('model', true, 180);
        await add('stream', true, 640);
        await add('tools', true, 920);
        await add('structured', p.protocol === 'openai_chat_compatible', 880, {
          kind: 'external_failure',
          message: '回應不符合 JSON schema，視為不支援',
          retryable: false,
        });
        await add('image', false, 0, {
          kind: 'external_failure',
          message: '模型回報不接受圖片輸入',
          retryable: false,
        });
      }
      const capStates: CapabilityResults['states'] = ok
        ? {
            text: 'supported',
            streaming: 'supported',
            tool_calling: 'supported',
            structured_output: p.protocol === 'openai_chat_compatible' ? 'supported' : 'unsupported',
            image_input: 'unsupported',
          }
        : p.capabilities.states;
      const capabilities: CapabilityResults = ok
        ? { states: capStates, context_limit: 128000 }
        : p.capabilities;
      if (ok) {
        p.capabilities = capabilities;
        p.verified_at = nowIso();
      }
      commit();
      const result: ConnectionTestResult = {
        profile_id,
        ok,
        steps,
        capabilities,
        tested_at: nowIso(),
      };
      return result;
    },
    set_model_routes: ({ routes }) => {
      state.routes = routes;
      commit();
      return routes;
    },
    get_appearance: () => readStoredAppearance(),
    set_appearance: (a) => {
      // Only appearance is written here: no task, model, memory or notification state is touched.
      const next: Appearance = { skin_id: String(a.skin_id), theme: a.theme };
      if (!next.skin_id || !['light', 'dark', 'system'].includes(next.theme)) {
        throw err('permission_denied', 'invalid appearance');
      }
      try {
        localStorage.setItem(APPEARANCE_KEY, JSON.stringify(next));
      } catch {
        /* storage unavailable: still propagate to open windows */
      }
      emit('appearance_changed', { schema_version: SCHEMA_VERSION, appearance: next });
      return next;
    },
    set_floating_expanded: ({ expanded }) => {
      // browser/mock: there is no native window to resize; just record the call order for tests
      debug.expandedLog.push({ expanded, wheelInDom: !!document.querySelector('.wheel') });
      const edge = expanded ? FLOATING_EXPANDED_PX : FLOATING_COLLAPSED_PX;
      return { width: edge, height: edge, sprite_offset_x: 0, sprite_offset_y: 0 };
    },
    open_window: ({ label, tab }) => {
      const file = label === 'panel' ? 'panel.html' : 'settings.html';
      const q = tab ? `?tab=${encodeURIComponent(tab)}` : '';
      const size = label === 'panel' ? 'width=440,height=720' : 'width=760,height=760';
      window.open(`./${file}${q}`, `elf-${label}`, size);
      return {};
    },
  };

  const debug: MockDebug = {
    dropNextEvents(n) {
      dropCount = n;
    },
    speed: SPEED,
    expandedLog: [],
  };
  (window as unknown as { __lumaMock: MockDebug }).__lumaMock = debug;

  return {
    kind: 'mock',
    async invoke<K extends CommandName>(cmd: K, args: CommandArgs<K>): Promise<CommandResult<K>> {
      owner = true;
      await sleep(60);
      const fn = handlers[cmd] as (a: CommandArgs<K>) => Promise<CommandResult<K>> | CommandResult<K>;
      // Structured clone so callers can never mutate mock state through a returned reference.
      return structuredClone(await fn(args));
    },
    listen<K extends EventName>(event: K, handler: (p: EventMap[K]) => void): Unsubscribe {
      let set = listeners.get(event);
      if (!set) listeners.set(event, (set = new Set()));
      const fn = handler as (p: never) => void;
      set.add(fn);
      return () => set.delete(fn);
    },
  };
}

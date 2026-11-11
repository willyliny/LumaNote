/**
 * ALL IPC wire types live in this file (handbook section 12).
 * They will be replaced by types generated from Rust; nothing else may redeclare them.
 *
 * Conventions
 *  - Field names are snake_case so they match serde defaults on the Rust side.
 *  - Times are ISO-8601 UTC strings (Iso). Schedules also carry an IANA timezone.
 *  - Items marked "NOT IN HANDBOOK" were needed by the UI and are proposals
 *    for the tech lead to freeze or reject (see the F1 report).
 */

export const SCHEMA_VERSION = 1 as const;
export type Iso = string;

/* ------------------------------------------------------------------ */
/* Task states (handbook 7.5)                                          */
/* ------------------------------------------------------------------ */

export type TaskStatus =
  | 'queued'
  | 'running'
  | 'awaiting_user'
  | 'paused'
  | 'verifying'
  | 'uncertain'
  | 'completed'
  | 'failed'
  | 'cancelled';

export const TERMINAL_STATUSES: readonly TaskStatus[] = ['completed', 'failed', 'cancelled'];

/** Legal transitions, copied from the 7.5 table. Used by the mock to enforce the state machine. */
export const TASK_TRANSITIONS: Record<TaskStatus, readonly TaskStatus[]> = {
  queued: ['running', 'cancelled'],
  running: ['awaiting_user', 'paused', 'verifying', 'uncertain', 'failed', 'cancelled'],
  awaiting_user: ['running', 'cancelled'],
  paused: ['running', 'cancelled'],
  verifying: ['completed', 'running', 'failed'],
  uncertain: ['completed', 'failed', 'cancelled'],
  completed: [],
  failed: [],
  cancelled: [],
};

/* ------------------------------------------------------------------ */
/* Errors (handbook 12: no permission / needs authorization / external */
/* failure / timeout / cancelled / outcome uncertain)                  */
/* ------------------------------------------------------------------ */

export type IpcErrorKind =
  | 'permission_denied'
  | 'authorization_required'
  | 'external_failure'
  | 'timeout'
  | 'cancelled'
  | 'outcome_uncertain'
  // NOT IN HANDBOOK: needed so a missing id or illegal transition is not shown as "AI error".
  | 'not_found'
  | 'invalid_transition';

export interface IpcErrorPayload {
  kind: IpcErrorKind;
  /** Machine-oriented detail, never shown verbatim as the headline. */
  message: string;
  retryable: boolean;
}

/** Thrown by `ipc.invoke` when a command fails. */
export class IpcError extends Error {
  readonly kind: IpcErrorKind;
  readonly retryable: boolean;
  constructor(payload: IpcErrorPayload) {
    super(payload.message);
    this.name = 'IpcError';
    this.kind = payload.kind;
    this.retryable = payload.retryable;
  }
}

/* ------------------------------------------------------------------ */
/* Core entities                                                       */
/* ------------------------------------------------------------------ */

export type SideEffect = 'none' | 'local_write' | 'external_write';
/** handbook 8: trust tag on tool results */
export type Trust = 'user_provided' | 'local_granted' | 'external';

export interface TaskStep {
  seq: number;
  title: string;
  tool: string | null;
  side_effect: SideEffect;
  status: 'started' | 'succeeded' | 'failed';
  evidence_ref: string | null;
}

export type ArtifactKind = 'markdown' | 'text' | 'html' | 'file';

export interface Artifact {
  id: string;
  task_id: string;
  kind: ArtifactKind;
  title: string;
  path: string;
  bytes: number;
  content_hash: string;
  created_at: Iso;
}

export interface Source {
  id: string;
  title: string;
  /** http(s) URL for web sources */
  url: string | null;
  /** local path for granted-folder sources */
  path: string | null;
  /** short excerpt that supports the claim */
  quote: string | null;
}

export type ApprovalAction =
  | 'modify_file'
  | 'send_message'
  | 'external_calendar_invite'
  | 'publish'
  | 'payment'
  | 'delete';

export interface DiffLine {
  op: 'context' | 'add' | 'remove';
  text: string;
}

export type ApprovalPreview =
  | { kind: 'text'; text: string }
  | { kind: 'diff'; lines: DiffLine[] };

export interface ApprovalRequest {
  approval_id: string;
  task_id: string;
  action: ApprovalAction;
  /** file path, recipient, or resource the action touches */
  target: string;
  preview: ApprovalPreview;
  /** consequence statements, produced by the policy engine (zh-TW) */
  consequences: string[];
  arguments_hash: string;
  expires_at: Iso | null;
  /** handbook 8: task read external content, so only per-use approval is offered */
  once_only_reason: 'external_content' | null;
  status: 'pending' | 'approved' | 'denied' | 'expired';
}

export type NeedsUser =
  | { kind: 'approval'; approval_id: string; summary: string }
  | { kind: 'resume'; reason: 'tool_limit' | 'time_limit' | 'budget' | 'offline'; summary: string }
  | { kind: 'confirm_outcome'; summary: string };

export interface TaskErrorInfo extends IpcErrorPayload {
  occurred_at: Iso;
}

export interface Task {
  id: string;
  /**
   * NOT IN HANDBOOK: handbook 1 says plain Q&A needs no background task card, yet submit_task
   * always returns an id. 'conversation' tasks are hidden from the task center.
   */
  kind: 'conversation' | 'job';
  conversation_id: string;
  intent: string;
  status: TaskStatus;
  /** last task_events.sequence folded into this snapshot */
  sequence: number;
  created_at: Iso;
  updated_at: Iso;
  steps: TaskStep[];
  artifacts: Artifact[];
  /** evidence refs; completed requires at least one (handbook 7.5) */
  evidence_refs: string[];
  result_ref: string | null;
  needs_user: NeedsUser | null;
  pending_approval: ApprovalRequest | null;
  error: TaskErrorInfo | null;
  model: { provider: string; model_id: string } | null;
  /** true once the task read external content (handbook 8) */
  tainted_by_external: boolean;
}

export interface ChatAttachment {
  name: string;
  bytes: number;
  /** absolute path when dropped inside Tauri; null in a plain browser */
  path: string | null;
}

export interface ChatMessage {
  id: string;
  conversation_id: string;
  task_id: string | null;
  role: 'user' | 'assistant';
  content: string;
  created_at: Iso;
  sources: Source[];
  artifact_ids: string[];
  attachments: ChatAttachment[];
  streaming: boolean;
}

/* ------------------------------------------------------------------ */
/* Today / schedule / suggestions                                      */
/* ------------------------------------------------------------------ */

export interface Todo {
  id: string;
  title: string;
  due_at: Iso | null;
  /** handbook 2: uncertain dates are marked as needing confirmation */
  due_uncertain: boolean;
  done: boolean;
  source_task_id: string | null;
}

export interface AgendaItem {
  id: string;
  title: string;
  start_at: Iso;
  end_at: Iso;
  source: 'local';
}

export interface Schedule {
  id: string;
  title: string;
  /** UTC instant, IANA zone and the user's original rule (handbook 11) */
  next_run_at: Iso;
  timezone: string;
  original_rule: string;
  recurrence: string | null;
  enabled: boolean;
  source_task_id: string | null;
}

export type SuggestionOutcome = 'pending' | 'accepted' | 'later' | 'never' | 'unanswered';

export interface Suggestion {
  id: string;
  rule_id: string;
  title: string;
  body: string;
  /** card field: 為什麼出現 */
  why: string;
  /** card field: 使用了哪些資料 */
  data_used: string[];
  intent_on_accept: string;
  created_at: Iso;
  outcome: SuggestionOutcome;
}

export interface DailySummary {
  date: string;
  body: string;
  source_ids: string[];
}

export interface TodaySnapshot {
  todos: Todo[];
  agenda: AgendaItem[];
  reminders: Schedule[];
  daily_summary: DailySummary | null;
  suggestions: Suggestion[];
}

/* ------------------------------------------------------------------ */
/* Observation / privacy                                               */
/* ------------------------------------------------------------------ */

export type ObservationSourceId = 'foreground_app' | 'idle_state' | 'window_title' | 'project_path';

export interface ObservationSource {
  id: ObservationSourceId;
  enabled: boolean;
}

export interface Exclusion {
  id: string;
  kind: 'app' | 'folder' | 'title_pattern';
  pattern: string;
}

export interface CloudScope {
  /** collection, storage and cloud sending are three separate settings (handbook 10) */
  send_daily_summary: boolean;
  send_window_titles: boolean;
  /** provider ids that may receive data; new providers are NOT inherited */
  allowed_profile_ids: string[];
}

export interface ObservationPolicy {
  enabled: boolean;
  /** pause observation until this time; null = not paused */
  paused_until: Iso | null;
  dnd: boolean;
  sources: ObservationSource[];
  exclusions: Exclusion[];
  cloud_scope: CloudScope;
}

export interface ObservationStatus {
  observing: boolean;
  paused: boolean;
  dnd: boolean;
  last_sample_at: Iso | null;
}

/* ------------------------------------------------------------------ */
/* Memory                                                              */
/* ------------------------------------------------------------------ */

export type MemoryType = 'explicit_preference' | 'task_fact' | 'short_term' | 'inferred_habit';

export interface MemorySourceRef {
  kind: 'task' | 'user_statement' | 'activity_summary';
  id: string;
  label: string;
}

export interface Memory {
  id: string;
  type: MemoryType;
  content: string;
  /** inferred habits only */
  confidence: number | null;
  sources: MemorySourceRef[];
  /** e.g. "10 個有資料的工作日中有 7 天" */
  evidence_note: string | null;
  confirmed_at: Iso | null;
  expires_at: Iso | null;
  created_at: Iso;
}

/* ------------------------------------------------------------------ */
/* Model profiles                                                      */
/* ------------------------------------------------------------------ */

export type ProtocolType =
  | 'openai_responses'
  | 'openai_chat_compatible'
  | 'anthropic_messages'
  | 'gemini_generate'
  | 'local_endpoint';

export interface ProtocolInfo {
  protocol: ProtocolType;
  /** adapter passed integration tests; otherwise shown as unavailable (handbook 15) */
  available: boolean;
}

export type CapabilityName =
  | 'text'
  | 'streaming'
  | 'tool_calling'
  | 'structured_output'
  | 'image_input';
export type CapabilityState = 'supported' | 'unsupported' | 'unknown';

export interface CapabilityResults {
  states: Record<CapabilityName, CapabilityState>;
  context_limit: number | null;
}

export interface ModelProfile {
  id: string;
  display_name: string;
  provider: string;
  protocol: ProtocolType;
  base_url: string;
  model_id: string;
  /** The credential itself is never returned (write-only). */
  has_credential: boolean;
  capabilities: CapabilityResults;
  verified_at: Iso | null;
  config_version: number;
}

export interface ModelProfileInput {
  id: string | null;
  display_name: string;
  provider: string;
  protocol: ProtocolType;
  base_url: string;
  model_id: string;
  /** write-only; null = keep the stored credential */
  credential: string | null;
}

export type ModelPurpose = 'chat' | 'background_summary' | 'research' | 'coding' | 'vision';

export interface ModelRoute {
  purpose: ModelPurpose;
  primary_profile_id: string | null;
  fallback_profile_ids: string[];
}

export interface ModelProfileList {
  profiles: ModelProfile[];
  routes: ModelRoute[];
  protocols: ProtocolInfo[];
}

export type TestStepName = 'connect' | 'auth' | 'model' | 'stream' | 'tools' | 'structured' | 'image';

export interface TestStepResult {
  step: TestStepName;
  ok: boolean;
  latency_ms: number | null;
  error: IpcErrorPayload | null;
}

export interface ConnectionTestResult {
  profile_id: string;
  ok: boolean;
  steps: TestStepResult[];
  capabilities: CapabilityResults;
  tested_at: Iso;
}

/* ------------------------------------------------------------------ */
/* Commands (handbook 12 + marked additions)                           */
/* ------------------------------------------------------------------ */

export interface CommandMap {
  submit_task: {
    args: { intent: string; conversation_id: string | null; attachments: ChatAttachment[] };
    result: { task_id: string; conversation_id: string };
  };
  cancel_task: { args: { task_id: string }; result: Task };
  /** NOT IN HANDBOOK: retry a failed/cancelled task as a new task linked to the old one */
  retry_task: { args: { task_id: string }; result: { task_id: string } };
  /** NOT IN HANDBOOK: paused -> running (7.5 allows it, no command existed) */
  resume_task: { args: { task_id: string }; result: Task };
  /**
   * NOT IN HANDBOOK: uncertain -> completed|failed after the user checked the outside world.
   * The user's confirmation is recorded as the evidence_ref required for completed.
   */
  resolve_uncertain_task: {
    args: { task_id: string; outcome: 'completed' | 'failed' };
    result: Task;
  };
  get_task: { args: { task_id: string }; result: Task };
  list_tasks: { args: { statuses: TaskStatus[] | null }; result: Task[] };
  respond_to_approval: {
    args: { approval_id: string; decision: 'approve_once' | 'deny' };
    result: Task;
  };
  set_observation_policy: {
    args: { policy: ObservationPolicy };
    result: { policy: ObservationPolicy; status: ObservationStatus };
  };
  /** NOT IN HANDBOOK */
  get_observation_policy: {
    args: Record<string, never>;
    result: { policy: ObservationPolicy; status: ObservationStatus };
  };
  list_memories: { args: { type: MemoryType | null }; result: Memory[] };
  correct_memory: { args: { id: string; content: string }; result: Memory };
  /** derived_invalidated: how many summaries/habits were invalidated along source_ids (handbook 9) */
  delete_memory: { args: { id: string }; result: { derived_invalidated: number } };
  create_schedule: {
    args: {
      title: string;
      run_at: Iso;
      timezone: string;
      original_rule: string;
      recurrence: string | null;
      source_task_id: string | null;
    };
    result: Schedule;
  };
  /** NOT IN HANDBOOK */
  list_messages: { args: { conversation_id: string }; result: ChatMessage[] };
  /** NOT IN HANDBOOK */
  get_today: { args: Record<string, never>; result: TodaySnapshot };
  /** NOT IN HANDBOOK */
  update_todo: { args: { id: string; done: boolean }; result: Todo };
  /** NOT IN HANDBOOK: accept creates a task and returns its id */
  respond_to_suggestion: {
    args: { suggestion_id: string; response: 'accept' | 'later' | 'never' };
    result: { task_id: string | null };
  };
  /** NOT IN HANDBOOK: model settings page */
  list_model_profiles: { args: Record<string, never>; result: ModelProfileList };
  save_model_profile: { args: { profile: ModelProfileInput }; result: ModelProfile };
  delete_model_profile: { args: { id: string }; result: Record<string, never> };
  test_model_profile: { args: { profile_id: string }; result: ConnectionTestResult };
  set_model_routes: { args: { routes: ModelRoute[] }; result: ModelRoute[] };
  /**
   * NEW (F2a, not in handbook): appearance settings, persisted by the backend. `skin_id` is an opaque
   * string to the backend; the UI decides whether it is a known, complete skin and falls back to the
   * default skin (with a visible notice) when it is not. Never touches model/task/memory/notification data.
   */
  get_appearance: { args: Record<string, never>; result: Appearance };
  /** NEW (F2a): replaces the stored appearance and emits `appearance_changed` to every window. */
  set_appearance: { args: Appearance; result: Appearance };
  /**
   * NEW (F2a, shell implements it in Rust): ask the shell to enlarge the floating window around its
   * centre (collapsed 96x96 -> expanded 280x280 for the radial menu) or restore it. Wait for it to resolve before
   * showing the wheel; keep the sprite at window centre + the returned offset. The UI calls it
   * with true BEFORE showing the wheel and with false after the close animation. No-op in the mock.
   */
  set_floating_expanded: {
    args: { expanded: boolean };
    /**
     * logical px. When the expanded window is clamped at a screen edge the shell shifts it;
     * sprite_offset_x/y is the sprite centre's offset from the window centre (0 on collapse).
     */
    result: { width: number; height: number; sprite_offset_x: number; sprite_offset_y: number };
  };
  /** NOT IN HANDBOOK: floating entry may only open the panel (handbook 10); done via backend */
  open_window: {
    args: { label: 'panel' | 'settings'; tab: string | null };
    result: Record<string, never>;
  };
}

/** NEW (F2a) */
export type ThemeChoice = 'light' | 'dark' | 'system';
export interface Appearance {
  skin_id: string;
  theme: ThemeChoice;
}

export type CommandName = keyof CommandMap;
export type CommandArgs<K extends CommandName> = CommandMap[K]['args'];
export type CommandResult<K extends CommandName> = CommandMap[K]['result'];

/* ------------------------------------------------------------------ */
/* Events (handbook 12): all carry task_id, sequence, schema_version   */
/* ------------------------------------------------------------------ */

export interface EventBase {
  schema_version: number;
  /**
   * Task events: task_events.sequence (monotonic per task).
   * Non-task events (task_id === null): a global counter. NOT IN HANDBOOK, proposal.
   */
  sequence: number;
  task_id: string | null;
}

export interface TaskProgressEvent extends EventBase {
  task_id: string;
  status: TaskStatus;
  step: TaskStep | null;
  message: string | null;
  error: TaskErrorInfo | null;
}

export interface AssistantDeltaEvent extends EventBase {
  task_id: string;
  conversation_id: string;
  message_id: string;
  delta: string;
  done: boolean;
  /** sent with the final delta */
  sources: Source[] | null;
}

export interface ApprovalRequiredEvent extends EventBase {
  task_id: string;
  approval: ApprovalRequest;
}

export interface ArtifactReadyEvent extends EventBase {
  task_id: string;
  artifact: Artifact;
}

export interface SuggestionReadyEvent extends EventBase {
  task_id: null;
  suggestion: Suggestion;
}

export interface ObservationStatusEvent extends EventBase {
  task_id: null;
  status: ObservationStatus;
}

/**
 * NEW (F2a): fired to every window after `set_appearance`. Deliberately NOT an EventBase: it carries
 * no task_id/sequence, so it cannot disturb the global sequence used for gap detection.
 */
export interface AppearanceChangedEvent {
  schema_version: number;
  appearance: Appearance;
}

export interface EventMap {
  appearance_changed: AppearanceChangedEvent;
  task_progress: TaskProgressEvent;
  assistant_delta: AssistantDeltaEvent;
  approval_required: ApprovalRequiredEvent;
  artifact_ready: ArtifactReadyEvent;
  suggestion_ready: SuggestionReadyEvent;
  observation_status: ObservationStatusEvent;
}

export type EventName = keyof EventMap;
export const EVENT_NAMES: readonly EventName[] = [
  'appearance_changed',
  'task_progress',
  'assistant_delta',
  'approval_required',
  'artifact_ready',
  'suggestion_ready',
  'observation_status',
];

export type Unsubscribe = () => void;

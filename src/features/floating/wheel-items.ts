/**
 * The radial menu's content, as data. One module decides WHAT appears (ids, order), WHAT each item
 * shows (value selector over existing store data) and WHAT it does (action). A later settings page can
 * replace WHEEL_LAYOUT (any 6 ids from WHEEL_ITEMS) without touching the component.
 */
import type { IconName } from '../../components/Icon';
import { t } from '../../lib/strings';
import type { ObservationPolicy, Task, TodaySnapshot } from '../../lib/ipc-types';

export type WheelItemId = 'chat' | 'tasks' | 'today' | 'reminders' | 'settings' | 'dnd';

/** What the selectors may read: already-synced store data, nothing fetched here. */
export interface WheelData {
  tasks: Task[];
  today: TodaySnapshot | null;
  dnd: boolean | null; // null until the policy is loaded
  now: number;
}

export type WheelAction =
  | { kind: 'open'; window: 'panel' | 'settings'; tab: string | null }
  | { kind: 'toggle_dnd' };

export interface WheelValue {
  text: string;
  /** replaces the item icon, so a state is never carried by colour alone */
  icon?: IconName;
  /** drawn with the "needs you" treatment (border + weight), always together with text */
  attention?: boolean;
  /** for toggles */
  pressed?: boolean;
}

export interface WheelItem {
  id: WheelItemId;
  label: string;
  icon: IconName;
  value: (d: WheelData) => WheelValue;
  action: WheelAction;
}

const sameLocalDay = (a: number, b: number) => new Date(a).toDateString() === new Date(b).toDateString();

function nextReminder(d: WheelData): string | null {
  const next = (d.today?.reminders ?? [])
    .filter((r) => r.enabled && Date.parse(r.next_run_at) > d.now)
    .sort((a, b) => Date.parse(a.next_run_at) - Date.parse(b.next_run_at))[0];
  if (!next) return null;
  const ms = Date.parse(next.next_run_at);
  const hhmm = new Intl.DateTimeFormat('en-GB', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
    timeZone: next.timezone,
  }).format(ms);
  if (sameLocalDay(ms, d.now)) return hhmm;
  if (sameLocalDay(ms, d.now + 86400_000)) return `明 ${hhmm}`;
  const dt = new Date(ms);
  return `${dt.getMonth() + 1}/${dt.getDate()}`;
}

export const WHEEL_ITEMS: Record<WheelItemId, WheelItem> = {
  chat: {
    id: 'chat',
    label: t.wheel.chat,
    icon: 'chat',
    value: () => ({ text: t.wheel.chatValue }),
    action: { kind: 'open', window: 'panel', tab: 'chat' },
  },
  tasks: {
    id: 'tasks',
    label: t.wheel.tasks,
    icon: 'tasks',
    value: (d) => {
      const jobs = d.tasks.filter((x) => x.kind === 'job');
      const waiting = jobs.filter((x) => x.status === 'awaiting_user' || x.status === 'uncertain').length;
      if (waiting > 0) return { text: t.wheel.tasksWaiting(waiting), icon: 'alert', attention: true };
      const running = jobs.filter((x) => x.status === 'running' || x.status === 'verifying').length;
      if (running > 0) return { text: t.wheel.tasksRunning(running), icon: 'spin' };
      return { text: t.wheel.tasksNone };
    },
    action: { kind: 'open', window: 'panel', tab: 'tasks' },
  },
  today: {
    id: 'today',
    label: t.wheel.today,
    icon: 'today',
    value: (d) => {
      const n = (d.today?.todos ?? []).filter((x) => !x.done && x.due_at && sameLocalDay(Date.parse(x.due_at), d.now)).length;
      return { text: n > 0 ? t.wheel.todayCount(n) : t.wheel.todayNone };
    },
    action: { kind: 'open', window: 'panel', tab: 'today' },
  },
  reminders: {
    id: 'reminders',
    label: t.wheel.reminders,
    icon: 'clock',
    value: (d) => ({ text: nextReminder(d) ?? t.wheel.reminderNone }),
    action: { kind: 'open', window: 'panel', tab: 'today' },
  },
  settings: {
    id: 'settings',
    label: t.wheel.settings,
    icon: 'gear',
    value: () => ({ text: t.wheel.settingsValue }),
    action: { kind: 'open', window: 'settings', tab: null },
  },
  dnd: {
    id: 'dnd',
    label: t.wheel.dnd,
    icon: 'moon',
    value: (d) => ({
      text: d.dnd ? t.wheel.on : t.wheel.off,
      icon: d.dnd ? 'slash' : 'moon',
      pressed: !!d.dnd,
    }),
    action: { kind: 'toggle_dnd' },
  },
};

/** Clockwise from the top. Six slots: the wheel geometry is built for exactly six. */
export const WHEEL_LAYOUT: readonly WheelItemId[] = ['chat', 'tasks', 'today', 'reminders', 'settings', 'dnd'];

export function dndPolicy(policy: ObservationPolicy): ObservationPolicy {
  return { ...policy, dnd: !policy.dnd };
}

const WD = ['日', '一', '二', '三', '四', '五', '六'];

function parts(ms: number, tz?: string) {
  const f = new Intl.DateTimeFormat('en-CA', {
    timeZone: tz,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
    weekday: 'short',
  });
  const o: Record<string, string> = {};
  for (const p of f.formatToParts(ms)) o[p.type] = p.value;
  const hour = o.hour === '24' ? '00' : o.hour;
  return { y: o.year, m: o.month, d: o.day, hh: hour, mm: o.minute, wd: o.weekday };
}
const ymd = (p: ReturnType<typeof parts>) => `${p.y}-${p.m}-${p.d}`;

/** "今天 17:00" / "明天 09:00" / "11/14（五）17:00" */
export function formatWhen(iso: string, tz?: string, now = Date.now()): string {
  const ms = Date.parse(iso);
  const p = parts(ms, tz);
  const today = ymd(parts(now, tz));
  const tomorrow = ymd(parts(now + 86400_000, tz));
  const yesterday = ymd(parts(now - 86400_000, tz));
  const time = `${p.hh}:${p.mm}`;
  const day = ymd(p);
  if (day === today) return `今天 ${time}`;
  if (day === tomorrow) return `明天 ${time}`;
  if (day === yesterday) return `昨天 ${time}`;
  const wd = WD[new Date(`${day}T12:00:00Z`).getUTCDay()] ?? '';
  return `${Number(p.m)}/${Number(p.d)}（${wd}）${time}`;
}

export function formatTime(iso: string, tz?: string): string {
  const p = parts(Date.parse(iso), tz);
  return `${p.hh}:${p.mm}`;
}

export function formatRelative(iso: string, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - Date.parse(iso)) / 1000));
  if (s < 45) return '剛剛';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} 分鐘前`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} 小時前`;
  return `${Math.round(h / 24)} 天前`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function daysUntil(iso: string, now = Date.now()): number {
  return Math.ceil((Date.parse(iso) - now) / 86400_000);
}

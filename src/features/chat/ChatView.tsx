import { useEffect, useLayoutEffect, useRef, useState, type DragEvent, type KeyboardEvent } from 'react';
import type { ChatAttachment, ChatMessage, Task } from '../../lib/ipc-types';
import { formatBytes } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Markdown } from '../../components/Markdown';
import { Sprite } from '../../components/Sprite';
import { actions, setListening, useApp } from '../../state/app-store';
import { SourceList } from './SourceList';
import { TaskStrip } from './TaskStrip';
import './chat.css';

const ACTIVE = new Set(['queued', 'running', 'awaiting_user', 'paused', 'verifying']);

export function ChatView({ onOpenTasks }: { onOpenTasks: () => void }) {
  const messages = useApp((s) => s.messages);
  const tasks = useApp((s) => s.tasks);
  const [draft, setDraft] = useState('');
  const [files, setFiles] = useState<ChatAttachment[]>([]);
  const [dragging, setDragging] = useState(false);
  const [sending, setSending] = useState(false);
  const [stopping, setStopping] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const area = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);

  const streaming = messages.some((m) => m.streaming);
  const activeTask = [...messages]
    .reverse()
    .map((m) => (m.task_id ? tasks[m.task_id] : undefined))
    .find((x): x is Task => !!x && ACTIVE.has(x.status) && x.status !== 'awaiting_user');
  const canStop = streaming || !!activeTask;

  const lastLen = messages[messages.length - 1]?.content.length ?? 0;
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages.length, lastLen, tasks]);

  const approvalId = Object.values(tasks).find((x) => x.pending_approval)?.pending_approval?.approval_id;
  useEffect(() => {
    if (!approvalId) return;
    stick.current = false;
    scroller.current?.querySelector('.approval')?.scrollIntoView({ block: 'start' });
  }, [approvalId]);

  useEffect(() => {
    const el = area.current;
    if (!el) return;
    // Auto-grow: border-box height = content + borders, otherwise a 1-line box is 2px short and shows a
    // scrollbar. The scrollbar only appears once the cap (about 6 lines) is exceeded.
    const cs = getComputedStyle(el);
    const border = parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
    const cap = parseFloat(cs.maxHeight) || 160;
    el.style.height = 'auto';
    el.style.overflowY = 'hidden';
    const want = el.scrollHeight + border;
    el.style.height = `${Math.min(want, cap)}px`;
    el.style.overflowY = want > cap ? 'auto' : 'hidden';
  }, [draft]);

  const submit = async () => {
    const text = draft.trim();
    if ((!text && files.length === 0) || sending) return;
    setSending(true);
    stick.current = true;
    const r = await actions.submit(text || files.map((f) => f.name).join('、'), files);
    setSending(false);
    if (r) {
      setDraft('');
      setFiles([]);
    }
  };

  const stop = async () => {
    const target = activeTask ?? [...messages].reverse().map((m) => (m.task_id ? tasks[m.task_id] : undefined)).find(Boolean);
    if (!target) return;
    setStopping(true);
    await actions.cancel(target.id);
    setStopping(false);
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter sends; Shift+Enter breaks the line; never send while an IME is composing.
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    }
  };

  const addFiles = (list: FileList | null) => {
    if (!list) return;
    const next: ChatAttachment[] = [];
    for (const f of Array.from(list)) {
      const path = (f as File & { path?: string }).path;
      next.push({ name: f.name, bytes: f.size, path: path ?? null });
    }
    setFiles((cur) => [...cur, ...next]);
  };
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    addFiles(e.dataTransfer.files);
  };

  // A message is "last of its task" when no later message shares the task id.
  const lastOfTask = new Set<string>();
  const seen = new Set<string>();
  for (let i = messages.length - 1; i >= 0; i--) {
    const id = messages[i]!.task_id;
    if (id && !seen.has(id)) {
      seen.add(id);
      lastOfTask.add(messages[i]!.id);
    }
  }

  return (
    <div
      className={`chat ${dragging ? 'chat--drag' : ''}`}
      onDragEnter={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragOver={(e) => e.preventDefault()}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragging(false);
      }}
      onDrop={onDrop}
    >
      <div
        className="chat__scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
        }}
        role="log"
        aria-live="polite"
        aria-busy={streaming}
      >
        {messages.length === 0 ? (
          <div className="chat__empty">
            <Sprite state="idle" size={96} title="" />
            <h2>{t.chat.emptyTitle}</h2>
            <p>{t.chat.emptyBody}</p>
            <button
              className="btn"
              onClick={() => {
                setDraft(t.chat.sampleNotes);
                area.current?.focus();
              }}
            >
              {t.chat.useSample}
            </button>
          </div>
        ) : (
          messages.map((m) => (
            <MessageBlock
              key={m.id}
              m={m}
              task={m.task_id ? tasks[m.task_id] : undefined}
              showStrip={lastOfTask.has(m.id)}
              onOpenTasks={onOpenTasks}
            />
          ))
        )}
      </div>

      {dragging && <div className="chat__drop">{t.chat.dropHere}</div>}

      <form
        className="composer"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        {files.length > 0 && (
          <ul className="chips">
            {files.map((f, i) => (
              <li key={`${f.name}-${i}`} className="chip">
                <Icon name="file" />
                <span className="chip__name">{f.name}</span>
                <span className="subtle">{formatBytes(f.bytes)}</span>
                <button
                  type="button"
                  className="btn btn--quiet btn--icon btn--small"
                  aria-label={t.chat.removeAttachment(f.name)}
                  onClick={() => setFiles((cur) => cur.filter((_, j) => j !== i))}
                >
                  <Icon name="x" />
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="composer__row">
          <button
            type="button"
            className="btn btn--quiet btn--icon"
            aria-label={t.chat.attach}
            title={t.chat.attachHint}
            onClick={() => picker.current?.click()}
          >
            <Icon name="clip" />
          </button>
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              addFiles(e.target.files);
              e.target.value = '';
            }}
          />
          <textarea
            ref={area}
            className="textarea composer__input"
            rows={1}
            value={draft}
            placeholder={t.chat.placeholder}
            aria-label={t.chat.placeholder}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKey}
            onFocus={() => setListening(true)}
            onBlur={() => setListening(false)}
          />
          {canStop ? (
            <button type="button" className="btn btn--danger composer__send" onClick={stop} disabled={stopping}>
              <Icon name="stop" />
              {stopping ? t.chat.stopping : t.chat.stop}
            </button>
          ) : (
            <button
              type="submit"
              className="btn btn--primary composer__send"
              disabled={sending || (!draft.trim() && files.length === 0)}
            >
              <Icon name="send" />
              {t.chat.send}
            </button>
          )}
        </div>
      </form>
    </div>
  );
}

function MessageBlock({
  m,
  task,
  showStrip,
  onOpenTasks,
}: {
  m: ChatMessage;
  task: Task | undefined;
  showStrip: boolean;
  onOpenTasks: () => void;
}) {
  if (m.role === 'user') {
    return (
      <article className="msg msg--user" aria-label={t.chat.userLabel}>
        <div className="msg__bubble">
          <p className="msg__plain">{m.content}</p>
          {m.attachments.length > 0 && (
            <ul className="chips chips--inline">
              {m.attachments.map((a) => (
                <li key={a.name} className="chip">
                  <Icon name="file" />
                  <span className="chip__name">{a.name}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
        {showStrip && task?.kind === 'job' && <TaskStrip task={task} onOpenTasks={onOpenTasks} />}
      </article>
    );
  }
  return (
    <article className="msg msg--assistant" aria-label={t.chat.assistantLabel}>
      <div className="msg__body">
        <Markdown text={m.content} streaming={m.streaming} />
        {m.streaming && <span className="sr-only">{t.chat.streaming}</span>}
        {!m.streaming && m.content === '' && <span className="subtle">{t.chat.stopped}</span>}
      </div>
      <SourceList sources={m.sources} />
      {task && (
        <p className="msg__model subtle">
          {task.model ? t.chat.modelLine(task.model.provider, task.model.model_id) : null}
        </p>
      )}
      {showStrip && task?.kind === 'job' && <TaskStrip task={task} onOpenTasks={onOpenTasks} />}
    </article>
  );
}

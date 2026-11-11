import { t } from '../lib/strings';
import type { SpriteState } from './spriteState';
import './sprite.css';

/**
 * PLACEHOLDER ART: an original teardrop "wisp" drawn in code so the six states can be
 * reviewed. Every state differs by SHAPE (ears, orbit ring, badge, sprout, flat dome)
 * and by MOTION, never by colour alone. The final art replaces this file's SVG only.
 */
const BODY = 'M32 5C37 14 50 24 50 39A18 18 0 0 1 14 39C14 24 27 14 32 5Z';
const DOME = 'M9 53C9 36 19 26 32 26S55 36 55 53Z';

function Eyes({ state }: { state: SpriteState }) {
  switch (state) {
    case 'listening':
      return (
        <g className="sprite__eyes">
          <circle cx="25" cy="40" r="3.8" fill="var(--sprite-ink)" />
          <circle cx="39" cy="40" r="3.8" fill="var(--sprite-ink)" />
          <circle cx="26.2" cy="38.8" r="1.1" fill="var(--sprite-body-light)" />
          <circle cx="40.2" cy="38.8" r="1.1" fill="var(--sprite-body-light)" />
        </g>
      );
    case 'working':
      return (
        <path
          d="M22 41h7M35 41h7"
          stroke="var(--sprite-ink)"
          strokeWidth="3"
          strokeLinecap="round"
        />
      );
    case 'awaiting':
      return (
        <g>
          <circle cx="27" cy="38" r="2.8" fill="var(--sprite-ink)" />
          <circle cx="40" cy="38" r="2.8" fill="var(--sprite-ink)" />
        </g>
      );
    case 'dnd':
      return (
        <path
          d="M20 43q4 3.5 8 0M36 43q4 3.5 8 0"
          stroke="var(--sprite-ink)"
          strokeWidth="2.6"
          strokeLinecap="round"
          fill="none"
        />
      );
    default:
      return (
        <g className="sprite__eyes">
          <circle cx="25.5" cy="40" r="2.7" fill="var(--sprite-ink)" />
          <circle cx="38.5" cy="40" r="2.7" fill="var(--sprite-ink)" />
        </g>
      );
  }
}

export function Sprite({
  state,
  size,
  title,
}: {
  state: SpriteState;
  size?: number | string;
  title?: string;
}) {
  const muted = state === 'dnd';
  const fill = muted ? 'var(--sprite-muted-body)' : 'url(#elf-body-grad)';
  const stroke = muted ? 'var(--sprite-muted-outline)' : 'var(--sprite-outline)';
  const bodyTransform =
    state === 'working'
      ? 'translate(32 36) scale(.78) translate(-32 -36)'
      : state === 'awaiting'
        ? 'translate(-2 4) scale(.9)'
        : state === 'suggestion'
          ? 'translate(32 62) scale(.88) translate(-32 -62)'
          : state === 'listening'
            ? 'translate(32 60) scale(.92) translate(-32 -60)'
            : undefined;

  return (
    <svg
      className={`sprite sprite--${state}`}
      viewBox="0 0 64 64"
      width={size}
      height={size}
      role="img"
      aria-label={title ?? t.sprite.label[state]}
    >
      <defs>
        <linearGradient id="elf-body-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" style={{ stopColor: 'var(--sprite-body-light)' }} />
          <stop offset="1" style={{ stopColor: 'var(--sprite-body)' }} />
        </linearGradient>
      </defs>

      {state === 'listening' && (
        <g className="sprite__waves" fill="none" stroke="var(--sprite-outline)" strokeWidth="2.6" strokeLinecap="round">
          <path className="sprite__wave sprite__wave--1" d="M9 31Q4 40 9 49M55 31Q60 40 55 49" />
          <path className="sprite__wave sprite__wave--2" d="M3 26Q-3 40 3 54M61 26Q67 40 61 54" />
        </g>
      )}

      {state === 'working' && (
        <g className="sprite__orbit">
          <circle cx="32" cy="36" r="28" fill="none" stroke="var(--sprite-outline)" strokeWidth="2" strokeDasharray="2 7" strokeLinecap="round" />
          <circle cx="32" cy="8" r="4" fill="var(--sprite-body-light)" stroke="var(--sprite-outline)" strokeWidth="2" />
          <circle cx="56.2" cy="50" r="4" fill="var(--sprite-body-light)" stroke="var(--sprite-outline)" strokeWidth="2" />
          <circle cx="7.8" cy="50" r="4" fill="var(--sprite-body-light)" stroke="var(--sprite-outline)" strokeWidth="2" />
        </g>
      )}

      {state === 'suggestion' && (
        <g className="sprite__sprout">
          <path d="M32 13V7" stroke="var(--sprite-leaf)" strokeWidth="2.6" strokeLinecap="round" />
          <path d="M32 8C26 3 21 5 22 10C27 12 31 11 32 8Z" fill="var(--sprite-leaf)" stroke="var(--sprite-outline)" strokeWidth="1.6" strokeLinejoin="round" />
          <path d="M32 8C38 2 43 4 42 9C37 11 33 11 32 8Z" fill="var(--sprite-leaf)" stroke="var(--sprite-outline)" strokeWidth="1.6" strokeLinejoin="round" />
        </g>
      )}

      <g className="sprite__body-wrap">
        <g transform={bodyTransform}>
          <path
            className="sprite__body"
            d={muted ? DOME : BODY}
            fill={fill}
            stroke={stroke}
            strokeWidth="2.4"
            strokeLinejoin="round"
            strokeDasharray={muted ? '4 3.5' : undefined}
          />
          {!muted && (
            <ellipse cx="24" cy="31" rx="3.2" ry="5.6" fill="var(--sprite-highlight)" transform="rotate(18 24 31)" />
          )}
          <Eyes state={state} />
        </g>
      </g>

      {state === 'awaiting' && (
        <g className="sprite__badge">
          <rect x="40" y="2" width="22" height="22" rx="6" fill="var(--sprite-badge)" stroke="var(--sprite-outline)" strokeWidth="2" />
          <rect x="49" y="7" width="4" height="9" rx="2" fill="var(--sprite-badge-ink)" />
          <circle cx="51" cy="19.2" r="2.2" fill="var(--sprite-badge-ink)" />
        </g>
      )}

      {state === 'dnd' && (
        <path
          className="sprite__zzz"
          d="M44 11h8l-8 9h8"
          fill="none"
          stroke="var(--sprite-muted-outline)"
          strokeWidth="2.6"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      )}
    </svg>
  );
}

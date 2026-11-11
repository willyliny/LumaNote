import type { SVGProps } from 'react';

const PATHS = {
  chat: 'M4 5h16v11H9l-5 4V5z',
  tasks: 'M5 6h2M5 12h2M5 18h2M10 6h9M10 12h9M10 18h9',
  today: 'M4 6h16v14H4zM4 10h16M8 3v4M16 3v4',
  gear: 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM12 3l1.6 2.4 2.8-.6.9 2.7 2.7.9-.6 2.8L21 12l-2.6 1.8.6 2.8-2.7.9-.9 2.7-2.8-.6L12 21l-1.6-2.4-2.8.6-.9-2.7-2.7-.9.6-2.8L3 12l2.6-1.8-.6-2.8 2.7-.9.9-2.7 2.8.6z',
  send: 'M4 12l16-8-6 17-3-7z',
  stop: 'M7 7h10v10H7z',
  clip: 'M16 8l-7 7a2.5 2.5 0 0 0 3.5 3.5l8-8a4.5 4.5 0 0 0-6.4-6.4l-8 8a6.5 6.5 0 0 0 9.2 9.2',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  x: 'M6 6l12 12M18 6L6 18',
  clock: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM12 8v4.5l3 1.5',
  pause: 'M9 6v12M15 6v12',
  question: 'M9.5 9a2.5 2.5 0 1 1 3.6 2.2c-.7.4-1.1.9-1.1 1.8M12 17v.5',
  slash: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM6.5 6.5l11 11',
  file: 'M7 3h7l4 4v14H7zM14 3v4h4',
  copy: 'M9 9h10v11H9zM5 15V4h10',
  chevron: 'M8 10l4 4 4-4',
  plus: 'M12 5v14M5 12h14',
  trash: 'M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13',
  pencil: 'M5 19l1-4L16 5l3 3L9 18zM14 7l3 3',
  link: 'M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1',
  lock: 'M6 11h12v9H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3',
  moon: 'M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z',
  leaf: 'M5 19c0-8 5-13 14-13 0 9-5 14-13 14M5 19l7-7',
  shield: 'M12 3l7 3v5c0 5-3 8-7 10-4-2-7-5-7-10V6z',
  alert: 'M12 4l9 16H3zM12 10v4M12 17v.5',
  spin: 'M12 4a8 8 0 1 0 8 8',
  dot: 'M12 12m-3 0a3 3 0 1 0 6 0a3 3 0 1 0-6 0',
  ring: 'M12 12m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0',
  dots: 'M12 12m-6 0a6 6 0 1 0 12 0a6 6 0 1 0-12 0M9 12h.01M12 12h.01M15 12h.01',
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, className, ...rest }: { name: IconName } & SVGProps<SVGSVGElement>) {
  return (
    <svg
      className={`icon ${className ?? ''}`}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      <path d={PATHS[name]} />
    </svg>
  );
}

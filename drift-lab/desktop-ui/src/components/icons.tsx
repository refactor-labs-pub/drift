/* Shared inline icons (kept tiny — no extra dep). */

const base = {
  fill: "none",
  stroke: "currentColor",
  viewBox: "0 0 24 24",
} as const;

export const FolderIcon = () => (
  <svg {...base} strokeWidth={2}>
    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
  </svg>
);

export const GridIcon = () => (
  <svg {...base} strokeWidth={2}>
    <rect x="3" y="3" width="7" height="7" rx="1" />
    <rect x="14" y="3" width="7" height="7" rx="1" />
    <rect x="3" y="14" width="7" height="7" rx="1" />
    <rect x="14" y="14" width="7" height="7" rx="1" />
  </svg>
);

export const CodeIcon = () => (
  <svg {...base} strokeWidth={2}>
    <polyline points="16 18 22 12 16 6" />
    <polyline points="8 6 2 12 8 18" />
  </svg>
);

export const UploadIcon = () => (
  <svg {...base} strokeWidth={2}>
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <polyline points="17 8 12 3 7 8" />
    <line x1="12" y1="3" x2="12" y2="15" />
  </svg>
);

export const PlayIcon = () => (
  <svg {...base} strokeWidth={2}>
    <polygon points="5 3 19 12 5 21 5 3" />
  </svg>
);

export const FlashIcon = () => (
  <svg {...base} strokeWidth={2}>
    <path d="M9 11H1l3-9 4 9H4" />
    <path d="M22 12h-4l-3 9-4-9h3" />
  </svg>
);

export const CheckIcon = () => (
  <svg {...base} strokeWidth={3}>
    <polyline points="20 6 9 17 4 12" />
  </svg>
);

export const ArrowRightIcon = () => (
  <svg {...base} strokeWidth={2.5} width={14} height={14}>
    <line x1="5" y1="12" x2="19" y2="12" />
    <polyline points="12 5 19 12 12 19" />
  </svg>
);

export const BoltIcon = () => (
  <svg {...base} strokeWidth={2.5}>
    <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
  </svg>
);

export const SettingsIcon = () => (
  <svg {...base} strokeWidth={2}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
);

export const XIcon = () => (
  <svg {...base} strokeWidth={2.5}>
    <line x1="18" y1="6" x2="6" y2="18" />
    <line x1="6" y1="6" x2="18" y2="18" />
  </svg>
);

export const STEP_ICONS = [
  <GridIcon />,
  <CodeIcon />,
  <UploadIcon />,
  <PlayIcon />,
  <FlashIcon />,
];

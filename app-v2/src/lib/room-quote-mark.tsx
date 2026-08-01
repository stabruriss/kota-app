export function RoomQuoteMark({
  className,
}: {
  className?: string;
}) {
  return (
    <svg
      className={className}
      viewBox="0 0 16 16"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M7.12 3.55C4.84 4.36 3.56 6.02 3.56 8.18c0 1.34.72 2.24 1.82 2.24 1.04 0 1.78-.78 1.78-1.88 0-.96-.59-1.66-1.48-1.85.2-.76.77-1.42 1.72-1.94l-.28-1.2Z" />
      <path d="M8.88 10.45c2.28-.81 3.56-2.47 3.56-4.63 0-1.34-.72-2.24-1.82-2.24-1.04 0-1.78.78-1.78 1.88 0 .96.59 1.66 1.48 1.85-.2.76-.77 1.42-1.72 1.94l.28 1.2Z" />
    </svg>
  );
}

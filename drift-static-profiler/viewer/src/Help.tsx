import { useState, useRef, useEffect, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

interface Props {
  /** Tooltip text. */
  text: string;
  /**
   * When provided, the children are wrapped in a hover-triggering span with
   * a dotted underline. The whole label becomes the trigger, not just the `?`.
   * When omitted, only the `?` chip is rendered (legacy standalone mode).
   */
  children?: ReactNode;
  /** Optional sizing override for the ? chip. */
  size?: number;
}

/**
 * Hover tooltip. Renders as a portal at document.body so it can't be clipped
 * by scrollable parents. Appears instantly, styled to match the UI.
 *
 * Two modes:
 *   <Help text="..." />              — standalone "?" chip
 *   <Help text="...">label</Help>    — wraps label; whole label is the trigger
 */
export function Help({ text, children, size = 12 }: Props) {
  const [show, setShow] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number; placement: 'below' | 'above' }>({
    x: 0,
    y: 0,
    placement: 'below',
  });
  const triggerRef = useRef<HTMLSpanElement>(null);

  const recompute = () => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    const placement = spaceBelow < 200 ? 'above' : 'below';
    const y = placement === 'below' ? rect.bottom + 6 : rect.top - 6;
    const x = Math.min(window.innerWidth - 360, Math.max(8, rect.left));
    setPos({ x, y, placement });
  };

  useEffect(() => {
    if (!show) return;
    recompute();
    const onScroll = () => setShow(false);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', recompute);
    return () => {
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', recompute);
    };
  }, [show]);

  const popover = show
    ? createPortal(
        <div style={popoverStyle(pos)} role="tooltip">
          {text}
        </div>,
        document.body,
      )
    : null;

  // Wrapper mode: the whole label is the trigger. Dotted underline is the
  // affordance — no ? chip, because that would be a second tooltip indicator
  // for the same element. One label, one popover.
  if (children !== undefined) {
    return (
      <span
        ref={triggerRef}
        onMouseEnter={() => setShow(true)}
        onMouseLeave={() => setShow(false)}
        onFocus={() => setShow(true)}
        onBlur={() => setShow(false)}
        tabIndex={0}
        aria-label={`help: ${text}`}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          cursor: 'help',
          textDecoration: 'underline dotted',
          textDecorationColor: show ? '#5b8def' : '#5b5e64',
          textUnderlineOffset: 3,
        }}
      >
        {children}
        {popover}
      </span>
    );
  }

  // Standalone chip mode.
  return (
    <>
      <span
        ref={triggerRef}
        onMouseEnter={() => setShow(true)}
        onMouseLeave={() => setShow(false)}
        onFocus={() => setShow(true)}
        onBlur={() => setShow(false)}
        tabIndex={0}
        aria-label={`help: ${text}`}
        style={chipStyle(show, size)}
      >
        ?
      </span>
      {popover}
    </>
  );
}

function chipStyle(show: boolean, size: number): React.CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: size,
    height: size,
    borderRadius: '50%',
    background: show ? '#5b8def' : '#3f4147',
    color: show ? '#0a0a14' : '#d7d9dc',
    fontSize: Math.max(8, size - 3),
    fontWeight: 700,
    cursor: 'help',
    userSelect: 'none',
    verticalAlign: 'middle',
    transition: 'background 0.12s, color 0.12s',
  };
}

function popoverStyle(pos: { x: number; y: number; placement: 'below' | 'above' }): React.CSSProperties {
  return {
    position: 'fixed',
    left: pos.x,
    top: pos.placement === 'below' ? pos.y : undefined,
    bottom: pos.placement === 'above' ? window.innerHeight - pos.y : undefined,
    zIndex: 9999,
    maxWidth: 340,
    background: '#1e1f22',
    color: '#d7d9dc',
    border: '1px solid #5b8def',
    borderRadius: 4,
    padding: '8px 10px',
    fontSize: 12,
    lineHeight: 1.45,
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
    fontWeight: 400,
    textTransform: 'none',
    letterSpacing: 'normal',
    boxShadow: '0 4px 16px rgba(0,0,0,0.5)',
    pointerEvents: 'none',
    whiteSpace: 'pre-wrap',
  };
}

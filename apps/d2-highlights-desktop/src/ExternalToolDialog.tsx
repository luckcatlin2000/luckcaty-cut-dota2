import { useEffect, useId, useRef } from "react";
import { Download, X } from "lucide-react";
import "./externalToolDialog.css";

export interface ExternalToolDialogProps {
  notice: { title: string; message: string } | null;
  onClose: () => void;
}

export function ExternalToolDialog({ notice, onClose }: ExternalToolDialogProps) {
  return notice ? <ExternalToolDialogContent notice={notice} onClose={onClose} /> : null;
}

function ExternalToolDialogContent({ notice, onClose }: {
  notice: NonNullable<ExternalToolDialogProps["notice"]>;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement : null;
    const focusable = () => Array.from(dialog.querySelectorAll<HTMLElement>(
      "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    )).filter((element) => !element.hidden && element.getClientRects().length > 0);
    const focusFirst = () => (confirmRef.current ?? focusable()[0] ?? dialog).focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        closeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const elements = focusable();
      const first = elements[0];
      const last = elements[elements.length - 1];
      if (!first || !last) {
        event.preventDefault();
        dialog.focus();
      } else if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };
    const onFocusIn = (event: FocusEvent) => {
      if (event.target instanceof Node && !dialog.contains(event.target)) focusFirst();
    };
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("focusin", onFocusIn, true);
    focusFirst();
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("focusin", onFocusIn, true);
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    };
  }, []);

  return (
    <div
      className="external-tool-dialog-backdrop"
      onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}
    >
      <div
        className="external-tool-dialog"
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        tabIndex={-1}
      >
        <button className="external-tool-dialog-close" type="button" aria-label="关闭独立软件提示" onClick={onClose}>
          <X aria-hidden="true" />
        </button>
        <div className="external-tool-dialog-icon" aria-hidden="true"><Download /></div>
        <h2 id={titleId}>{notice.title}</h2>
        <p id={descriptionId}>{notice.message}</p>
        <button className="external-tool-dialog-confirm" type="button" ref={confirmRef} onClick={onClose}>知道了</button>
      </div>
    </div>
  );
}

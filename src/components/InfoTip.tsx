import { useId } from "react";

type InfoTipProps = {
  label: string;
  children: React.ReactNode;
};

export function InfoTip({ label, children }: InfoTipProps) {
  const tooltipId = useId();

  return (
    <span className="info-tip">
      <button
        className="info-tip-trigger"
        type="button"
        aria-label={label}
        aria-describedby={tooltipId}
      >
        <span aria-hidden="true">?</span>
      </button>
      <span className="info-tip-content" id={tooltipId} role="tooltip">
        {children}
      </span>
    </span>
  );
}

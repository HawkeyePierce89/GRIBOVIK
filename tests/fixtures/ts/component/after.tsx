import { classNames } from "./util";

export interface BadgeProps {
  label: string;
  tone?: string;
}

export const Badge = ({ label, tone }: BadgeProps) => {
  const cls = classNames("badge", tone);
  return <span className={cls}>{label}</span>;
};

export default function Panel(): JSX.Element {
  return (
    <div>
      <Badge label="ok" tone="green" />
    </div>
  );
}

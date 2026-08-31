import { classNames } from "./util";

export const Badge = ({ label }: { label: string }) => {
  const cls = classNames("badge");
  return <span className={cls}>{label}</span>;
};

export default function Panel(): JSX.Element {
  return (
    <div>
      <Badge label="ok" />
    </div>
  );
}

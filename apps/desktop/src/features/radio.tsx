import type { KeyboardEvent, ReactNode } from "react";

// Radio groups made of buttons: one tab stop, arrow keys move and choose
// (the WAI-ARIA radio pattern, audit U10).

export interface RadioItem {
  checked: boolean;
  disabled?: boolean;
}

/** Index of the radio that takes Tab: the checked one, else the first that
 * can be chosen; -1 when none can. */
export function tabStop(items: RadioItem[]): number {
  const checked = items.findIndex((i) => i.checked && !i.disabled);
  return checked >= 0 ? checked : items.findIndex((i) => !i.disabled);
}

/** Where `key` moves from `current`, skipping disabled radios and wrapping;
 * null when the key isn't one that moves. */
export function nextRadio(key: string, current: number, items: RadioItem[]): number | null {
  const enabled = items.flatMap((it, i) => (it.disabled ? [] : [i]));
  if (enabled.length < 2) return null;
  const at = enabled.indexOf(current);
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
      return enabled[(at + 1) % enabled.length]!;
    case "ArrowLeft":
    case "ArrowUp":
      return enabled[(at - 1 + enabled.length) % enabled.length]!;
    case "Home":
      return enabled[0]!;
    case "End":
      return enabled[enabled.length - 1]!;
    default:
      return null;
  }
}

export function RadioGroup<T extends RadioItem & { key: string }>({
  label,
  className,
  items,
  render,
}: {
  label: string;
  className?: string;
  items: T[];
  /** Renders one radio (`role="radio"`) with the tab index it is given. */
  render: (item: T, tabIndex: 0 | -1) => ReactNode;
}) {
  const stop = tabStop(items);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const radios = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]'));
    const current = radios.findIndex((r) => r === (e.target as Element).closest('[role="radio"]'));
    if (current < 0) return;
    const next = nextRadio(e.key, current, radios.map((r) => ({ checked: false, disabled: r.disabled })));
    if (next === null) return;
    e.preventDefault();
    radios[next]!.focus();
    // Arrow keys choose, as native radios do.
    radios[next]!.click();
  };
  return (
    <div role="radiogroup" aria-label={label} className={className} onKeyDown={onKeyDown}>
      {items.map((item, i) => render(item, i === stop ? 0 : -1))}
    </div>
  );
}

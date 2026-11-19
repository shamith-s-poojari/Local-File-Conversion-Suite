// The page organizer's operations on its list. Each page carries an `id` that
// stays with it through every move, so React keeps the same element for it
// (and focus can follow it). The ids never leave the widget: the value sent
// on is just the entries.

export interface Entry {
  source: number;
  rotate?: 0 | 90 | 180 | 270;
}

export interface Item {
  id: string;
  entry: Entry;
}

export function moveItem(items: Item[], i: number, by: number): Item[] {
  const j = i + by;
  if (j < 0 || j >= items.length) return items;
  const next = [...items];
  const [item] = next.splice(i, 1);
  next.splice(j, 0, item!);
  return next;
}

export function rotateItem(items: Item[], i: number): Item[] {
  return items.map((it, j) =>
    j === i ? { ...it, entry: { ...it.entry, rotate: (((it.entry.rotate ?? 0) + 90) % 360) as Entry["rotate"] } } : it,
  );
}

/** The last page can't be deleted: a PDF needs one. */
export function removeItem(items: Item[], i: number): Item[] {
  return items.length > 1 ? items.filter((_, j) => j !== i) : items;
}

/** The copy goes right after the original and gets `id`. */
export function duplicateItem(items: Item[], i: number, id: string): Item[] {
  return [...items.slice(0, i + 1), { id, entry: { ...items[i]!.entry } }, ...items.slice(i + 1)];
}
